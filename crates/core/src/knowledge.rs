//! Knowledge base integration — maintain a Karpathy-style LLM wiki from meeting data.
//!
//! After each meeting, extract facts about people and decisions, update person
//! profiles, append to a chronological log, and maintain an index. All writes
//! include provenance (source meeting) and confidence levels to prevent
//! hallucination propagation.

use crate::config::{Config, KnowledgeConfig};
use crate::markdown::{is_inactive_corpus_dir_name, Frontmatter, Sensitivity};
use cap_std::fs::{
    Dir as CapDir, OpenOptions as CapOpenOptions, OpenOptionsExt as CapOpenOptionsExt,
};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::hash_map::RandomState;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::hash::BuildHasher;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

const QMD_MIRROR_DIR: &str = "qmd-policy-mirror";
const QMD_MIRROR_MARKER: &str = ".minutes-policy-mirror-v1.json";
const QMD_RETIREMENT_RECEIPT: &str = ".minutes-retirement-receipt-v1.json";
const QMD_RETIREMENT_RECEIPT_BYTES: &[u8] = b"{\"schema\":2,\"state\":\"retained-private\"}\n";
const KNOWLEDGE_POLICY_LOCK: &str = "knowledge-policy.lock";
// Positive publication authority must never live in the agent-readable
// knowledge tree. Keep the legacy public name only so it can be quarantined;
// the authoritative manifest is descriptor-bound beneath the owner-private
// preservation namespace outside both configured public roots.
const LEGACY_PUBLIC_KNOWLEDGE_PROVENANCE_MANIFEST: &str = ".minutes-provenance-v2.json";
const PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST: &str = "knowledge-provenance-v4.json";
const PRIVATE_KNOWLEDGE_PROVENANCE_TEMP: &str = ".knowledge-provenance-v4.tmp";
#[cfg(any(windows, test))]
const PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP: &str = ".knowledge-provenance-v4.previous";
#[cfg(any(windows, test))]
const PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS: [&str; 3] = [
    ".knowledge-provenance-v4.txn-a",
    ".knowledge-provenance-v4.txn-b",
    ".knowledge-provenance-v4.txn-c",
];
#[cfg(any(windows, test))]
const MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES: u64 = 4 * 1024;
const MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;
const MAX_RETAINED_RECONCILIATION_CAPTURES: usize = 256;
const MAX_RETAINED_RECONCILIATION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES: u64 = 2 * 1024 * 1024;
const MAX_PARA_PERSON_RECONCILIATION_BYTES: u64 = 4 * 1024 * 1024;
const MAX_PARA_RECONCILIATION_ITEMS: usize = 4096;
const MAX_PARA_RECONCILIATION_VALUE_DEPTH: usize = 32;
const MAX_PARA_PERSON_GENERATIONS: usize = 64;
const PARA_PERSON_STAGE_PREFIX: &str = ".minutes-person-publish-";
const PARA_PERSON_CAPTURE_PREFIX: &str = ".minutes-person-capture-";
const PARA_PERSON_FAILED_PREFIX: &str = ".minutes-person-failed-";
const PARA_PERSON_TRANSACTION_PREFIX: &str = ".minutes-person-transaction-";
const PARA_PERSON_COMPLETED_TRANSACTION_PREFIX: &str = ".minutes-person-completed-transaction-";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const PARA_PERSON_SLOT_PREFIX: &str = ".minutes-person-slot-v2-";
#[cfg(any(target_os = "linux", target_os = "macos"))]
const PARA_PERSON_PARKED_PREFIX: &str = ".minutes-person-parked-v2-";
const PARA_PRIVATE_ROOT: &str = ".minutes-para-private-v1";
const MAX_PARA_TRANSACTION_BYTES: u64 = 2 * 1024 * 1024;
const INERT_RECONCILIATION_TOMBSTONE_NOTICE_THRESHOLD: usize = 256;
#[cfg(test)]
const MAX_RETAINED_PUBLICATION_TEMPS: usize = 64;
#[cfg(test)]
const MAX_RETAINED_PUBLICATION_BYTES: u64 = 16 * 1024 * 1024;
const QMD_POLICY_LOCK: &str = "qmd-policy-mirror.lock";
const QMD_OWNED_TARGET: &str = "qmd-owned-target-v1.json";
const QMD_RETIREMENT_PENDING: &str = "qmd-retirement-pending-v1";
pub const AGENT_TRUST_READINESS_REMEDIATION: &str =
    "This machine shows a Minutes-owned QMD registration that qmd could not confirm was removed. Make sure `qmd` runs (`qmd collection list`), then run `minutes qmd cleanup` and restart Minutes.";
/// Privacy-safe reason returned when a caller requests persistent QMD state.
pub const QMD_PERSISTENCE_DISABLED_REASON: &str = "Persistent QMD collections are disabled because QMD's global index cannot guarantee revocation after an external meeting-policy change";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QmdRetirementReadiness {
    ReadyClean,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentTrustReadiness {
    pub schema: u32,
    pub ready: bool,
    pub qmd_retirement: QmdRetirementReadiness,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remediation: Option<String>,
}

impl AgentTrustReadiness {
    fn ready(qmd_retirement: QmdRetirementReadiness) -> Self {
        Self {
            schema: 1,
            ready: true,
            qmd_retirement,
            remediation: None,
        }
    }

    fn blocked() -> Self {
        Self {
            schema: 1,
            ready: false,
            qmd_retirement: QmdRetirementReadiness::Blocked,
            remediation: Some(AGENT_TRUST_READINESS_REMEDIATION.to_string()),
        }
    }

    pub fn require_ready(&self) -> Result<(), String> {
        if self.ready && self.qmd_retirement == QmdRetirementReadiness::ReadyClean {
            Ok(())
        } else {
            Err(format!(
                "Agent memory is unavailable until legacy QMD retirement is confirmed. {}",
                AGENT_TRUST_READINESS_REMEDIATION
            ))
        }
    }
}

// This flag records only that the application crossed its startup boundary. It
// deliberately does not cache the result: QMD's external registry can change
// independently of Minutes, so every content authorization must revalidate it.
static AGENT_TRUST_BOUNDARY_ESTABLISHED: AtomicBool = AtomicBool::new(false);
static INERT_RECONCILIATION_TOMBSTONES_REPORTED: AtomicBool = AtomicBool::new(false);

// ── Types ───────────────────────────────────────────────────────

/// Confidence level for an extracted fact. Mirrors events::InsightConfidence
/// but applied to knowledge base writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Topic discussed, possible direction — never written to profiles by default.
    Tentative,
    /// Inferred from discussion flow.
    Inferred,
    /// Clear discussion → conclusion pattern, or extracted from structured YAML.
    Strong,
    /// Verbatim quote or explicit statement: "We decided...", "I commit to...".
    Explicit,
}

impl Confidence {
    pub fn parse(s: &str) -> Self {
        match s {
            "explicit" => Confidence::Explicit,
            "strong" => Confidence::Strong,
            "inferred" => Confidence::Inferred,
            "tentative" => Confidence::Tentative,
            other => {
                tracing::warn!(
                    value = other,
                    "unknown confidence level in config, defaulting to 'strong' (safe)"
                );
                Confidence::Strong
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::Explicit => "explicit",
            Confidence::Strong => "strong",
            Confidence::Inferred => "inferred",
            Confidence::Tentative => "tentative",
        }
    }

    /// Whether this confidence meets or exceeds the given threshold.
    pub fn meets(&self, threshold: Confidence) -> bool {
        *self >= threshold
    }
}

/// A single extracted fact about a person, with provenance.
#[derive(Debug, Clone)]
pub struct Fact {
    pub text: String,
    pub category: String, // "decision", "commitment", "context", "preference", "relationship"
    pub confidence: Confidence,
    pub source_meeting: String, // filename slug for traceability
    pub source_date: String,    // ISO date
}

/// Facts grouped by person.
#[derive(Debug, Clone)]
pub struct PersonFacts {
    pub slug: String,
    pub name: String,
    pub facts: Vec<Fact>,
}

/// A log entry for the append-only chronological log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub date: DateTime<Local>,
    pub meeting_title: String,
    pub meeting_path: String,
    pub people_updated: Vec<String>,
    pub fact_count: usize,
    pub skipped_count: usize, // facts below confidence threshold
}

/// Result of a knowledge update operation.
#[derive(Debug)]
pub struct UpdateResult {
    pub facts_written: usize,
    pub facts_skipped: usize, // below confidence threshold
    pub people_updated: Vec<String>,
}

#[derive(Debug)]
struct KnowledgeWriteCommit {
    update: UpdateResult,
    records: BTreeSet<String>,
}

/// Result of rebuilding the QMD corpus boundary from live, policy-authorized files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QmdMirrorResult {
    pub path: PathBuf,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct QmdMirrorMarker {
    schema: u32,
    source: String,
    policy: String,
    sources: BTreeMap<String, String>,
}

/// Result of removing knowledge facts whose source is no longer policy-authorized.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KnowledgeReconcileResult {
    pub facts_removed: usize,
    pub log_entries_removed: usize,
}

/// Privacy-safe knowledge status returned by the Rust-owned pre-read gate.
///
/// Deliberately excludes filesystem paths and source identities. Callers must
/// not inspect the knowledge tree independently after receiving this snapshot:
/// reconciliation and counting happen under the same policy lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KnowledgeStatusSnapshot {
    pub enabled: bool,
    pub configured: bool,
    pub adapter: Option<String>,
    pub engine: Option<String>,
    pub people_count: usize,
    pub log_entries: usize,
}

#[derive(Debug)]
struct PolicyDeniedAfterConfirmedRetraction {
    source_scope: String,
    reason: String,
}

impl std::fmt::Display for PolicyDeniedAfterConfirmedRetraction {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "policy-ineligible meeting ({}): {}; derivatives retracted",
            self.source_scope, self.reason
        )
    }
}

impl std::error::Error for PolicyDeniedAfterConfirmedRetraction {}

/// Whether a failed knowledge update completed and durably confirmed its
/// cleanup before returning the error.
pub fn knowledge_failure_cleanup_confirmed(error: &(dyn std::error::Error + 'static)) -> bool {
    error
        .downcast_ref::<PolicyDeniedAfterConfirmedRetraction>()
        .is_some()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
struct KnowledgeProvenanceManifest {
    schema: u32,
    sources: BTreeMap<String, String>,
    #[serde(default)]
    records: BTreeMap<String, BTreeSet<String>>,
    #[serde(default)]
    managed_logs: BTreeSet<String>,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WindowsProvenanceFileProof {
    identity: QmdObjectIdentity,
    len: u64,
    sha256: [u8; 32],
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum WindowsProvenanceJournalState {
    Baseline,
    Active,
    Completed,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WindowsProvenanceJournalRecord {
    schema: u32,
    sequence: u64,
    state: WindowsProvenanceJournalState,
    #[serde(default)]
    prior_sequence: Option<u64>,
    #[serde(default)]
    previous: Option<WindowsProvenanceFileProof>,
    #[serde(default)]
    intended: Option<WindowsProvenanceFileProof>,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsProvenanceObservedFile {
    Absent,
    Empty,
    Previous,
    Intended,
    Other,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsProvenanceActiveLayout {
    PreMutation,
    PreviousParked,
    PublishedWithPrevious,
    PublishedRetired,
    Unknown,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsProvenanceObservation {
    state: WindowsProvenanceObservedFile,
    proof: Option<WindowsProvenanceFileProof>,
}

#[cfg(any(windows, test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsProvenanceInitializationBoundary {
    TemporaryRetirement,
    BackupRetirement,
    JournalReset(usize),
    BaselineWrite,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyKnowledgeProvenanceManifestV3 {
    schema: u32,
    sources: BTreeMap<String, u64>,
    #[serde(default)]
    records: BTreeMap<String, BTreeSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordDisposition {
    Keep,
    RemoveOwned,
    Quarantine,
}

// ── Public API ──────────────────────────────────────────────────

// ── Policy-authorized source snapshots ───────────────────────────────────────

#[derive(Debug)]
struct AuthorizedMeeting {
    path: PathBuf,
    content: String,
    frontmatter: Frontmatter,
}

fn path_has_only_normal_components(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn active_corpus_relative_path(relative: &Path) -> bool {
    path_has_only_normal_components(relative)
        && !relative.components().any(|component| {
            let Component::Normal(name) = component else {
                return false;
            };
            is_inactive_corpus_dir_name(name)
        })
}

fn lexical_platform_aliases(path: &Path) -> Vec<PathBuf> {
    #[cfg(target_os = "macos")]
    let mut aliases = vec![path.to_path_buf()];
    #[cfg(not(target_os = "macos"))]
    let aliases = vec![path.to_path_buf()];
    #[cfg(target_os = "macos")]
    {
        for (short, private) in [
            (Path::new("/var"), Path::new("/private/var")),
            (Path::new("/tmp"), Path::new("/private/tmp")),
            (Path::new("/etc"), Path::new("/private/etc")),
        ] {
            if let Ok(relative) = path.strip_prefix(short) {
                let alias = private.join(relative);
                if !aliases.contains(&alias) {
                    aliases.push(alias);
                }
            }
            if let Ok(relative) = path.strip_prefix(private) {
                let alias = short.join(relative);
                if !aliases.contains(&alias) {
                    aliases.push(alias);
                }
            }
        }
    }
    aliases
}

/// Whether a Markdown path belongs to the configured live meeting corpus.
///
/// This is the shared path-scope predicate for direct reads, recursive
/// traversal, CLI selection, QMD mirroring, and knowledge reconciliation. It
/// intentionally supports a lexically contained path that has just been
/// deleted, so mutation hooks can retract its exact derivatives. This is a
/// scope check only; callers that read bytes must still use
/// `authorized_meeting` for descriptor-bound classification.
pub fn is_live_corpus_path(path: &Path, config: &Config) -> bool {
    if path.extension().is_none_or(|extension| extension != "md") {
        return false;
    }
    let lexical_root = if config.output_dir.is_absolute() {
        config.output_dir.clone()
    } else {
        std::env::current_dir()
            .map(|current| current.join(&config.output_dir))
            .unwrap_or_else(|_| config.output_dir.clone())
    };
    let lexical = if path.is_absolute() {
        path.to_path_buf()
    } else {
        lexical_root.join(path)
    };
    let mut roots = lexical_platform_aliases(&lexical_root);
    if let Ok(canonical_root) = config.output_dir.canonicalize() {
        for root in lexical_platform_aliases(&canonical_root) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let mut candidates = lexical_platform_aliases(&lexical);
    if let Ok(canonical) = lexical.canonicalize() {
        for candidate in lexical_platform_aliases(&canonical) {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    for root in &roots {
        for candidate in &candidates {
            let Ok(relative) = candidate.strip_prefix(root) else {
                continue;
            };
            if active_corpus_relative_path(relative)
                && !relative_path_has_unsafe_component(root, relative)
            {
                return true;
            }
        }
    }
    false
}

fn relative_path_has_unsafe_component(root: &Path, relative: &Path) -> bool {
    let components: Vec<_> = relative.components().collect();
    let mut current = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(component) = component else {
            return true;
        };
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => return true,
            Ok(_) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && index + 1 == components.len() =>
            {
                // A just-deleted leaf remains a valid retraction identity.
            }
            Err(_) => return true,
        }
    }
    false
}

fn acquire_policy_lock(
    name: &str,
) -> Result<crate::policy_fs::BoundRecoveryLeaseFile, Box<dyn std::error::Error>> {
    acquire_policy_lock_at(&Config::minutes_dir(), name)
}

fn acquire_policy_lock_at(
    directory: &Path,
    name: &str,
) -> Result<crate::policy_fs::BoundRecoveryLeaseFile, Box<dyn std::error::Error>> {
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(directory)?;
    let lock = boundary.bind_or_create_private_lease_file(OsStr::new(name))?;
    lock.lock_exclusive()?;
    Ok(lock)
}

fn acquire_policy_lock_at_until(
    directory: &Path,
    name: &str,
    deadline: Instant,
) -> Result<crate::policy_fs::BoundRecoveryLeaseFile, Box<dyn std::error::Error>> {
    let deadline_error = || {
        std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "policy lock exceeded its operation deadline",
        )
    };
    if Instant::now() >= deadline {
        return Err(deadline_error().into());
    }
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(directory)?;
    let lock = boundary.bind_or_create_private_lease_file(OsStr::new(name))?;
    loop {
        if Instant::now() >= deadline {
            return Err(deadline_error().into());
        }
        match lock.try_lock_exclusive() {
            Ok(true) if Instant::now() < deadline => return Ok(lock),
            Ok(true) => return Err(deadline_error().into()),
            Ok(false) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(deadline_error().into());
                }
                std::thread::sleep(remaining.min(Duration::from_millis(10)));
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub fn acquire_qmd_policy_lock(
) -> Result<crate::policy_fs::BoundRecoveryLeaseFile, Box<dyn std::error::Error>> {
    acquire_policy_lock(QMD_POLICY_LOCK)
}

fn canonical_output_root(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(config.output_dir.canonicalize()?)
}

/// Process-local keyed label for diagnostics. Meeting filenames and paths can
/// themselves be sensitive and must not enter background logs. RandomState's
/// per-process key prevents an offline filename dictionary from reproducing
/// these labels.
pub fn privacy_safe_source_scope(path: &Path) -> String {
    static HASHER: OnceLock<RandomState> = OnceLock::new();
    format!(
        "src-{:016x}",
        HASHER
            .get_or_init(RandomState::new)
            .hash_one(path.as_os_str())
    )
}

fn open_regular_file_no_follow(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err("not a regular file".into());
    }
    Ok(file)
}

fn open_regular_file_no_follow_for_update(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options
            // Every caller updates a private journal/control file that may be
            // renamed by its exact handle after recovery. FileRenameInformation
            // requires DELETE access on that retained source handle.
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err("not a regular file".into());
    }
    Ok(file)
}

fn policy_reason(error: &str) -> &'static str {
    if error.contains("restricted") {
        "restricted"
    } else if error.contains("symbolic link") || error.contains("outside corpus") {
        "unsafe-path"
    } else if error.contains("frontmatter") || error.contains("YAML") {
        "invalid-frontmatter"
    } else if error.contains("changed during read") {
        "unstable-source"
    } else {
        "unreadable-or-invalid"
    }
}

/// Read and classify one meeting from the configured corpus. Any ambiguity is
/// denied: links, escapes, malformed YAML, and restricted sources are not a
/// valid input to a persistent derivative.
fn authorized_meeting(
    path: &Path,
    config: &Config,
) -> Result<AuthorizedMeeting, Box<dyn std::error::Error>> {
    authorized_meeting_with_hook(path, config, || {})
}

fn authorized_meeting_with_hook<F>(
    path: &Path,
    config: &Config,
    after_first_read: F,
) -> Result<AuthorizedMeeting, Box<dyn std::error::Error>>
where
    F: FnOnce(),
{
    let scope = privacy_safe_source_scope(path);
    if path.extension().is_none_or(|extension| extension != "md") {
        return Err(format!("policy-ineligible meeting ({scope}): not Markdown").into());
    }
    let root = canonical_output_root(config)?;
    let snapshot =
        crate::policy_fs::read_bound_utf8_file_with_hooks(&root, path, || {}, after_first_read)
            .map_err(|_| format!("policy-ineligible meeting ({scope}): changed during read"))?;
    if !is_live_corpus_path(&snapshot.canonical_path, config) {
        return Err(format!("policy-ineligible meeting ({scope}): inactive or unsafe path").into());
    }

    let (frontmatter_str, _body) = crate::markdown::split_frontmatter(&snapshot.content);
    if frontmatter_str.is_empty() {
        return Err(format!("policy-ineligible meeting ({scope}): missing frontmatter").into());
    }
    let frontmatter: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .map_err(|_| format!("policy-ineligible meeting ({scope}): invalid frontmatter"))?;
    if matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) {
        return Err(format!("policy-ineligible meeting ({scope}): restricted").into());
    }
    Ok(AuthorizedMeeting {
        path: snapshot.canonical_path,
        content: snapshot.content,
        frontmatter,
    })
}

fn source_keys_for_path(path: &Path, config: &Config) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
        // Compatibility key for facts written before policy reconciliation.
        keys.push(stem.to_string());
    }

    let lexical_root = if config.output_dir.is_absolute() {
        config.output_dir.clone()
    } else {
        std::env::current_dir()
            .map(|current| current.join(&config.output_dir))
            .unwrap_or_else(|_| config.output_dir.clone())
    };
    let mut roots = lexical_platform_aliases(&lexical_root);
    if let Ok(canonical_root) = config.output_dir.canonicalize() {
        for root in lexical_platform_aliases(&canonical_root) {
            if !roots.contains(&root) {
                roots.push(root);
            }
        }
    }
    let lexical_candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        lexical_root.join(path)
    };
    let canonical_candidate = path.canonicalize().ok();
    let mut candidates = lexical_platform_aliases(&lexical_candidate);
    if let Some(canonical) = canonical_candidate {
        for candidate in lexical_platform_aliases(&canonical) {
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    for candidate in &candidates {
        for root in &roots {
            if let Ok(relative) = candidate.strip_prefix(root) {
                if !active_corpus_relative_path(relative) {
                    continue;
                }
                let mut relative = relative.to_path_buf();
                relative.set_extension("");
                let key = format!(
                    "v2:{}",
                    encode_provenance_key(&relative.to_string_lossy().replace('\\', "/"))
                );
                if !key.is_empty() && !keys.contains(&key) {
                    keys.push(key);
                }
            }
        }
    }
    keys
}

fn exact_source_key(path: &Path, config: &Config) -> Option<String> {
    source_keys_for_path(path, config)
        .into_iter()
        .rev()
        .find(|key| key.starts_with("v2:"))
}

fn legacy_source_key(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

fn lexically_contained_active_path(path: &Path, config: &Config) -> bool {
    is_live_corpus_path(path, config)
}

fn encode_provenance_key(value: &str) -> String {
    let mut encoded = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(*byte, b'.' | b'_' | b'-' | b'/') {
            encoded.push(char::from(*byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn content_revision(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in content.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn record_id(kind: &str, content: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(content.as_bytes());
    format!("{kind}:{:x}", digest.finalize())
}

fn source_revision(content: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"minutes-source-revision\0");
    digest.update(content.as_bytes());
    format!("sha256:{:x}", digest.finalize())
}

fn wiki_fact_record(fact: &Fact) -> String {
    format!(
        "- {} *({}; {} — {})*",
        single_line(&fact.text),
        fact.confidence.as_str(),
        fact.source_date,
        fact.source_meeting,
    )
}

fn para_fact_record(slug: &str, fact: &Fact) -> serde_json::Value {
    let provenance_identity = format!("{}\0{}", fact.text, fact.source_meeting);
    let id = format!("{}-{:x}", slug, hash_fact(&provenance_identity));
    serde_json::json!({
        "id": id,
        "fact": fact.text,
        "category": fact.category,
        "confidence": fact.confidence.as_str(),
        "timestamp": fact.source_date,
        "source": fact.source_meeting,
        "status": "active",
        "supersededBy": null,
    })
}

fn render_log_section(entry: &LogEntry) -> String {
    let people = if entry.people_updated.is_empty() {
        "no people updated".to_string()
    } else {
        entry.people_updated.join(", ")
    };
    format!(
        "## [{}] ingest | {}\n\n- Source: `{}`\n- Facts written: {}, skipped: {}\n- People: {}\n\n",
        entry.date.format("%Y-%m-%d %H:%M"),
        single_line(&entry.meeting_title),
        single_line(&entry.meeting_path),
        entry.fact_count,
        entry.skipped_count,
        people,
    )
}

fn generated_record_ids(
    people: &[PersonFacts],
    min_confidence: Confidence,
    adapter: &str,
    log: &LogEntry,
) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let mut records = BTreeSet::new();
    for person in people {
        for fact in person
            .facts
            .iter()
            .filter(|fact| fact.confidence.meets(min_confidence))
        {
            if adapter.eq_ignore_ascii_case("para") {
                records.insert(record_id(
                    "para",
                    &serde_json::to_string(&para_fact_record(&person.slug, fact))?,
                ));
            } else {
                records.insert(record_id("wiki", &wiki_fact_record(fact)));
            }
        }
    }
    let rendered = render_log_section(log);
    let section = rendered
        .strip_prefix("## [")
        .unwrap_or(&rendered)
        .trim_end();
    records.insert(record_id("log", section));
    Ok(records)
}

fn absolute_lexical(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn open_preservation_root(config: &Config) -> Result<(PathBuf, File), Box<dyn std::error::Error>> {
    let knowledge = absolute_lexical(&config.knowledge.path);
    let knowledge_canonical = config.knowledge.path.canonicalize().ok();
    let output = absolute_lexical(&config.output_dir);
    let output_canonical = config.output_dir.canonicalize().ok();
    let config_parent = Config::config_path()
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(Config::minutes_dir);
    let knowledge_sibling = knowledge
        .parent()
        .map(|parent| parent.join(".minutes-preserved-knowledge"));
    #[cfg(test)]
    let candidates: Vec<PathBuf> = knowledge_sibling
        .into_iter()
        .chain([
            Config::minutes_dir().join("preserved-knowledge"),
            config_parent.join("preserved-knowledge"),
        ])
        .collect();
    #[cfg(not(test))]
    let candidates: Vec<PathBuf> = [
        Config::minutes_dir().join("preserved-knowledge"),
        config_parent.join("preserved-knowledge"),
    ]
    .into_iter()
    .chain(knowledge_sibling)
    .collect();
    for candidate in candidates {
        let candidate = absolute_lexical(&candidate);
        if candidate.starts_with(&knowledge) || candidate.starts_with(&output) {
            continue;
        }
        let directory = open_private_dir_no_follow(&candidate)?;
        let before = candidate.canonicalize()?;
        let after = candidate.canonicalize()?;
        if before != after
            || !file_identity_matches_path(&directory, &candidate)
            || knowledge_canonical
                .as_ref()
                .is_some_and(|root| before.starts_with(root))
            || output_canonical
                .as_ref()
                .is_some_and(|root| before.starts_with(root))
        {
            continue;
        }
        return Ok((candidate, directory));
    }
    Err("no preservation store exists outside the configured knowledge derivative".into())
}

#[cfg(test)]
fn preservation_root(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    open_preservation_root(config).map(|(path, _directory)| path)
}

#[cfg(test)]
fn preservation_namespace(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(preservation_root(config)?.join(format!(
        "kb-{:016x}",
        content_revision(&absolute_lexical(&config.knowledge.path).to_string_lossy())
    )))
}

fn open_preservation_namespace(
    config: &Config,
) -> Result<(PathBuf, File, File), Box<dyn std::error::Error>> {
    open_preservation_namespace_with(config, |_| {})
}

fn open_preservation_namespace_with<F>(
    config: &Config,
    mut after_root_open: F,
) -> Result<(PathBuf, File, File), Box<dyn std::error::Error>>
where
    F: FnMut(&Path),
{
    // Candidate selection returns the exact validated descriptor. Never reopen
    // this pathname between validation and namespace creation.
    let (root, root_directory) = open_preservation_root(config)?;
    after_root_open(&root);
    let name = format!(
        "kb-{:016x}",
        content_revision(&absolute_lexical(&config.knowledge.path).to_string_lossy())
    );
    let namespace = root.join(&name);
    let namespace_directory = open_private_child_dir_at(&root_directory, &root, &name)?;
    if !file_identity_matches_path(&root_directory, &root)
        || !file_identity_matches_path(&namespace_directory, &namespace)
    {
        return Err("preservation namespace changed during validation".into());
    }
    Ok((namespace, namespace_directory, root_directory))
}

fn file_identity_matches_path(file: &File, path: &Path) -> bool {
    let Ok(path_metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if path_metadata.file_type().is_symlink() || !path_metadata.is_dir() {
        return false;
    }
    #[cfg(not(windows))]
    let Ok(file_metadata) = file.metadata() else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        file_metadata.dev() == path_metadata.dev() && file_metadata.ino() == path_metadata.ino()
    }
    #[cfg(windows)]
    {
        windows_private::same_directory_identity(file, path)
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        file_metadata.len() == path_metadata.len()
            && file_metadata.modified().ok() == path_metadata.modified().ok()
    }
}

#[derive(Debug)]
struct PreservedIdentity {
    bytes: Vec<u8>,
    len: u64,
    modified: Option<std::time::SystemTime>,
    symlink: bool,
    object_identity: Option<QmdObjectIdentity>,
    links: u64,
    _durable_directory: Option<File>,
    _durable_root_directory: Option<File>,
}

fn preserved_source_identity(path: &Path) -> Result<PreservedIdentity, Box<dyn std::error::Error>> {
    preserved_source_identity_with_limit(path, MAX_RETAINED_RECONCILIATION_BYTES)
}

fn preserved_source_identity_with_limit(
    path: &Path,
    max_bytes: u64,
) -> Result<PreservedIdentity, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(path)?;
    let symlink = metadata.file_type().is_symlink();
    let (bytes, bound_metadata, object_identity, links) = if symlink {
        let target = fs::read_link(path)?;
        let mut bytes = b"MINUTES-PRESERVED-SYMLINK\n".to_vec();
        bytes.extend_from_slice(target.as_os_str().as_encoded_bytes());
        let after = fs::symlink_metadata(path)?;
        if !after.file_type().is_symlink()
            || after.len() != metadata.len()
            || after.modified().ok() != metadata.modified().ok()
        {
            return Err("knowledge symlink changed while being preserved".into());
        }
        #[cfg(unix)]
        let object_identity = {
            use std::os::unix::fs::MetadataExt;
            if after.dev() != metadata.dev() || after.ino() != metadata.ino() {
                return Err("knowledge symlink changed while being preserved".into());
            }
            Some(QmdObjectIdentity {
                scope: metadata.dev(),
                object: metadata.ino(),
            })
        };
        #[cfg(not(unix))]
        let object_identity = None;
        #[cfg(unix)]
        let links = {
            use std::os::unix::fs::MetadataExt;
            metadata.nlink()
        };
        #[cfg(not(unix))]
        let links = 1;
        (bytes, metadata, object_identity, links)
    } else if metadata.is_file() {
        let mut file = open_regular_file_no_follow(path)?;
        let opened_before = file.metadata()?;
        if opened_before.len() > max_bytes {
            return Err("knowledge object exceeds the bounded preservation size".into());
        }
        let (object_identity, links) =
            qmd_file_identity_and_links(&file).ok_or("knowledge object identity is unavailable")?;
        let capacity = usize::try_from(opened_before.len())
            .map_err(|_| "knowledge object exceeds the addressable preservation size")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| "knowledge object preservation allocation failed")?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            let projected = bytes
                .len()
                .checked_add(count)
                .ok_or("knowledge object preservation length overflowed")?;
            if projected > capacity {
                return Err("knowledge object changed while being preserved".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        let opened_after = file.metadata()?;
        let path_file = open_regular_file_no_follow(path)?;
        if opened_before.len() != opened_after.len()
            || u64::try_from(bytes.len()).ok() != Some(opened_before.len())
            || opened_before.modified().ok() != opened_after.modified().ok()
            || !qmd_file_handles_match(&file, &path_file)
            || qmd_file_identity_and_links(&file).map(|(identity, _links)| identity)
                != Some(object_identity)
        {
            return Err("knowledge object changed while being preserved".into());
        }
        (bytes, opened_after, Some(object_identity), links)
    } else {
        return Err("cannot preserve a non-file knowledge object".into());
    };
    Ok(PreservedIdentity {
        bytes,
        len: bound_metadata.len(),
        modified: bound_metadata.modified().ok(),
        symlink,
        object_identity,
        links,
        _durable_directory: None,
        _durable_root_directory: None,
    })
}

fn preserved_identity_matches(path: &Path, expected: &PreservedIdentity) -> bool {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return false;
    };
    if expected.symlink {
        if !metadata.file_type().is_symlink()
            || metadata.len() != expected.len
            || metadata.modified().ok() != expected.modified
        {
            return false;
        }
        let Ok(target) = fs::read_link(path) else {
            return false;
        };
        let Some(expected_target) = expected.bytes.strip_prefix(b"MINUTES-PRESERVED-SYMLINK\n")
        else {
            return false;
        };
        let after = match fs::symlink_metadata(path) {
            Ok(after) => after,
            Err(_) => return false,
        };
        if !after.file_type().is_symlink()
            || after.len() != metadata.len()
            || after.modified().ok() != metadata.modified().ok()
            || target.as_os_str().as_encoded_bytes() != expected_target
        {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            return expected.links == metadata.nlink()
                && expected.object_identity
                    == Some(QmdObjectIdentity {
                        scope: metadata.dev(),
                        object: metadata.ino(),
                    })
                && metadata.dev() == after.dev()
                && metadata.ino() == after.ino();
        }
        #[cfg(not(unix))]
        {
            return expected.links == 1 && expected.object_identity.is_none();
        }
    }

    let Ok(file) = open_regular_file_no_follow(path) else {
        return false;
    };
    let Ok(opened) = file.metadata() else {
        return false;
    };
    let Some((identity, links)) = qmd_file_identity_and_links(&file) else {
        return false;
    };
    if !opened.is_file()
        || opened.len() != expected.len
        || opened.modified().ok() != expected.modified
        || Some(identity) != expected.object_identity
        || links != expected.links
        || !exact_capture_matches_expected_bytes(&file, &expected.bytes).unwrap_or(false)
    {
        return false;
    }
    open_regular_file_no_follow(path).is_ok_and(|visible| {
        qmd_file_handles_match(&file, &visible)
            && qmd_file_identity_and_links(&visible) == Some((identity, links))
    })
}

fn exact_capture_matches_expected_bytes(file: &File, expected: &[u8]) -> std::io::Result<bool> {
    let expected_len = u64::try_from(expected.len())
        .map_err(|_| std::io::Error::other("knowledge capture length overflowed"))?;
    if expected_len > MAX_RETAINED_RECONCILIATION_BYTES {
        return Ok(false);
    }
    let before = file.metadata()?;
    if !before.is_file() || before.len() != expected_len {
        return Ok(false);
    }

    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut offset = 0usize;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let end = offset
            .checked_add(count)
            .ok_or_else(|| std::io::Error::other("knowledge capture length overflowed"))?;
        if end > expected.len() || buffer[..count] != expected[offset..end] {
            return Ok(false);
        }
        offset = end;
    }
    let after = file.metadata()?;
    Ok(after.is_file() && after.len() == expected_len && offset == expected.len())
}

fn attest_visible_exact_file(
    path: &Path,
    file: &File,
    expected: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    if !exact_capture_matches_expected_bytes(file, expected)? {
        return Err("knowledge publication content changed before final attestation".into());
    }
    let (identity, links) =
        qmd_file_identity_and_links(file).ok_or("knowledge publication identity is unavailable")?;
    let visible = open_regular_file_no_follow(path)?;
    let (visible_identity, visible_links) = qmd_file_identity_and_links(&visible)
        .ok_or("knowledge publication identity is unavailable")?;
    if links != 1
        || visible_links != 1
        || identity != visible_identity
        || !qmd_file_handles_match(file, &visible)
    {
        return Err("knowledge publication name or links changed before final attestation".into());
    }
    if !exact_capture_matches_expected_bytes(&visible, expected)? {
        return Err("knowledge publication content changed at the visible-name boundary".into());
    }
    let final_visible = open_regular_file_no_follow(path)?;
    if !qmd_file_handles_match(file, &final_visible)
        || qmd_file_identity_and_links(file) != Some((identity, 1))
        || qmd_file_identity_and_links(&final_visible) != Some((identity, 1))
        || !exact_capture_matches_expected_bytes(&final_visible, expected)?
    {
        return Err(
            "knowledge publication content, name, or links changed at final attestation".into(),
        );
    }
    Ok(())
}

fn attest_path_is_absent(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(_) => {
            Err("knowledge deletion destination was repopulated before final attestation".into())
        }
        Err(error) => Err(error.into()),
    }
}

fn open_exact_capture_for_retirement(
    path: &Path,
    expected: &PreservedIdentity,
) -> Result<Option<File>, Box<dyn std::error::Error>> {
    if expected.symlink {
        return Ok(None);
    }
    if expected.links != 1 {
        return Err("knowledge object has multiple links; refusing destructive mutation".into());
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    let (object_identity, links) =
        qmd_file_identity_and_links(&file).ok_or("knowledge capture identity is unavailable")?;
    let exact_bytes = exact_capture_matches_expected_bytes(&file, &expected.bytes)?;
    if !metadata.is_file()
        || metadata.len() != expected.len
        || metadata.modified().ok() != expected.modified
        || Some(object_identity) != expected.object_identity
        || links != expected.links
        || !exact_bytes
    {
        return Err("knowledge capture changed before exact retirement".into());
    }
    Ok(Some(file))
}

#[cfg(test)]
fn attest_retained_capture_with_hook<F>(
    path: &Path,
    file: Option<&mut File>,
    expected: &PreservedIdentity,
    after_initial_proof: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    attest_retained_capture_with_successor_proof(
        path,
        file,
        expected,
        after_initial_proof,
        |_| Ok(()),
        || Ok(()),
    )
}

#[cfg(test)]
fn attest_retained_capture_with_hooks<F, G>(
    path: &Path,
    file: Option<&mut File>,
    expected: &PreservedIdentity,
    after_initial_proof: F,
    after_final_content_proof: G,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
    G: FnOnce(&Path) -> std::io::Result<()>,
{
    attest_retained_capture_with_successor_proof(
        path,
        file,
        expected,
        after_initial_proof,
        after_final_content_proof,
        || Ok(()),
    )
}

fn attest_retained_capture_with_successor_proof<F, G, S>(
    path: &Path,
    file: Option<&mut File>,
    expected: &PreservedIdentity,
    after_initial_proof: F,
    after_final_content_proof: G,
    mut attest_successor: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
    G: FnOnce(&Path) -> std::io::Result<()>,
    S: FnMut() -> Result<(), Box<dyn std::error::Error>>,
{
    if let Some(file) = file {
        let metadata = file.metadata()?;
        let (identity, links) = qmd_file_identity_and_links(file)
            .ok_or_else(|| std::io::Error::other("knowledge capture identity is unavailable"))?;
        let visible = open_regular_file_no_follow(path).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "knowledge capture name changed before exact retirement",
            )
        })?;
        let visible_metadata = visible.metadata()?;
        let (visible_identity, visible_links) = qmd_file_identity_and_links(&visible)
            .ok_or_else(|| std::io::Error::other("knowledge capture identity is unavailable"))?;
        let exact_bytes = exact_capture_matches_expected_bytes(file, &expected.bytes)?;
        if !metadata.is_file()
            || !visible_metadata.is_file()
            || links != 1
            || visible_links != 1
            || identity != visible_identity
            || !qmd_file_handles_match(file, &visible)
            || Some(identity) != expected.object_identity
            || metadata.len() != expected.len
            || !exact_bytes
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "knowledge capture name, links, or content changed before exact retirement",
            )
            .into());
        }
        after_initial_proof(path)?;
        after_final_content_proof(path)?;
        // Successor attestation may be arbitrarily slow and may execute
        // platform-specific I/O. Complete it before the final old-generation
        // proof so no fallible or long-running operation separates that proof
        // from truncation.
        attest_successor()?;
        // Final proof is still useful for detecting a concurrent name/content
        // change, but it does not authorize in-place destruction. On POSIX a
        // same-UID process can create a hard link after any nlink proof and
        // before ftruncate. Retaining the byte-bearing object in the bounded,
        // owner-private capture is the only portable non-destructive outcome.
        attest_visible_exact_file(path, file, &expected.bytes).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "knowledge capture changed at the exact retirement boundary/content boundary",
            )
        })?;
    }
    Ok(())
}

fn exact_reconciliation_zero_tombstone(path: &Path) -> Option<File> {
    exact_reconciliation_zero_tombstone_with_hook(path, |_| Ok(()))
}

fn exact_reconciliation_zero_tombstone_with_hook<F>(
    path: &Path,
    after_retained_checks: F,
) -> Option<File>
where
    F: FnOnce(&Path) -> std::io::Result<()>,
{
    let path_metadata = fs::symlink_metadata(path).ok()?;
    if !path_metadata.is_file() || path_metadata.len() != 0 {
        return None;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = options.open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let (_identity, links) = qmd_file_identity_and_links(&file)?;
    if !metadata.is_file() || metadata.len() != 0 || links != 1 {
        return None;
    }
    let visible = open_regular_file_no_follow(path).ok()?;
    if !qmd_file_handles_match(&file, &visible) {
        return None;
    }
    let final_metadata = file.metadata().ok()?;
    let (identity, final_links) = qmd_file_identity_and_links(&file)?;
    if !final_metadata.is_file() || final_metadata.len() != 0 || final_links != 1 {
        return None;
    }
    after_retained_checks(path).ok()?;
    let final_visible = open_regular_file_no_follow(path).ok()?;
    let final_visible_metadata = final_visible.metadata().ok()?;
    let (final_visible_identity, final_visible_links) =
        qmd_file_identity_and_links(&final_visible)?;
    (final_visible_metadata.is_file()
        && final_visible_metadata.len() == 0
        && final_visible_links == 1
        && final_visible_identity == identity
        && qmd_file_handles_match(&file, &final_visible))
    .then_some(file)
}

fn require_retained_slot_budget(
    directory: &Path,
    prefix: &str,
    additional_bytes: u64,
    max_entries: usize,
    max_bytes: u64,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = 0usize;
    let mut bytes = 0u64;
    let mut inert_tombstones = 0usize;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let complete_capture_namespace = prefix == "capture-";
        if !complete_capture_namespace && !entry.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if complete_capture_namespace
            && exact_reconciliation_zero_tombstone(&entry.path()).is_some()
        {
            // A successful reconciliation zeroes the exact captured file
            // through its already-open handle. POSIX has no unlink-by-open-file
            // identity primitive: unlinking this pathname after a final check
            // would let a same-UID writer swap in a winner and have that winner
            // deleted. Keep the zero name as inert metadata instead. It carries
            // no private bytes and therefore must not permanently consume the
            // fail-closed budget reserved for byte-bearing or ambiguous
            // captures. The no-follow writable open, single-link check, and
            // identity-confirmed reopen are required for this exemption. All
            // other entries in the complete private capture namespace count,
            // regardless of their mutable filename.
            inert_tombstones = inert_tombstones
                .saturating_add(1)
                .min(INERT_RECONCILIATION_TOMBSTONE_NOTICE_THRESHOLD);
            continue;
        }
        if complete_capture_namespace && metadata.is_dir() {
            // Descendant bytes are private residue too, but recursively walking
            // an attacker-mutable directory would introduce traversal races and
            // link-following hazards. Treat any directory as exhausting the
            // capture byte budget instead of undercounting only its metadata.
            return Err("retained reconciliation safety budget is exhausted".into());
        }
        entries = entries
            .checked_add(1)
            .ok_or("retained reconciliation entry count overflowed")?;
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or("retained reconciliation byte count overflowed")?;
    }
    if inert_tombstones >= INERT_RECONCILIATION_TOMBSTONE_NOTICE_THRESHOLD
        && !INERT_RECONCILIATION_TOMBSTONES_REPORTED.swap(true, Ordering::AcqRel)
    {
        tracing::warn!(
            inert_tombstones_at_least = INERT_RECONCILIATION_TOMBSTONE_NOTICE_THRESHOLD,
            "knowledge reconciliation has accumulated inert zero-byte tombstones; privacy cleanup remains unblocked"
        );
    }
    let projected_bytes = bytes
        .checked_add(additional_bytes)
        .ok_or("retained reconciliation byte budget overflowed")?;
    if entries >= max_entries || projected_bytes > max_bytes {
        return Err("retained reconciliation safety budget is exhausted".into());
    }
    Ok(())
}

/// Preserve exact user bytes before removing an unowned or malformed object
/// from an agent-visible derivative. Filenames are opaque and the private
/// store is deliberately outside the configured knowledge root.
fn preserve_file_before_retraction(
    config: &Config,
    path: &Path,
) -> Result<PreservedIdentity, Box<dyn std::error::Error>> {
    preserve_file_before_retraction_with_hook(config, path, |_| {})
}

fn preserve_file_before_retraction_with_hook<F>(
    config: &Config,
    path: &Path,
    before_create: F,
) -> Result<PreservedIdentity, Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
{
    let identity = preserved_source_identity(path)?;
    preserve_bound_file_before_retraction_with_hook(config, path, identity, before_create)
}

fn preserve_bound_file_before_retraction(
    config: &Config,
    path: &Path,
    identity: PreservedIdentity,
) -> Result<PreservedIdentity, Box<dyn std::error::Error>> {
    preserve_bound_file_before_retraction_with_hook(config, path, identity, |_| {})
}

fn preserve_bound_file_before_retraction_with_hook<F>(
    config: &Config,
    path: &Path,
    mut identity: PreservedIdentity,
    before_create: F,
) -> Result<PreservedIdentity, Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
{
    preserve_bound_file_before_retraction_in_place_with_hook(
        config,
        path,
        &mut identity,
        before_create,
    )?;
    Ok(identity)
}

fn preserve_bound_file_before_retraction_in_place(
    config: &Config,
    path: &Path,
    identity: &mut PreservedIdentity,
) -> Result<(), Box<dyn std::error::Error>> {
    preserve_bound_file_before_retraction_in_place_with_hook(config, path, identity, |_| {})
}

fn preserve_bound_file_before_retraction_in_place_with_hook<F>(
    config: &Config,
    path: &Path,
    identity: &mut PreservedIdentity,
    before_create: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
{
    let (namespace, namespace_directory, root_directory) = open_preservation_namespace(config)?;
    before_create(&namespace);
    let seed = format!(
        "{}:{}:{}",
        path.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    for attempt in 0..100u64 {
        let filename = format!(
            "preserved-{:016x}.bin",
            content_revision(&format!("{seed}:{attempt}"))
        );
        match create_new_private_file_at(
            &namespace_directory,
            &namespace,
            std::ffi::OsStr::new(&filename),
        ) {
            Ok(mut file) => {
                set_restrictive_permissions_file(&file)?;
                file.write_all(&identity.bytes)?;
                file.sync_all()?;
                #[cfg(unix)]
                namespace_directory.sync_all()?;
                // Windows FlushFileBuffers requires a GENERIC_WRITE file
                // handle and does not provide the Unix directory-fsync
                // contract. The new file handle above is synchronously opened
                // and flushed; retained root/namespace handles keep its name
                // capability stable through the final identity check.
                if !file_identity_matches_path(&namespace_directory, &namespace)
                    || !namespace
                        .parent()
                        .is_some_and(|root| file_identity_matches_path(&root_directory, root))
                {
                    return Err(
                        "preservation root or namespace changed before durability confirmation"
                            .into(),
                    );
                }
                identity._durable_directory = Some(namespace_directory);
                identity._durable_root_directory = Some(root_directory);
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate a private preservation record".into())
}

fn replace_preserved_file(
    config: &Config,
    path: &Path,
    identity: &PreservedIdentity,
    replacement: Option<&[u8]>,
) -> Result<(), Box<dyn std::error::Error>> {
    replace_preserved_file_with_hooks(config, path, identity, replacement, |_| {}, |_| {})
}

#[cfg(test)]
fn replace_preserved_file_with_hook<F>(
    config: &Config,
    path: &Path,
    identity: &PreservedIdentity,
    replacement: Option<&[u8]>,
    after_replacement_published: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
{
    replace_preserved_file_with_hooks(
        config,
        path,
        identity,
        replacement,
        after_replacement_published,
        |_| {},
    )
}

fn replace_preserved_file_with_hooks<F, G>(
    config: &Config,
    path: &Path,
    identity: &PreservedIdentity,
    replacement: Option<&[u8]>,
    after_replacement_published: F,
    after_initial_successor_attestation: G,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
    G: FnOnce(&Path),
{
    if replacement.is_some_and(|bytes| {
        u64::try_from(bytes.len())
            .map(|len| len > MAX_RETAINED_RECONCILIATION_BYTES)
            .unwrap_or(true)
    }) {
        return Err("knowledge replacement exceeds the bounded preservation size".into());
    }
    if !identity.symlink && identity.links != 1 {
        return Err("knowledge object has multiple links; refusing destructive mutation".into());
    }
    let parent = path.parent().ok_or("knowledge object has no parent")?;
    let staging = parent.join(".minutes-private-reconcile");
    create_private_dir_all_no_follow(&staging)?;
    require_retained_slot_budget(
        &staging,
        "capture-",
        identity.len,
        MAX_RETAINED_RECONCILIATION_CAPTURES,
        MAX_RETAINED_RECONCILIATION_BYTES,
    )?;
    let capture = staging.join(format!(
        "capture-{:016x}",
        content_revision(&format!(
            "{}:{}",
            path.to_string_lossy(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ))
    ));
    crate::policy_fs::move_entry_no_replace(path, &capture)?;
    if !preserved_identity_matches(&capture, identity) {
        // The name was swapped after preservation. Preserve the captured new
        // object too, then attempt only an atomic no-replace restoration. A
        // destination winner is never overwritten; an ambiguous staging entry
        // is never unlinked. The caller receives an error either way.
        let swapped = preserve_file_before_retraction(config, &capture)?;
        if crate::policy_fs::move_entry_no_replace(&capture, path).is_ok()
            && !preserved_identity_matches(path, &swapped)
        {
            // A staging-slot winner was moved instead of the captured object.
            // Keep it recoverable too; the transaction remains failed closed.
            let _unexpected = preserve_file_before_retraction(config, path)?;
        }
        return Err("knowledge object changed after preservation; refusing mutation".into());
    }
    let mut capture_file = open_exact_capture_for_retirement(&capture, identity)?;
    let replacement_file = if let Some(bytes) = replacement {
        match OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
        {
            Ok(mut file) => {
                set_restrictive_permissions_file(&file)?;
                file.write_all(bytes)?;
                file.sync_all()?;
                Some(file)
            }
            Err(error) => {
                // A concurrent writer won the original name. Do not overwrite
                // it or sanitize the exact captured original. A failed
                // replacement must leave the old generation recoverable.
                return Err(error.into());
            }
        }
    } else {
        None
    };
    after_replacement_published(&capture);
    if let (Some(bytes), Some(file)) = (replacement, replacement_file.as_ref()) {
        attest_visible_exact_file(path, file, bytes)?;
    } else {
        attest_path_is_absent(path)?;
    }
    after_initial_successor_attestation(path);
    attest_retained_capture_with_successor_proof(
        &capture,
        capture_file.as_mut(),
        identity,
        |_| Ok(()),
        |_| Ok(()),
        || {
            if let (Some(bytes), Some(file)) = (replacement, replacement_file.as_ref()) {
                attest_visible_exact_file(path, file, bytes)
            } else {
                attest_path_is_absent(path)
            }
        },
    )?;
    // The capture name is deliberately retained after successful exact
    // retention attestation. If it was detached or replaced, the proof above fails
    // and both the exact bytes and any same-UID winner remain untouched. There
    // is no portable conditional unlink that says "remove this name only if it
    // is still this exact file". The hidden owner-only staging directory is
    // outside every adapter's recognized derivative surface.
    Ok(())
}

fn remove_preserved_file(
    config: &Config,
    path: &Path,
    identity: &PreservedIdentity,
) -> Result<(), Box<dyn std::error::Error>> {
    replace_preserved_file(config, path, identity, None)
}

fn legacy_public_provenance_manifest_path(config: &Config) -> PathBuf {
    config
        .knowledge
        .path
        .join(LEGACY_PUBLIC_KNOWLEDGE_PROVENANCE_MANIFEST)
}

#[cfg(test)]
fn provenance_manifest_path(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let (namespace, _namespace_directory, _root_directory) = open_preservation_namespace(config)?;
    Ok(namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST))
}

fn load_provenance_manifest(config: &Config) -> (KnowledgeProvenanceManifest, bool) {
    let Ok((namespace_path, namespace_directory, _root_directory)) =
        open_preservation_namespace(config)
    else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    #[cfg(windows)]
    if recover_windows_provenance_publication(&namespace_path).is_err() {
        return (KnowledgeProvenanceManifest::default(), false);
    }
    #[cfg(not(windows))]
    let _ = namespace_path;
    let namespace = CapDir::from_std_file(namespace_directory);
    let name = OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST);
    match namespace.symlink_metadata(name) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return (KnowledgeProvenanceManifest::default(), true)
        }
        _ => return (KnowledgeProvenanceManifest::default(), false),
    }
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let Ok(file) = namespace
        .open_with(name, &options)
        .map(|file| file.into_std())
    else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    let Ok(metadata) = file.metadata() else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    if metadata.len() > MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES {
        return (KnowledgeProvenanceManifest::default(), false);
    }
    let Ok(capacity) = usize::try_from(metadata.len()) else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    let read_exact = |file: &File| -> Result<String, Box<dyn std::error::Error>> {
        let mut raw = String::new();
        raw.try_reserve_exact(capacity)
            .map_err(|_| "knowledge provenance allocation failed")?;
        let mut reader = file.try_clone()?;
        reader.seek(SeekFrom::Start(0))?;
        reader
            .take(MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES + 1)
            .read_to_string(&mut raw)?;
        if u64::try_from(raw.len()).ok() != Some(metadata.len()) {
            return Err("knowledge provenance manifest changed during bounded read".into());
        }
        Ok(raw)
    };
    let Ok(raw) = read_exact(&file) else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    let Ok(confirmed_raw) = read_exact(&file) else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    let Ok(confirmed_file) = namespace
        .open_with(name, &options)
        .map(|file| file.into_std())
    else {
        return (KnowledgeProvenanceManifest::default(), false);
    };
    if raw != confirmed_raw
        || !qmd_file_handles_match(&file, &confirmed_file)
        || file.metadata().ok().map(|metadata| metadata.len()) != Some(metadata.len())
    {
        return (KnowledgeProvenanceManifest::default(), false);
    }
    match serde_json::from_str::<KnowledgeProvenanceManifest>(&raw) {
        Ok(manifest) if manifest.schema == 4 => (manifest, true),
        _ => match serde_json::from_str::<LegacyKnowledgeProvenanceManifestV3>(&raw) {
            Ok(legacy) if legacy.schema == 3 => {
                // v3 used non-cryptographic numeric content revisions. They cannot
                // authorize a persistent derivative under the v4 policy. Preserve
                // its exact record ownership so reconciliation can retract only
                // Minutes-owned records, then persist a clean v4 manifest. Live
                // sources are regenerated by their next normal ingestion.
                let _legacy_source_count = legacy.sources.len();
                (
                    KnowledgeProvenanceManifest {
                        schema: 4,
                        sources: BTreeMap::new(),
                        records: legacy.records,
                        managed_logs: BTreeSet::new(),
                    },
                    true,
                )
            }
            _ => (KnowledgeProvenanceManifest::default(), false),
        },
    }
}

#[cfg(not(windows))]
fn publish_fixed_provenance_temp(namespace_cap: &CapDir, _namespace: &Path) -> std::io::Result<()> {
    namespace_cap.rename(
        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
        namespace_cap,
        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
    )
}

#[cfg(any(windows, test))]
fn classify_windows_provenance_active_layout(
    previous: Option<&WindowsProvenanceFileProof>,
    target: WindowsProvenanceObservedFile,
    temporary: WindowsProvenanceObservedFile,
    backup: WindowsProvenanceObservedFile,
) -> WindowsProvenanceActiveLayout {
    let backup_available = matches!(
        backup,
        WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty
    );
    if previous.is_some() {
        match (target, temporary, backup) {
            (
                WindowsProvenanceObservedFile::Previous,
                WindowsProvenanceObservedFile::Intended,
                WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty,
            ) => WindowsProvenanceActiveLayout::PreMutation,
            (
                WindowsProvenanceObservedFile::Absent,
                WindowsProvenanceObservedFile::Intended,
                WindowsProvenanceObservedFile::Previous,
            ) => WindowsProvenanceActiveLayout::PreviousParked,
            (
                WindowsProvenanceObservedFile::Intended,
                WindowsProvenanceObservedFile::Absent,
                WindowsProvenanceObservedFile::Previous,
            ) => WindowsProvenanceActiveLayout::PublishedWithPrevious,
            (WindowsProvenanceObservedFile::Intended, WindowsProvenanceObservedFile::Absent, _)
                if backup_available =>
            {
                WindowsProvenanceActiveLayout::PublishedRetired
            }
            _ => WindowsProvenanceActiveLayout::Unknown,
        }
    } else {
        match (target, temporary) {
            (WindowsProvenanceObservedFile::Absent, WindowsProvenanceObservedFile::Intended)
                if backup_available =>
            {
                WindowsProvenanceActiveLayout::PreMutation
            }
            (WindowsProvenanceObservedFile::Intended, WindowsProvenanceObservedFile::Absent)
                if backup_available =>
            {
                WindowsProvenanceActiveLayout::PublishedRetired
            }
            _ => WindowsProvenanceActiveLayout::Unknown,
        }
    }
}

#[cfg(any(windows, test))]
fn windows_provenance_terminal_layout_is_exact(
    published: Option<&WindowsProvenanceFileProof>,
    target: WindowsProvenanceObservedFile,
    temporary: WindowsProvenanceObservedFile,
    backup: WindowsProvenanceObservedFile,
) -> bool {
    let target_matches = if published.is_some() {
        target == WindowsProvenanceObservedFile::Intended
    } else {
        target == WindowsProvenanceObservedFile::Absent
    };
    target_matches
        && matches!(
            temporary,
            WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty
        )
        && matches!(
            backup,
            WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty
        )
}

#[cfg(any(windows, test))]
fn windows_provenance_terminal_can_abort_unjournaled_temp(
    published: Option<&WindowsProvenanceFileProof>,
    target: WindowsProvenanceObservedFile,
    backup: WindowsProvenanceObservedFile,
) -> bool {
    let target_matches = if published.is_some() {
        target == WindowsProvenanceObservedFile::Intended
    } else {
        target == WindowsProvenanceObservedFile::Absent
    };
    target_matches
        && matches!(
            backup,
            WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty
        )
}

#[cfg(any(windows, test))]
fn select_latest_windows_provenance_record(
    records: &[(usize, WindowsProvenanceJournalRecord)],
) -> Result<Option<(usize, WindowsProvenanceJournalRecord)>, &'static str> {
    let mut sequences = BTreeSet::new();
    if records
        .iter()
        .any(|(_, record)| !sequences.insert(record.sequence))
    {
        return Err("private provenance journals contain duplicate sequences");
    }
    Ok(records
        .iter()
        .max_by_key(|(_, record)| record.sequence)
        .cloned())
}

#[cfg(any(windows, test))]
fn windows_provenance_file_proof(
    file: &crate::policy_fs::BoundRecoveryFile,
    max_bytes: u64,
) -> Result<WindowsProvenanceFileProof, Box<dyn std::error::Error>> {
    file.attest_visible_identity()?;
    let len = file.len()?;
    if len > max_bytes {
        return Err("private provenance control exceeds its fixed byte budget".into());
    }
    let capacity = usize::try_from(len)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(capacity)
        .map_err(|_| "private provenance proof allocation failed")?;
    let mut exact = file.try_clone_exact_file()?;
    let identity = qmd_file_identity_and_links(&exact)
        .map(|value| value.0)
        .ok_or("private provenance control has no stable exact identity")?;
    exact.seek(SeekFrom::Start(0))?;
    exact.take(max_bytes + 1).read_to_end(&mut bytes)?;
    if u64::try_from(bytes.len())? != len {
        return Err("private provenance control changed during its bounded proof".into());
    }
    file.recovery_proof_for_exact_bytes_bounded(
        &bytes,
        max_bytes,
        Instant::now() + Duration::from_secs(5),
    )?;
    Ok(WindowsProvenanceFileProof {
        identity,
        len,
        sha256: Sha256::digest(&bytes).into(),
    })
}

#[cfg(any(windows, test))]
fn inspect_windows_provenance_file(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    name: &OsStr,
    previous: Option<&WindowsProvenanceFileProof>,
    intended: Option<&WindowsProvenanceFileProof>,
    max_bytes: u64,
) -> Result<WindowsProvenanceObservation, Box<dyn std::error::Error>> {
    let file = match directory.bind_owner_private_exact_file(name) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WindowsProvenanceObservation {
                state: WindowsProvenanceObservedFile::Absent,
                proof: None,
            })
        }
        Err(error) => return Err(error.into()),
    };
    let proof = windows_provenance_file_proof(&file, max_bytes)?;
    let state = if proof.len == 0 {
        WindowsProvenanceObservedFile::Empty
    } else if intended == Some(&proof) {
        WindowsProvenanceObservedFile::Intended
    } else if previous == Some(&proof) {
        WindowsProvenanceObservedFile::Previous
    } else {
        WindowsProvenanceObservedFile::Other
    };
    Ok(WindowsProvenanceObservation {
        state,
        proof: Some(proof),
    })
}

#[cfg(any(windows, test))]
fn windows_current_provenance_proof(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
) -> Result<Option<WindowsProvenanceFileProof>, Box<dyn std::error::Error>> {
    match directory.bind_owner_private_exact_file(OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST))
    {
        Ok(file) => {
            if file.is_empty()? {
                return Err("private provenance manifest is unexpectedly empty".into());
            }
            Ok(Some(windows_provenance_file_proof(
                &file,
                MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
            )?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(any(windows, test))]
enum WindowsProvenanceJournalRead {
    Empty(WindowsProvenanceFileProof),
    Valid(WindowsProvenanceJournalRecord, WindowsProvenanceFileProof),
    Malformed(WindowsProvenanceFileProof),
}

#[cfg(any(windows, test))]
fn windows_provenance_journal_read_proof(
    read: &WindowsProvenanceJournalRead,
) -> &WindowsProvenanceFileProof {
    match read {
        WindowsProvenanceJournalRead::Empty(proof)
        | WindowsProvenanceJournalRead::Valid(_, proof)
        | WindowsProvenanceJournalRead::Malformed(proof) => proof,
    }
}

#[cfg(any(windows, test))]
fn ensure_windows_provenance_journal_slots(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    for name in PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS {
        match directory.bind_owner_private_exact_file(OsStr::new(name)) {
            Ok(file) => {
                if file.len()? > MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES {
                    return Err("private provenance journal exceeds its fixed byte budget".into());
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let created = directory.create_new_exact_file(OsStr::new(name))?;
                set_restrictive_permissions_file(&created)?;
                created.sync_all()?;
                let bound = directory.bind_owner_private_exact_file(OsStr::new(name))?;
                if !qmd_file_handles_match(&created, &bound.try_clone_exact_file()?) {
                    return Err("private provenance journal changed during creation".into());
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn read_windows_provenance_journal(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    name: &str,
) -> Result<WindowsProvenanceJournalRead, Box<dyn std::error::Error>> {
    let file = directory.bind_owner_private_exact_file(OsStr::new(name))?;
    let proof = windows_provenance_file_proof(&file, MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES)?;
    if file.is_empty()? {
        return Ok(WindowsProvenanceJournalRead::Empty(proof));
    }
    let mut bytes = Vec::with_capacity(usize::try_from(proof.len)?);
    let mut exact = file.try_clone_exact_file()?;
    exact.seek(SeekFrom::Start(0))?;
    exact.read_to_end(&mut bytes)?;
    match serde_json::from_slice::<WindowsProvenanceJournalRecord>(&bytes) {
        Ok(record) if record.schema == 1 => Ok(WindowsProvenanceJournalRead::Valid(record, proof)),
        _ => Ok(WindowsProvenanceJournalRead::Malformed(proof)),
    }
}

#[cfg(any(windows, test))]
fn write_windows_provenance_journal(
    mut file: crate::policy_fs::BoundRecoveryFile,
    record: &WindowsProvenanceJournalRecord,
) -> Result<crate::policy_fs::BoundRecoveryFile, Box<dyn std::error::Error>> {
    if !file.is_empty()? {
        return Err("private provenance journal slot is not empty".into());
    }
    let bytes = bounded_json_to_vec_pretty(record, MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES)?;
    file.fill_exact_empty_visible(&bytes)?;
    // Windows cannot flush traversal-only directory handles. Journal and
    // generation handles are opened FILE_FLAG_WRITE_THROUGH instead; every
    // namespace phase remains Active until the exact moved handle is synced
    // and the post-move layout is re-attested. Any unsupported/failed flush
    // retains the Active journal and fails closed.
    file.try_clone_exact_file()?.sync_all()?;
    Ok(file)
}

#[cfg(any(windows, test))]
fn reset_windows_provenance_journal(
    file: crate::policy_fs::BoundRecoveryFile,
) -> Result<crate::policy_fs::BoundRecoveryFile, Box<dyn std::error::Error>> {
    let empty = file.zero_exact_for_retirement()?;
    empty.attest_visible_identity()?;
    if !empty.is_empty()? {
        return Err("private provenance journal did not reset exactly".into());
    }
    Ok(empty)
}

#[cfg(any(windows, test))]
fn zero_observed_windows_provenance_residue_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    name: &OsStr,
    expected: &WindowsProvenanceFileProof,
    max_bytes: u64,
    before_rebind: impl FnOnce(),
) -> Result<(), Box<dyn std::error::Error>> {
    before_rebind();
    let file = directory.bind_owner_private_exact_file(name)?;
    if windows_provenance_file_proof(&file, max_bytes)? != *expected {
        return Err("private provenance residue changed at the exact-zero boundary".into());
    }
    let empty = file.zero_exact_for_retirement()?;
    empty.attest_visible_identity()?;
    if !empty.is_empty()? {
        return Err("private provenance residue did not retire exactly".into());
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn remove_observed_empty_windows_provenance_residue_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    name: &OsStr,
    expected: &WindowsProvenanceFileProof,
    before_rebind: impl FnOnce(),
) -> Result<(), Box<dyn std::error::Error>> {
    before_rebind();
    let file = directory.bind_owner_private_exact_file(name)?;
    if windows_provenance_file_proof(&file, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)? != *expected
        || expected.len != 0
    {
        return Err("private provenance empty residue changed at the removal boundary".into());
    }
    directory.remove_owned_private_file(file)?;
    Ok(())
}

#[cfg(any(windows, test))]
fn rename_windows_provenance_exact_no_replace_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    source: &OsStr,
    destination: &OsStr,
    expected: &WindowsProvenanceFileProof,
    before_rebind: impl FnOnce(),
) -> Result<(), Box<dyn std::error::Error>> {
    before_rebind();
    let source = directory.bind_owner_private_exact_file(source)?;
    if windows_provenance_file_proof(&source, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)? != *expected
    {
        return Err("private provenance source changed at the exact-rename boundary".into());
    }
    let moved = directory.rename_bound_no_replace(source, destination)?;
    moved.try_clone_exact_file()?.sync_all()?;
    moved.attest_visible_identity()?;
    directory.attest_for_source_cleanup()?;
    Ok(())
}

#[cfg(windows)]
fn rename_windows_provenance_exact_no_replace(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    source: &OsStr,
    destination: &OsStr,
    expected: &WindowsProvenanceFileProof,
) -> Result<(), Box<dyn std::error::Error>> {
    rename_windows_provenance_exact_no_replace_with_hook(
        directory,
        source,
        destination,
        expected,
        || {},
    )
}

#[cfg(any(windows, test))]
fn observe_windows_provenance_layout(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    previous: Option<&WindowsProvenanceFileProof>,
    intended: Option<&WindowsProvenanceFileProof>,
) -> Result<
    (
        WindowsProvenanceObservation,
        WindowsProvenanceObservation,
        WindowsProvenanceObservation,
    ),
    Box<dyn std::error::Error>,
> {
    Ok((
        inspect_windows_provenance_file(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
            previous,
            intended,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
        )?,
        inspect_windows_provenance_file(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
            previous,
            intended,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
        )?,
        inspect_windows_provenance_file(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
            previous,
            intended,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
        )?,
    ))
}

#[cfg(any(windows, test))]
fn windows_provenance_record_is_terminal(record: &WindowsProvenanceJournalRecord) -> bool {
    matches!(
        record.state,
        WindowsProvenanceJournalState::Baseline | WindowsProvenanceJournalState::Completed
    ) && record.prior_sequence.is_none()
        && record.previous.is_none()
}

#[cfg(any(windows, test))]
fn reset_observed_windows_provenance_slot_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    name: &str,
    expected: &WindowsProvenanceFileProof,
    before_rebind: impl FnOnce(),
) -> Result<crate::policy_fs::BoundRecoveryFile, Box<dyn std::error::Error>> {
    before_rebind();
    let slot = directory.bind_owner_private_exact_file(OsStr::new(name))?;
    if windows_provenance_file_proof(&slot, MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES)? != *expected {
        return Err("private provenance journal changed at the exact-reset boundary".into());
    }
    reset_windows_provenance_journal(slot)
}

#[cfg(any(windows, test))]
fn normalize_windows_provenance_terminal(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    terminal_index: usize,
    terminal: &WindowsProvenanceJournalRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    if !windows_provenance_record_is_terminal(terminal) {
        return Err("private provenance terminal journal is structurally invalid".into());
    }
    let journal_reads = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
        .iter()
        .map(|name| read_windows_provenance_journal(directory, name))
        .collect::<Result<Vec<_>, _>>()?;
    if !matches!(
        &journal_reads[terminal_index],
        WindowsProvenanceJournalRead::Valid(current, _) if current == terminal
    ) {
        return Err("private provenance terminal journal changed before normalization".into());
    }
    let (mut target, mut temporary, mut backup) =
        observe_windows_provenance_layout(directory, None, terminal.intended.as_ref())?;
    if !windows_provenance_terminal_layout_is_exact(
        terminal.intended.as_ref(),
        target.state,
        temporary.state,
        backup.state,
    ) && windows_provenance_terminal_can_abort_unjournaled_temp(
        terminal.intended.as_ref(),
        target.state,
        backup.state,
    ) {
        // Temp bytes are not publication authority until a complete Active
        // record is durable. A crash or torn write before that boundary may
        // leave only this reserved internal leaf. When the exact terminal
        // target and absence/zero backup prove no namespace mutation began,
        // retire only that exact temp handle and keep the terminal generation.
        let expected = temporary
            .proof
            .as_ref()
            .ok_or("private provenance unjournaled temp lost its exact proof")?;
        zero_observed_windows_provenance_residue_with_hook(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
            expected,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
            || {},
        )?;
        (target, temporary, backup) =
            observe_windows_provenance_layout(directory, None, terminal.intended.as_ref())?;
    }
    if !windows_provenance_terminal_layout_is_exact(
        terminal.intended.as_ref(),
        target.state,
        temporary.state,
        backup.state,
    ) {
        return Err(
            "private provenance terminal journal does not match the fixed-name layout".into(),
        );
    }
    for (index, name) in PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS.iter().enumerate() {
        if index != terminal_index {
            reset_observed_windows_provenance_slot_with_hook(
                directory,
                name,
                windows_provenance_journal_read_proof(&journal_reads[index]),
                || {},
            )?;
        }
    }
    Ok(())
}

#[cfg(windows)]
fn initialize_windows_provenance_journal(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
) -> Result<(), Box<dyn std::error::Error>> {
    initialize_windows_provenance_journal_with_hook(directory, |_| {})
}

#[cfg(any(windows, test))]
fn initialize_windows_provenance_journal_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    mut before_action: impl FnMut(WindowsProvenanceInitializationBoundary),
) -> Result<(), Box<dyn std::error::Error>> {
    let temporary = inspect_windows_provenance_file(
        directory,
        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
        None,
        None,
        MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
    )?;
    let backup = inspect_windows_provenance_file(
        directory,
        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
        None,
        None,
        MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
    )?;
    if !matches!(
        backup.state,
        WindowsProvenanceObservedFile::Absent | WindowsProvenanceObservedFile::Empty
    ) {
        return Err(
            "private provenance has byte-bearing parked state without a valid journal; recovery fails closed"
                .into(),
        );
    }
    if let Some(expected) = temporary.proof.as_ref() {
        zero_observed_windows_provenance_residue_with_hook(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
            expected,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
            || before_action(WindowsProvenanceInitializationBoundary::TemporaryRetirement),
        )?;
    }
    if let Some(expected) = backup.proof.as_ref() {
        zero_observed_windows_provenance_residue_with_hook(
            directory,
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
            expected,
            MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
            || before_action(WindowsProvenanceInitializationBoundary::BackupRetirement),
        )?;
    }
    let journal_reads = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
        .iter()
        .map(|name| read_windows_provenance_journal(directory, name))
        .collect::<Result<Vec<_>, _>>()?;
    let mut baseline_slot = None;
    for (index, name) in PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS.iter().enumerate() {
        let reset = reset_observed_windows_provenance_slot_with_hook(
            directory,
            name,
            windows_provenance_journal_read_proof(&journal_reads[index]),
            || before_action(WindowsProvenanceInitializationBoundary::JournalReset(index)),
        )?;
        if index == 0 {
            baseline_slot = Some(reset);
        }
    }
    let baseline = WindowsProvenanceJournalRecord {
        schema: 1,
        sequence: 0,
        state: WindowsProvenanceJournalState::Baseline,
        prior_sequence: None,
        previous: None,
        intended: windows_current_provenance_proof(directory)?,
    };
    before_action(WindowsProvenanceInitializationBoundary::BaselineWrite);
    write_windows_provenance_journal(
        baseline_slot.ok_or("private provenance baseline slot was not retained")?,
        &baseline,
    )?;
    normalize_windows_provenance_terminal(directory, 0, &baseline)
}

#[cfg(windows)]
fn recover_windows_provenance_publication(
    namespace: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(namespace)?;
    ensure_windows_provenance_journal_slots(&directory)?;
    let reads = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
        .iter()
        .map(|name| read_windows_provenance_journal(&directory, name))
        .collect::<Result<Vec<_>, _>>()?;
    let records = reads
        .iter()
        .enumerate()
        .filter_map(|(index, read)| match read {
            WindowsProvenanceJournalRead::Valid(record, _) => Some((index, record.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    if records.is_empty() {
        return initialize_windows_provenance_journal(&directory);
    }
    let (latest_index, latest) = select_latest_windows_provenance_record(&records)?
        .ok_or("private provenance journal selection failed")?;
    if windows_provenance_record_is_terminal(&latest) {
        return normalize_windows_provenance_terminal(&directory, latest_index, &latest);
    }
    if latest.state != WindowsProvenanceJournalState::Active
        || latest.intended.is_none()
        || latest.prior_sequence.is_none()
        || latest.previous == latest.intended
    {
        return Err("private provenance Active journal is structurally invalid".into());
    }
    let prior_index = records
        .iter()
        .find_map(|(index, record)| {
            (Some(record.sequence) == latest.prior_sequence
                && windows_provenance_record_is_terminal(record)
                && record.intended == latest.previous)
                .then_some(*index)
        })
        .ok_or("private provenance Active journal lost its exact prior terminal receipt")?;
    let intended = latest
        .intended
        .as_ref()
        .ok_or("private provenance Active journal lost its intended proof")?;

    loop {
        let (target, temporary, backup) = observe_windows_provenance_layout(
            &directory,
            latest.previous.as_ref(),
            Some(intended),
        )?;
        match classify_windows_provenance_active_layout(
            latest.previous.as_ref(),
            target.state,
            temporary.state,
            backup.state,
        ) {
            WindowsProvenanceActiveLayout::PreMutation => {
                if backup.state == WindowsProvenanceObservedFile::Empty {
                    remove_observed_empty_windows_provenance_residue_with_hook(
                        &directory,
                        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
                        backup
                            .proof
                            .as_ref()
                            .ok_or("private provenance empty backup lost its exact proof")?,
                        || {},
                    )?;
                }
                if latest.previous.is_some() {
                    rename_windows_provenance_exact_no_replace(
                        &directory,
                        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
                        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
                        target
                            .proof
                            .as_ref()
                            .ok_or("private provenance target lost its exact proof")?,
                    )?;
                } else {
                    rename_windows_provenance_exact_no_replace(
                        &directory,
                        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
                        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
                        temporary
                            .proof
                            .as_ref()
                            .ok_or("private provenance temp lost its exact proof")?,
                    )?;
                }
            }
            WindowsProvenanceActiveLayout::PreviousParked => {
                rename_windows_provenance_exact_no_replace(
                    &directory,
                    OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
                    OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
                    temporary
                        .proof
                        .as_ref()
                        .ok_or("private provenance temp lost its exact proof")?,
                )?;
            }
            WindowsProvenanceActiveLayout::PublishedWithPrevious => {
                zero_observed_windows_provenance_residue_with_hook(
                    &directory,
                    OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
                    backup
                        .proof
                        .as_ref()
                        .ok_or("private provenance backup lost its exact proof")?,
                    MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
                    || {},
                )?;
            }
            WindowsProvenanceActiveLayout::PublishedRetired => break,
            WindowsProvenanceActiveLayout::Unknown => {
                return Err(
                    "private provenance publication layout is ambiguous; Active journal retained"
                        .into(),
                )
            }
        }
    }

    let completion_index = (0..PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS.len())
        .find(|index| *index != latest_index && *index != prior_index)
        .ok_or("private provenance completion journal has no fixed slot")?;
    let completion_slot = reset_observed_windows_provenance_slot_with_hook(
        &directory,
        PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[completion_index],
        windows_provenance_journal_read_proof(&reads[completion_index]),
        || {},
    )?;
    let completed = WindowsProvenanceJournalRecord {
        schema: 1,
        sequence: latest
            .sequence
            .checked_add(1)
            .ok_or("private provenance journal sequence overflowed")?,
        state: WindowsProvenanceJournalState::Completed,
        prior_sequence: None,
        previous: None,
        intended: Some(intended.clone()),
    };
    write_windows_provenance_journal(completion_slot, &completed)?;
    let (target, temporary, backup) =
        observe_windows_provenance_layout(&directory, None, completed.intended.as_ref())?;
    if !windows_provenance_terminal_layout_is_exact(
        completed.intended.as_ref(),
        target.state,
        temporary.state,
        backup.state,
    ) {
        return Err("private provenance completion was not durably re-attested".into());
    }
    reset_observed_windows_provenance_slot_with_hook(
        &directory,
        PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[latest_index],
        windows_provenance_journal_read_proof(&reads[latest_index]),
        || {},
    )?;
    reset_observed_windows_provenance_slot_with_hook(
        &directory,
        PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[prior_index],
        windows_provenance_journal_read_proof(&reads[prior_index]),
        || {},
    )?;
    normalize_windows_provenance_terminal(&directory, completion_index, &completed)
}

#[cfg(any(windows, test))]
fn retire_equal_content_windows_provenance_temp_with_hook(
    directory: &crate::policy_fs::BoundRecoveryDirectory,
    current: Option<&WindowsProvenanceFileProof>,
    intended: &WindowsProvenanceFileProof,
    intended_bytes: &[u8],
    before_rebind: impl FnOnce(),
) -> Result<bool, Box<dyn std::error::Error>> {
    if intended.len != u64::try_from(intended_bytes.len())?
        || intended.sha256 != <[u8; 32]>::from(Sha256::digest(intended_bytes))
    {
        return Err("private provenance temp no longer matches intended bytes".into());
    }
    if !current
        .is_some_and(|current| current.len == intended.len && current.sha256 == intended.sha256)
    {
        return Ok(false);
    }
    zero_observed_windows_provenance_residue_with_hook(
        directory,
        OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
        intended,
        MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
        before_rebind,
    )?;
    Ok(true)
}

#[cfg(windows)]
fn begin_windows_provenance_publication(
    namespace: &Path,
    intended_bytes: &[u8],
) -> Result<bool, Box<dyn std::error::Error>> {
    let directory = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(namespace)?;
    let reads = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
        .iter()
        .map(|name| read_windows_provenance_journal(&directory, name))
        .collect::<Result<Vec<_>, _>>()?;
    let terminals = reads
        .iter()
        .enumerate()
        .filter_map(|(index, read)| match read {
            WindowsProvenanceJournalRead::Valid(record, _)
                if windows_provenance_record_is_terminal(record) =>
            {
                Some((index, record.clone()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    if terminals.len() != 1 {
        return Err(
            "private provenance publication requires one normalized terminal receipt".into(),
        );
    }
    let (terminal_index, terminal) = terminals[0].clone();
    let current = windows_current_provenance_proof(&directory)?;
    if current != terminal.intended {
        return Err("private provenance target changed after terminal recovery".into());
    }
    let temporary =
        directory.bind_owner_private_exact_file(OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP))?;
    let intended =
        windows_provenance_file_proof(&temporary, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)?;
    if retire_equal_content_windows_provenance_temp_with_hook(
        &directory,
        current.as_ref(),
        &intended,
        intended_bytes,
        || {},
    )? {
        return Ok(false);
    }
    let active_index = reads
        .iter()
        .enumerate()
        .find_map(|(index, read)| {
            (index != terminal_index && matches!(read, WindowsProvenanceJournalRead::Empty(_)))
                .then_some(index)
        })
        .ok_or("private provenance publication has no empty Active journal slot")?;
    let active = WindowsProvenanceJournalRecord {
        schema: 1,
        sequence: terminal
            .sequence
            .checked_add(1)
            .ok_or("private provenance journal sequence overflowed")?,
        state: WindowsProvenanceJournalState::Active,
        prior_sequence: Some(terminal.sequence),
        previous: current,
        intended: Some(intended),
    };
    let active_slot = directory.bind_owner_private_exact_file(OsStr::new(
        PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[active_index],
    ))?;
    if windows_provenance_file_proof(&active_slot, MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES)?
        != *windows_provenance_journal_read_proof(&reads[active_index])
    {
        return Err("private provenance Active slot changed before journal publication".into());
    }
    write_windows_provenance_journal(active_slot, &active)?;
    recover_windows_provenance_publication(namespace)?;
    Ok(true)
}

fn save_provenance_manifest(
    config: &Config,
    manifest: &KnowledgeProvenanceManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    save_provenance_manifest_with_hook(config, manifest, |_| Ok(()))
}

fn save_provenance_manifest_with_hook(
    config: &Config,
    manifest: &KnowledgeProvenanceManifest,
    after_temp_sync: impl FnOnce(&Path) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = serde_json::to_vec_pretty(manifest)?;
    if u64::try_from(bytes.len())? > MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES {
        return Err("knowledge provenance manifest exceeds its private byte budget".into());
    }

    // A legacy/public manifest is untrusted input. Preserve its exact bytes in
    // the private reconciliation store and retract the public name before
    // publishing any new positive authority. If quarantine fails, the private
    // manifest is left unchanged and the whole update fails closed.
    let legacy = legacy_public_provenance_manifest_path(config);
    match fs::symlink_metadata(&legacy) {
        Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
            let identity = preserve_file_before_retraction(config, &legacy)?;
            remove_preserved_file(config, &legacy, &identity)?;
        }
        Ok(_) => return Err("legacy public provenance manifest is not a regular file".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let (namespace, namespace_directory, _root_directory) = open_preservation_namespace(config)?;
    let private = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace)?;
    #[cfg(windows)]
    recover_windows_provenance_publication(&namespace)?;
    let temporary_name = OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP);
    let mut temporary = match private.bind_owner_private_exact_file(temporary_name) {
        Ok(existing) => {
            if existing.len()? > MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES {
                return Err("private provenance temp exceeds its fixed byte budget".into());
            }
            existing.zero_exact_for_retirement()?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let created = private.create_new_exact_file(temporary_name)?;
            set_restrictive_permissions_file(&created)?;
            created.sync_all()?;
            let bound = private.bind_owner_private_exact_file(temporary_name)?;
            if !qmd_file_handles_match(&created, &bound.try_clone_exact_file()?) {
                return Err("private provenance temp changed while it was created".into());
            }
            bound
        }
        Err(error) => return Err(error.into()),
    };
    temporary.fill_exact_empty_visible(&bytes)?;
    after_temp_sync(&namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP))?;

    let namespace_cap = CapDir::from_std_file(namespace_directory);
    let mut read_options = CapOpenOptions::new();
    read_options.read(true);
    #[cfg(unix)]
    read_options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        read_options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let temporary_confirmed = namespace_cap
        .open_with(temporary_name, &read_options)?
        .into_std();
    let temporary_file = temporary.try_clone_exact_file()?;
    if !qmd_file_handles_match(&temporary_file, &temporary_confirmed) {
        return Err("private provenance namespace changed during publication".into());
    }
    drop(temporary_confirmed);
    #[cfg(not(windows))]
    publish_fixed_provenance_temp(&namespace_cap, &namespace)?;
    #[cfg(windows)]
    let windows_published_temp = begin_windows_provenance_publication(&namespace, &bytes)?;
    private.sync()?;
    let published =
        private.bind_owner_private_exact_file(OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST))?;
    let published_confirmed = namespace_cap
        .open_with(
            OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST),
            &read_options,
        )?
        .into_std();
    #[cfg(not(windows))]
    let temp_identity_changed =
        !qmd_file_handles_match(&temporary_file, &published.try_clone_exact_file()?);
    #[cfg(windows)]
    let temp_identity_changed = windows_published_temp
        && !qmd_file_handles_match(&temporary_file, &published.try_clone_exact_file()?);
    if temp_identity_changed
        || !qmd_file_handles_match(&published.try_clone_exact_file()?, &published_confirmed)
    {
        return Err("private provenance namespace changed after publication".into());
    }
    published.recovery_proof_for_exact_bytes_bounded(
        &bytes,
        MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
        Instant::now() + Duration::from_secs(5),
    )?;
    Ok(())
}

fn record_authorized_source(
    config: &Config,
    authorized: &AuthorizedMeeting,
    records: BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (mut manifest, valid) = load_provenance_manifest(config);
    if !valid {
        manifest = KnowledgeProvenanceManifest::default();
    }
    manifest.schema = 4;
    let key = exact_source_key(&authorized.path, config)
        .ok_or("authorized meeting lacks exact v2 provenance")?;
    manifest
        .sources
        .insert(key.clone(), source_revision(&authorized.content));
    manifest.records.insert(key.clone(), records);
    if let Some(log) = managed_log_relative_path(config) {
        manifest.managed_logs.insert(log);
    }
    save_provenance_manifest(config, &manifest)
}

fn preferred_source_key(
    path: &Path,
    config: &Config,
) -> Result<String, Box<dyn std::error::Error>> {
    source_keys_for_path(path, config)
        .into_iter()
        .last()
        .ok_or_else(|| {
            format!(
                "meeting {} has no safe corpus-relative identity",
                privacy_safe_source_scope(path)
            )
            .into()
        })
}

fn corpus_markdown_entries(config: &Config) -> impl Iterator<Item = walkdir::DirEntry> + '_ {
    WalkDir::new(&config.output_dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            if entry.file_type().is_dir() {
                if entry.depth() == 0 {
                    return true;
                }
                !is_inactive_corpus_dir_name(entry.file_name())
            } else {
                true
            }
        })
        .filter_map(Result::ok)
        .filter(move |entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "md")
                && is_live_corpus_path(entry.path(), config)
        })
}

/// Candidate paths under the one canonical live-corpus traversal policy.
/// Classification still occurs from descriptor-bound bytes in
/// `authorized_meeting`; callers must not treat this list as authorization.
pub fn live_corpus_markdown_paths(config: &Config) -> Vec<PathBuf> {
    corpus_markdown_entries(config)
        .map(|entry| entry.into_path())
        .collect()
}

/// Historical Minutes-owned QMD mirror path. Persistent QMD is disabled; this
/// path remains public only so status/remediation code can identify and retract
/// registrations created by older releases.
pub fn qmd_policy_mirror_path() -> PathBuf {
    Config::minutes_dir().join(QMD_MIRROR_DIR)
}

/// Persistent QMD mirrors are intentionally disabled. A globally queryable
/// third-party index can outlive Minutes and cannot promise immediate policy
/// revocation after an external writer changes a meeting. This entry point
/// first retracts any previously owned registration and plaintext mirror.
pub fn rebuild_qmd_policy_mirror(
    config: &Config,
) -> Result<QmdMirrorResult, Box<dyn std::error::Error>> {
    ensure_qmd_persistence_disabled(config)?;
    Err(QMD_PERSISTENCE_DISABLED_REASON.into())
}

#[cfg(test)]
fn rebuild_qmd_policy_mirror_at(
    config: &Config,
    mirror: &Path,
) -> Result<QmdMirrorResult, Box<dyn std::error::Error>> {
    rebuild_qmd_policy_mirror_at_with_hook(config, mirror, |_, _| {})
}

#[cfg(test)]
fn qmd_relative_source_key(relative: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let relative = relative
        .to_str()
        .ok_or("QMD source path is not valid UTF-8")?
        .replace('\\', "/");
    if relative.is_empty() || !active_corpus_relative_path(Path::new(&relative)) {
        return Err("QMD source path is outside the active corpus".into());
    }
    Ok(relative)
}

#[cfg(test)]
fn current_qmd_source_revisions(
    config: &Config,
    root: &Path,
) -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
    let mut sources = BTreeMap::new();
    for entry in corpus_markdown_entries(config) {
        let authorized = match authorized_meeting(entry.path(), config) {
            Ok(authorized) => authorized,
            Err(_) => continue,
        };
        let relative = authorized.path.strip_prefix(root)?;
        let key = qmd_relative_source_key(relative)?;
        if sources
            .insert(key, source_revision(&authorized.content))
            .is_some()
        {
            return Err("QMD source identity was duplicated".into());
        }
    }
    Ok(sources)
}

#[cfg(test)]
fn read_qmd_mirror_marker(mirror: &Path) -> Result<QmdMirrorMarker, Box<dyn std::error::Error>> {
    let marker_path = mirror.join(QMD_MIRROR_MARKER);
    if fs::symlink_metadata(&marker_path)?.file_type().is_symlink() {
        return Err("QMD mirror marker is unsafe".into());
    }
    let mut marker_file = open_regular_file_no_follow(&marker_path)?;
    let mut raw = Vec::new();
    marker_file.read_to_end(&mut raw)?;
    let marker: QmdMirrorMarker = serde_json::from_slice(&raw)?;
    if marker.schema != 2
        || marker.source != "configured-meeting-corpus"
        || marker.policy != "normal-only-strict-frontmatter-no-links"
    {
        return Err("QMD mirror marker was invalid".into());
    }
    Ok(marker)
}

#[cfg(test)]
fn attest_qmd_policy_mirror_at(
    config: &Config,
    mirror: &Path,
) -> Result<usize, Box<dyn std::error::Error>> {
    let root = canonical_output_root(config)?;
    let marker = read_qmd_mirror_marker(mirror)?;
    let live = current_qmd_source_revisions(config, &root)?;
    if marker.sources != live {
        return Err("QMD mirror source revisions no longer match the live corpus".into());
    }

    let mut mirrored = BTreeMap::new();
    for entry in WalkDir::new(mirror).follow_links(false) {
        let entry = entry?;
        if entry.depth() == 0 || entry.file_type().is_dir() {
            continue;
        }
        if entry.file_type().is_symlink() || !entry.file_type().is_file() {
            return Err("QMD mirror contains an unsafe entry".into());
        }
        let relative = entry.path().strip_prefix(mirror)?;
        if relative == Path::new(QMD_MIRROR_MARKER) {
            continue;
        }
        let key = qmd_relative_source_key(relative)?;
        let mut file = open_regular_file_no_follow(entry.path())?;
        let mut content = String::new();
        file.read_to_string(&mut content)?;
        if mirrored.insert(key, source_revision(&content)).is_some() {
            return Err("QMD mirror source identity was duplicated".into());
        }
    }
    if mirrored != marker.sources {
        return Err("QMD mirror bytes did not match their source manifest".into());
    }
    Ok(mirrored.len())
}

#[cfg(test)]
fn rebuild_qmd_policy_mirror_at_with_hook<F>(
    config: &Config,
    mirror: &Path,
    mut after_copy: F,
) -> Result<QmdMirrorResult, Box<dyn std::error::Error>>
where
    F: FnMut(usize, &Path),
{
    let root = canonical_output_root(config)?;
    let parent = mirror
        .parent()
        .ok_or("QMD policy mirror has no parent directory")?;
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".{QMD_MIRROR_DIR}.staging-{}", std::process::id()));
    let backup = parent.join(format!(".{QMD_MIRROR_DIR}.previous-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }
    fs::create_dir(&staging)?;
    set_restrictive_directory_permissions(&staging)?;

    let result = (|| -> Result<BTreeMap<String, String>, Box<dyn std::error::Error>> {
        let mut copied = 0usize;
        let mut sources = BTreeMap::new();
        for entry in corpus_markdown_entries(config) {
            let authorized = match authorized_meeting(entry.path(), config) {
                Ok(authorized) => authorized,
                Err(error) => {
                    tracing::debug!(
                        source_scope = %privacy_safe_source_scope(entry.path()),
                        reason = %policy_reason(&error.to_string()),
                        "meeting excluded from QMD policy mirror"
                    );
                    continue;
                }
            };
            let relative = authorized.path.strip_prefix(&root)?;
            let source_key = qmd_relative_source_key(relative)?;
            let destination = staging.join(relative);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)?;
                set_restrictive_directory_permissions(parent)?;
            }
            fs::write(&destination, authorized.content.as_bytes())?;
            set_restrictive_permissions(&destination)?;
            if sources
                .insert(source_key, source_revision(&authorized.content))
                .is_some()
            {
                return Err("QMD source identity was duplicated".into());
            }
            copied += 1;
            after_copy(copied, relative);
        }
        if current_qmd_source_revisions(config, &root)? != sources {
            return Err("QMD source corpus changed during mirror rebuild".into());
        }
        let marker = QmdMirrorMarker {
            schema: 2,
            source: "configured-meeting-corpus".into(),
            policy: "normal-only-strict-frontmatter-no-links".into(),
            sources: sources.clone(),
        };
        fs::write(
            staging.join(QMD_MIRROR_MARKER),
            serde_json::to_vec_pretty(&marker)?,
        )?;
        set_restrictive_permissions(&staging.join(QMD_MIRROR_MARKER))?;
        if attest_qmd_policy_mirror_at(config, &staging)? != copied {
            return Err("QMD mirror pre-swap attestation failed".into());
        }
        Ok(sources)
    })();

    let sources = match result {
        Ok(sources) => sources,
        Err(error) => {
            fs::remove_dir_all(&staging).ok();
            return Err(error);
        }
    };

    if mirror.exists() {
        fs::rename(mirror, &backup)?;
    }
    if let Err(error) = fs::rename(&staging, mirror) {
        if backup.exists() {
            fs::rename(&backup, mirror).ok();
        }
        return Err(error.into());
    }
    if let Err(error) = attest_qmd_policy_mirror_at(config, mirror) {
        fs::remove_dir_all(mirror).ok();
        if backup.exists() {
            fs::rename(&backup, mirror).ok();
        }
        return Err(error);
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)?;
    }

    Ok(QmdMirrorResult {
        path: mirror.canonicalize()?,
        files: sources.len(),
    })
}

/// Parse `qmd collection show` output without assuming a particular amount of
/// label padding.
pub fn parse_qmd_collection_path(stdout: &str) -> Option<PathBuf> {
    let paths: Vec<PathBuf> = stdout
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix("Path:"))
        .map(|path| PathBuf::from(path.trim()))
        .collect();
    match paths.as_slice() {
        [path] if path.is_absolute() && !path.as_os_str().is_empty() => Some(path.clone()),
        _ => None,
    }
}

const MAX_QMD_REGISTRY_COLLECTIONS: usize = 128;
const MAX_QMD_OPERATION_COMMANDS: usize = 1024;
const MAX_QMD_COMMAND_STDOUT_BYTES: u64 = 512 * 1024;
const MAX_QMD_COMMAND_STDERR_BYTES: usize = 64 * 1024;

fn parse_qmd_collection_names(stdout: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let mut declared_count = None;
    let mut upstream_zero_found = false;
    let mut upstream_zero_hint = false;
    for line in stdout.lines() {
        let trimmed = line.trim();
        if !line.chars().next().is_some_and(char::is_whitespace) {
            if let Some((name, suffix)) = trimmed.split_once(" (qmd://") {
                if declared_count.is_none() || upstream_zero_found {
                    return Err("QMD registry list output was malformed".into());
                }
                let name = name.trim();
                if name.is_empty() || !suffix.contains(')') {
                    return Err("QMD registry list output was malformed".into());
                }
                if names.len() >= MAX_QMD_REGISTRY_COLLECTIONS {
                    return Err("QMD registry collection budget exceeded".into());
                }
                names.push(name.to_string());
                continue;
            }
        }
        if trimmed.is_empty() {
            continue;
        }
        if let Some(count) = trimmed
            .strip_prefix("Collections (")
            .and_then(|value| value.strip_suffix("):"))
            .and_then(|value| value.parse::<usize>().ok())
        {
            if declared_count.replace(count).is_some() {
                return Err("QMD registry list output was malformed".into());
            }
            if count > MAX_QMD_REGISTRY_COLLECTIONS {
                return Err("QMD registry collection budget exceeded".into());
            }
            continue;
        }
        if line == "No collections" {
            if declared_count.replace(0).is_some() {
                return Err("QMD registry list output was malformed".into());
            }
            continue;
        }
        if line == "No collections found." {
            if declared_count.replace(0).is_some() {
                return Err("QMD registry list output was malformed".into());
            }
            upstream_zero_found = true;
            continue;
        }
        if line == "Run 'qmd collection add .' to create one." {
            if !upstream_zero_found || upstream_zero_hint {
                return Err("QMD registry list output was malformed".into());
            }
            upstream_zero_hint = true;
            continue;
        }
        if line.chars().next().is_some_and(char::is_whitespace)
            && [
                "Pattern:",
                "Ignore:",
                "Files:",
                "Updated:",
                "Index:",
                "Description:",
            ]
            .iter()
            .any(|label| trimmed.starts_with(label))
            || (line.chars().next().is_some_and(char::is_whitespace) && trimmed == "[excluded]")
        {
            if declared_count.is_none() || declared_count == Some(0) {
                return Err("QMD registry list output was malformed".into());
            }
            continue;
        }
        return Err("QMD registry list output was malformed".into());
    }
    if declared_count.is_none()
        || declared_count.is_some_and(|count| count != names.len())
        || upstream_zero_found != upstream_zero_hint
    {
        return Err("QMD registry list output was malformed".into());
    }
    Ok(names)
}

#[derive(Debug, Clone)]
struct QmdCommandResult {
    success: bool,
    stdout: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum QmdRunError {
    DeadlineExceeded,
    OutputLimitExceeded(&'static str),
    Io {
        kind: std::io::ErrorKind,
        message: String,
    },
    Other(String),
}

impl QmdRunError {
    fn io(error: std::io::Error) -> Self {
        Self::Io {
            kind: error.kind(),
            message: error.to_string(),
        }
    }
}

impl std::fmt::Display for QmdRunError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeadlineExceeded => {
                formatter.write_str("QMD command exceeded the registry operation deadline")
            }
            Self::OutputLimitExceeded(stream) => {
                write!(formatter, "QMD command {stream} resource budget exceeded")
            }
            Self::Io { kind, message } => {
                write!(
                    formatter,
                    "QMD command execution failed ({kind:?}): {message}"
                )
            }
            Self::Other(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for QmdRunError {}

impl From<String> for QmdRunError {
    fn from(message: String) -> Self {
        Self::Other(message)
    }
}

impl From<&str> for QmdRunError {
    fn from(message: &str) -> Self {
        Self::Other(message.to_string())
    }
}

trait QmdRunner {
    fn run_until(&self, args: &[&str], deadline: Instant) -> Result<QmdCommandResult, QmdRunError>;
}

struct SystemQmdRunner;

fn run_bounded_qmd_command(
    command: &mut crate::bounded_child::BoundedCommand,
    deadline: Instant,
) -> Result<QmdCommandResult, QmdRunError> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(QmdRunError::DeadlineExceeded);
    }
    let run = crate::bounded_child::run(
        command,
        None,
        crate::bounded_child::StdoutTarget::Capture {
            max_bytes: MAX_QMD_COMMAND_STDOUT_BYTES,
        },
        crate::bounded_child::ChildBudget {
            wall_clock: remaining,
            // Retain one sentinel byte beyond the public limit so oversized
            // finite stderr is distinguishable from exact-limit output without
            // ever retaining an unbounded diagnostic. A streaming flood is
            // still bounded by the same command deadline.
            stderr_tail: MAX_QMD_COMMAND_STDERR_BYTES + 1,
        },
    )
    .map_err(|error| {
        if error
            .to_string()
            .contains("stdout resource budget exceeded")
        {
            QmdRunError::OutputLimitExceeded("stdout")
        } else {
            QmdRunError::io(error)
        }
    })?;
    if run.timed_out {
        return Err(QmdRunError::DeadlineExceeded);
    }
    if run.output.stderr.len() > MAX_QMD_COMMAND_STDERR_BYTES {
        return Err(QmdRunError::OutputLimitExceeded("stderr"));
    }
    Ok(QmdCommandResult {
        success: run.output.status.success(),
        stdout: String::from_utf8_lossy(&run.output.stdout).into_owned(),
    })
}

impl QmdRunner for SystemQmdRunner {
    fn run_until(&self, args: &[&str], deadline: Instant) -> Result<QmdCommandResult, QmdRunError> {
        let mut command = crate::bounded_child::BoundedCommand::new("qmd");
        command.args(args);
        run_bounded_qmd_command(&mut command, deadline)
    }
}

struct QmdOperationRunner<'a, R: QmdRunner> {
    runner: &'a R,
    deadline: Instant,
    commands_started: std::cell::Cell<usize>,
    /// Set the first time a `qmd` command comes back successful, meaning the
    /// registry answered us at least once.
    ///
    /// `run` flattens errors to strings for callers, so without this the
    /// difference between "we could not inspect a registry that answered us"
    /// and "there is no registry here to inspect" is lost, and that difference
    /// is the whole of #788.
    ///
    /// This tracks the positive fact rather than the negative one on purpose.
    /// The retraction runs a second audit after its removals and can fail with
    /// an I/O error at any point, so "the last command failed to spawn" does
    /// not mean qmd was never there. Once it has answered, every later failure
    /// keeps failing closed.
    registry_answered: std::cell::Cell<bool>,
}

impl<'a, R: QmdRunner> QmdOperationRunner<'a, R> {
    fn new(runner: &'a R) -> Self {
        Self::with_timeout(runner, QMD_RETIREMENT_DEADLINE)
    }

    fn with_timeout(runner: &'a R, timeout: Duration) -> Self {
        Self {
            runner,
            deadline: Instant::now() + timeout,
            commands_started: std::cell::Cell::new(0),
            registry_answered: std::cell::Cell::new(false),
        }
    }

    fn run(&self, args: &[&str]) -> Result<QmdCommandResult, String> {
        if Instant::now() >= self.deadline {
            return Err(QmdRunError::DeadlineExceeded.to_string());
        }
        let started = self.commands_started.get();
        if started >= MAX_QMD_OPERATION_COMMANDS {
            return Err("QMD registry command budget exceeded".into());
        }
        self.commands_started.set(started + 1);
        let result = self.runner.run_until(args, self.deadline);
        if result.as_ref().is_ok_and(|command| command.success) {
            self.registry_answered.set(true);
        }
        result.map_err(|error| error.to_string())
    }

    /// Whether a command failed specifically because `qmd` is not installed.
    fn registry_never_answered(&self) -> bool {
        !self.registry_answered.get()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct QmdOwnedTarget {
    schema: u32,
    target: String,
}

pub fn qmd_collection_name_is_valid(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    name.len() <= 64
        && first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
}

fn load_qmd_owned_target(directory: &Path) -> Result<Option<String>, String> {
    let path = directory.join(QMD_OWNED_TARGET);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("QMD ownership marker is unsafe".into());
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("QMD ownership marker could not be inspected".into()),
    }
    let raw = fs::read_to_string(&path)
        .map_err(|_| "QMD ownership marker could not be read".to_string())?;
    let marker: QmdOwnedTarget =
        serde_json::from_str(&raw).map_err(|_| "QMD ownership marker was malformed".to_string())?;
    if marker.schema != 1 || !qmd_collection_name_is_valid(&marker.target) {
        return Err("QMD ownership marker was malformed".into());
    }
    Ok(Some(marker.target))
}

#[cfg(test)]
fn save_qmd_owned_target(directory: &Path, target: &str) -> Result<(), String> {
    if !qmd_collection_name_is_valid(target) {
        return Err("QMD collection target is invalid".into());
    }
    create_private_dir_all_no_follow(directory)
        .map_err(|_| "QMD ownership marker directory was unsafe".to_string())?;
    let path = directory.join(QMD_OWNED_TARGET);
    if fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        return Err("QMD ownership marker is unsafe".into());
    }
    let temporary = path.with_extension("json.tmp");
    if fs::symlink_metadata(&temporary).is_ok() {
        fs::remove_file(&temporary)
            .map_err(|_| "QMD ownership marker temporary could not be reset".to_string())?;
    }
    let marker = QmdOwnedTarget {
        schema: 1,
        target: target.to_string(),
    };
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(|_| "QMD ownership marker could not be created".to_string())?;
    set_restrictive_permissions_file(&file)
        .map_err(|_| "QMD ownership marker permissions failed".to_string())?;
    file.write_all(
        &serde_json::to_vec(&marker)
            .map_err(|_| "QMD ownership marker could not be encoded".to_string())?,
    )
    .map_err(|_| "QMD ownership marker could not be written".to_string())?;
    file.sync_all()
        .map_err(|_| "QMD ownership marker could not be synced".to_string())?;
    fs::rename(&temporary, &path)
        .map_err(|_| "QMD ownership marker could not be committed".to_string())?;
    qmd_sync_directory(directory)
        .map_err(|_| "QMD ownership marker directory could not be synced".to_string())?;
    Ok(())
}

fn clear_qmd_owned_target(directory: &Path) -> Result<(), String> {
    let path = directory.join(QMD_OWNED_TARGET);
    match fs::remove_file(path) {
        Ok(()) => qmd_sync_directory(directory)
            .map_err(|_| "QMD ownership marker cleanup could not be synced".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("QMD ownership marker could not be cleared".into()),
    }
}

fn mark_qmd_retirement_pending(directory: &Path) -> Result<(), String> {
    create_private_dir_all_no_follow(directory)
        .map_err(|_| "QMD retirement marker directory was unsafe".to_string())?;
    let path = directory.join(QMD_RETIREMENT_PENDING);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => return Ok(()),
        Ok(_) => return Err("QMD retirement marker is unsafe".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("QMD retirement marker could not be inspected".into()),
    }
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|_| "QMD retirement marker could not be created".to_string())?;
    set_restrictive_permissions_file(&file)
        .map_err(|_| "QMD retirement marker permissions failed".to_string())?;
    file.sync_all()
        .map_err(|_| "QMD retirement marker could not be synced".to_string())?;
    qmd_sync_directory(directory)
        .map_err(|_| "QMD retirement marker directory could not be synced".to_string())
}

fn clear_qmd_retirement_pending(directory: &Path) -> Result<(), String> {
    let path = directory.join(QMD_RETIREMENT_PENDING);
    match fs::remove_file(path) {
        Ok(()) => qmd_sync_directory(directory)
            .map_err(|_| "QMD retirement marker cleanup could not be synced".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("QMD retirement marker could not be cleared".into()),
    }
}

#[derive(Debug, Default)]
struct QmdRegistryAudit {
    all: Vec<String>,
    safe: Vec<String>,
    owned_unsafe: Vec<String>,
    unrelated_uninspectable: Vec<String>,
}

fn qmd_security_inspection_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        // `/var`, `/tmp`, and `/etc` are fixed platform aliases into
        // `/private` on macOS. Treating those roots like user-controlled
        // symlink components makes an otherwise exact QMD registration under
        // a TempDir uninspectable. Rewrite only these documented root aliases
        // before component inspection; every component below the real root is
        // still checked with `symlink_metadata`.
        for (alias, real) in [
            (Path::new("/var"), Path::new("/private/var")),
            (Path::new("/tmp"), Path::new("/private/tmp")),
            (Path::new("/etc"), Path::new("/private/etc")),
        ] {
            if let Ok(relative) = path.strip_prefix(alias) {
                return real.join(relative);
            }
        }
    }
    path.to_path_buf()
}

fn path_has_symlink_component(path: &Path) -> Result<bool, ()> {
    if !path.is_absolute() {
        return Ok(true);
    }
    let inspection_path = qmd_security_inspection_path(path);
    let mut ancestors: Vec<&Path> = inspection_path.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        let metadata = fs::symlink_metadata(ancestor).map_err(|_| ())?;
        if metadata.file_type().is_symlink() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn exact_stable_path(candidate: &Path, expected: &Path) -> Result<bool, ()> {
    if qmd_security_inspection_path(candidate) != qmd_security_inspection_path(expected)
        || path_has_symlink_component(candidate)?
        || path_has_symlink_component(expected)?
    {
        return Ok(false);
    }
    let before = candidate.canonicalize().map_err(|_| ())?;
    let expected_canonical = expected.canonicalize().map_err(|_| ())?;
    let after = candidate.canonicalize().map_err(|_| ())?;
    Ok(before == expected_canonical
        && after == before
        && !path_has_symlink_component(candidate)?
        && !path_has_symlink_component(expected)?)
}

fn stable_canonical_alias(candidate: &Path, expected: &Path) -> Result<bool, ()> {
    if path_has_symlink_component(candidate)? || path_has_symlink_component(expected)? {
        return Ok(false);
    }
    let before = candidate.canonicalize().map_err(|_| ())?;
    let expected_canonical = expected.canonicalize().map_err(|_| ())?;
    let after = candidate.canonicalize().map_err(|_| ())?;
    Ok(before == expected_canonical
        && after == before
        && !path_has_symlink_component(candidate)?
        && !path_has_symlink_component(expected)?)
}

pub fn qmd_path_is_exact_policy_mirror(candidate: &Path) -> bool {
    let Ok(expected) = qmd_policy_mirror_path().canonicalize() else {
        return false;
    };
    exact_stable_path(candidate, &expected).unwrap_or(false)
}

pub fn qmd_path_is_provable_raw_alias(candidate: &Path, config: &Config) -> bool {
    stable_canonical_alias(candidate, &config.output_dir).unwrap_or(false)
}

pub fn qmd_path_is_uninspectable(candidate: &Path) -> bool {
    path_has_symlink_component(candidate).unwrap_or(true) || candidate.canonicalize().is_err()
}

fn qmd_registry_audit<R: QmdRunner>(
    runner: &QmdOperationRunner<'_, R>,
    target: &str,
    mirror: &Path,
    raw_root: &Path,
) -> Result<QmdRegistryAudit, String> {
    let list = runner.run(&["collection", "list"])?;
    if !list.success {
        // qmd ran but declined to enumerate. A broken install fails this way,
        // a native-binding load failure exits non-zero, and it leaves us as
        // uninformed as no qmd at all. Nothing to set here: an unsuccessful
        // command never marks the registry as having answered.
        return Err("QMD registry could not be listed".into());
    }
    let all = parse_qmd_collection_names(&list.stdout)?;
    let mut audit = QmdRegistryAudit::default();
    for name in all {
        audit.all.push(name.clone());
        let show = match runner.run(&["collection", "show", &name]) {
            Ok(show) if show.success => show,
            Ok(_) => {
                if name == target {
                    audit.owned_unsafe.push(name);
                } else {
                    audit.unrelated_uninspectable.push(name);
                }
                continue;
            }
            Err(error) => return Err(error),
        };
        let Some(path) = parse_qmd_collection_path(&show.stdout) else {
            if name == target {
                audit.owned_unsafe.push(name);
            } else {
                audit.unrelated_uninspectable.push(name);
            }
            continue;
        };
        let exact_safe = exact_stable_path(&path, mirror).unwrap_or(false);
        let uninspectable =
            path_has_symlink_component(&path).unwrap_or(true) || path.canonicalize().is_err();
        let raw_alias = stable_canonical_alias(&path, raw_root).unwrap_or(false);
        if exact_safe {
            audit.safe.push(name);
        } else if raw_alias || name == target {
            audit.owned_unsafe.push(name);
        } else if uninspectable {
            audit.unrelated_uninspectable.push(name);
        }
    }
    audit.all.sort();
    audit.all.dedup();
    audit.safe.sort();
    audit.safe.dedup();
    audit.owned_unsafe.sort();
    audit.owned_unsafe.dedup();
    audit.unrelated_uninspectable.sort();
    audit.unrelated_uninspectable.dedup();
    Ok(audit)
}

fn remove_and_confirm<R: QmdRunner>(
    runner: &QmdOperationRunner<'_, R>,
    names: &[String],
) -> Result<(), String> {
    let mut failures = Vec::new();
    let mut unique = names.to_vec();
    unique.sort();
    unique.dedup();
    for name in unique {
        let _ = runner.run(&["collection", "remove", &name])?;
        let show = runner.run(&["collection", "show", &name])?;
        let list = runner.run(&["collection", "list"])?;
        let show_absent = !show.success;
        let list_absent = list.success
            && parse_qmd_collection_names(&list.stdout).is_ok_and(|names| !names.contains(&name));
        let confirmed = show_absent && list_absent;
        if !confirmed {
            failures.push(name);
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err("one or more unsafe QMD collections could not be confirmed removed".into())
    }
}

#[cfg(test)]
fn disable_qmd_names<R: QmdRunner>(
    runner: &QmdOperationRunner<'_, R>,
    audit: Option<&QmdRegistryAudit>,
    target: &str,
) -> Result<(), String> {
    let mut names = Vec::new();
    if !target.is_empty() {
        names.push(target.to_string());
    }
    if let Some(audit) = audit {
        names.extend(audit.safe.iter().cloned());
        names.extend(audit.owned_unsafe.iter().cloned());
    }
    remove_and_confirm(runner, &names)
}

#[cfg(test)]
fn disable_qmd_names_and_clear<R: QmdRunner>(
    runner: &QmdOperationRunner<'_, R>,
    audit: Option<&QmdRegistryAudit>,
    target: &str,
    lock_directory: &Path,
) -> Result<(), String> {
    disable_qmd_names(runner, audit, target)?;
    clear_qmd_owned_target(lock_directory)
}

#[cfg(test)]
fn refresh_qmd_collection_with_at<R: QmdRunner>(
    config: &Config,
    collection: &str,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> Result<QmdMirrorResult, String> {
    let operation = QmdOperationRunner::new(runner);
    let runner = &operation;
    let _lock = acquire_policy_lock_at_until(lock_directory, QMD_POLICY_LOCK, operation.deadline)
        .map_err(|_| "QMD policy lock failed")?;
    if !qmd_collection_name_is_valid(collection) {
        disable_unconfigured_qmd_locked(config, runner, mirror_path, lock_directory)?;
        return Err("QMD collection target is invalid; prior Minutes QMD was disabled".into());
    }
    let pre_audit = qmd_registry_audit(runner, collection, mirror_path, &config.output_dir).ok();
    if pre_audit
        .as_ref()
        .is_some_and(|audit| !audit.unrelated_uninspectable.is_empty())
    {
        disable_qmd_names_and_clear(runner, pre_audit.as_ref(), collection, lock_directory)?;
        return Err(
            "QMD registry contains an unrelated uninspectable collection; Minutes QMD was disabled"
                .into(),
        );
    }
    let mirror = match rebuild_qmd_policy_mirror_at(config, mirror_path) {
        Ok(mirror) => mirror,
        Err(_) => {
            disable_qmd_names_and_clear(runner, pre_audit.as_ref(), collection, lock_directory)?;
            return Err("QMD mirror rebuild failed; Minutes QMD was disabled".into());
        }
    };
    let audit = match qmd_registry_audit(runner, collection, &mirror.path, &config.output_dir) {
        Ok(audit) => audit,
        Err(_) => {
            disable_qmd_names_and_clear(runner, pre_audit.as_ref(), collection, lock_directory)?;
            return Err("QMD registry attestation failed; Minutes QMD was disabled".into());
        }
    };
    if !audit.unrelated_uninspectable.is_empty() {
        disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
        return Err(
            "QMD registry contains an unrelated uninspectable collection; Minutes QMD was disabled"
                .into(),
        );
    }
    let mut stale_aliases = audit.owned_unsafe.clone();
    stale_aliases.extend(
        audit
            .safe
            .iter()
            .filter(|name| name.as_str() != collection)
            .cloned(),
    );
    if !stale_aliases.is_empty() && remove_and_confirm(runner, &stale_aliases).is_err() {
        disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
        return Err("unsafe QMD aliases could not be removed; Minutes QMD was disabled".into());
    }
    if mirror.files == 0 || !audit.safe.iter().any(|name| name == collection) {
        disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
        return Err(
            "QMD corpus was empty or target registration was unsafe; Minutes QMD was disabled"
                .into(),
        );
    }
    let update = runner.run(&["update", "-c", collection]);
    if !matches!(update, Ok(ref result) if result.success) {
        disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
        return Err("QMD update failed; Minutes QMD was disabled".into());
    }
    if attest_qmd_policy_mirror_at(config, &mirror.path).is_err() {
        disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
        return Err("QMD source policy changed during update; Minutes QMD was disabled".into());
    }
    let post = match qmd_registry_audit(runner, collection, &mirror.path, &config.output_dir) {
        Ok(post) => post,
        Err(_) => {
            disable_qmd_names_and_clear(runner, Some(&audit), collection, lock_directory)?;
            return Err("QMD post-update attestation failed; Minutes QMD was disabled".into());
        }
    };
    if post.safe != vec![collection.to_string()]
        || !post.owned_unsafe.is_empty()
        || !post.unrelated_uninspectable.is_empty()
    {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD registration changed during refresh; Minutes QMD was disabled".into());
    }
    if save_qmd_owned_target(lock_directory, collection).is_err() {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD ownership could not be persisted; Minutes QMD was disabled".into());
    }
    if attest_qmd_policy_mirror_at(config, &mirror.path).is_err() {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD final source attestation failed; Minutes QMD was disabled".into());
    }
    Ok(mirror)
}

#[cfg(test)]
fn register_qmd_collection_with_at<R: QmdRunner>(
    config: &Config,
    collection: &str,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> Result<QmdMirrorResult, String> {
    let operation = QmdOperationRunner::new(runner);
    let runner = &operation;
    let _lock = acquire_policy_lock_at_until(lock_directory, QMD_POLICY_LOCK, operation.deadline)
        .map_err(|_| "QMD policy lock failed")?;
    if !qmd_collection_name_is_valid(collection) {
        disable_unconfigured_qmd_locked(config, runner, mirror_path, lock_directory)?;
        return Err("QMD collection target is invalid; prior Minutes QMD was disabled".into());
    }
    let pre = match qmd_registry_audit(runner, collection, mirror_path, &config.output_dir) {
        Ok(pre) => pre,
        Err(_) => {
            disable_qmd_names_and_clear(runner, None, collection, lock_directory)?;
            return Err("QMD registry attestation failed; Minutes QMD remains disabled".into());
        }
    };
    if !pre.unrelated_uninspectable.is_empty() {
        disable_qmd_names_and_clear(runner, Some(&pre), collection, lock_directory)?;
        return Err(
            "QMD registry contains an unrelated uninspectable collection; Minutes QMD remains disabled"
                .into(),
        );
    }
    let mut remove = pre.owned_unsafe.clone();
    remove.extend(pre.safe.iter().cloned());
    if !remove.is_empty() && remove_and_confirm(runner, &remove).is_err() {
        disable_qmd_names_and_clear(runner, Some(&pre), collection, lock_directory)?;
        return Err(
            "existing QMD registrations could not be removed; Minutes QMD remains disabled".into(),
        );
    }
    let mirror = match rebuild_qmd_policy_mirror_at(config, mirror_path) {
        Ok(mirror) if mirror.files > 0 => mirror,
        _ => {
            disable_qmd_names_and_clear(runner, Some(&pre), collection, lock_directory)?;
            return Err("QMD mirror was unavailable or empty; Minutes QMD remains disabled".into());
        }
    };
    let added = runner.run(&[
        "collection",
        "add",
        mirror.path.to_string_lossy().as_ref(),
        "--name",
        collection,
    ]);
    if !matches!(added, Ok(ref result) if result.success) {
        disable_qmd_names_and_clear(runner, None, collection, lock_directory)?;
        return Err("QMD registration failed; Minutes QMD remains disabled".into());
    }
    let update = runner.run(&["update", "-c", collection]);
    if !matches!(update, Ok(ref result) if result.success) {
        disable_qmd_names_and_clear(runner, None, collection, lock_directory)?;
        return Err("QMD update failed; Minutes QMD was disabled".into());
    }
    if attest_qmd_policy_mirror_at(config, &mirror.path).is_err() {
        disable_qmd_names_and_clear(runner, None, collection, lock_directory)?;
        return Err("QMD source policy changed during update; Minutes QMD was disabled".into());
    }
    let post = match qmd_registry_audit(runner, collection, &mirror.path, &config.output_dir) {
        Ok(post) => post,
        Err(_) => {
            disable_qmd_names_and_clear(runner, Some(&pre), collection, lock_directory)?;
            return Err(
                "QMD post-registration attestation failed; Minutes QMD was disabled".into(),
            );
        }
    };
    if post.safe != vec![collection.to_string()]
        || !post.owned_unsafe.is_empty()
        || !post.unrelated_uninspectable.is_empty()
    {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD post-registration attestation failed; Minutes QMD was disabled".into());
    }
    if save_qmd_owned_target(lock_directory, collection).is_err() {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD ownership could not be persisted; Minutes QMD was disabled".into());
    }
    if attest_qmd_policy_mirror_at(config, &mirror.path).is_err() {
        disable_qmd_names_and_clear(runner, Some(&post), collection, lock_directory)?;
        return Err("QMD final source attestation failed; Minutes QMD was disabled".into());
    }
    Ok(mirror)
}

fn disable_unconfigured_qmd_with_at<R: QmdRunner>(
    config: &Config,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> Result<(), String> {
    disable_qmd_persistence_with_at(config, runner, mirror_path, lock_directory, None)
}

#[cfg(test)]
fn disable_unconfigured_qmd_locked<R: QmdRunner>(
    config: &Config,
    runner: &QmdOperationRunner<'_, R>,
    mirror_path: &Path,
    lock_directory: &Path,
) -> Result<(), String> {
    disable_qmd_persistence_locked(config, runner, mirror_path, lock_directory, None)
}

const MAX_QMD_RETIREMENT_DESCENDANTS: usize = 4096;
const MAX_QMD_RETIREMENT_CANDIDATES: usize = 64;
const MAX_QMD_RETIREMENT_DEPTH: usize = 32;
const MAX_QMD_RETIREMENT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_QMD_RETIREMENT_MARKER_BYTES: u64 = 1024 * 1024;
const MAX_QMD_RETIREMENT_SCAN_BYTES: u64 = 512 * 1024 * 1024;
const QMD_RETIREMENT_DEADLINE: Duration = Duration::from_secs(30);
const QMD_RETIREMENT_CLAIM_RETRIES: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QmdInventoryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct QmdObjectIdentity {
    scope: u64,
    object: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QmdInventoryEntry {
    relative: PathBuf,
    kind: QmdInventoryKind,
    identity: QmdObjectIdentity,
    nlink: u64,
    size: u64,
    hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct QmdPlaintextInventory {
    entries: Vec<QmdInventoryEntry>,
}

impl QmdPlaintextInventory {
    fn entry(&self, relative: &Path) -> Option<&QmdInventoryEntry> {
        self.entries
            .binary_search_by(|entry| entry.relative.as_path().cmp(relative))
            .ok()
            .map(|index| &self.entries[index])
    }
}

struct QmdInventoryBudget {
    entries: usize,
    bytes: u64,
    deadline: Instant,
}

impl QmdInventoryBudget {
    fn new(deadline: Instant) -> Self {
        Self {
            entries: 0,
            bytes: 0,
            deadline,
        }
    }

    fn require_time(&self) -> Result<(), String> {
        if Instant::now() >= self.deadline {
            Err("QMD mirror plaintext cleanup exceeded its time budget".into())
        } else {
            Ok(())
        }
    }

    fn charge_entry(&mut self) -> Result<(), String> {
        self.require_time()?;
        self.entries = self.entries.checked_add(1).ok_or_else(|| {
            "QMD mirror plaintext exceeded the safe cleanup entry budget".to_string()
        })?;
        if self.entries > MAX_QMD_RETIREMENT_DESCENDANTS {
            return Err("QMD mirror plaintext exceeded the safe cleanup entry budget".into());
        }
        Ok(())
    }

    fn charge_bytes(&mut self, bytes: usize) -> Result<(), String> {
        self.require_time()?;
        self.bytes = self.bytes.checked_add(bytes as u64).ok_or_else(|| {
            "QMD mirror plaintext exceeded the safe cleanup byte budget".to_string()
        })?;
        if self.bytes > MAX_QMD_RETIREMENT_SCAN_BYTES {
            return Err("QMD mirror plaintext exceeded the safe cleanup byte budget".into());
        }
        Ok(())
    }
}

struct BoundQmdPlaintextDirectory {
    root: File,
    inventory: QmdPlaintextInventory,
    deadline: Instant,
}

fn qmd_open_directory_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            );
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(std::io::Error::other(
            "QMD retirement candidate is not a directory",
        ));
    }
    Ok(file)
}

fn qmd_sync_directory(path: &Path) -> std::io::Result<()> {
    qmd_open_directory_no_follow(path)?.sync_all()
}

fn qmd_open_file_read_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn qmd_open_directory_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    #[cfg(windows)]
    {
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn qmd_file_handles_match(left: &File, right: &File) -> bool {
    qmd_file_identity_and_links(left)
        .zip(qmd_file_identity_and_links(right))
        .is_some_and(|(left, right)| left.0 == right.0)
}

fn qmd_file_identity_and_links(file: &File) -> Option<(QmdObjectIdentity, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = file.metadata().ok()?;
        Some((
            QmdObjectIdentity {
                scope: metadata.dev(),
                object: metadata.ino(),
            },
            metadata.nlink(),
        ))
    }
    #[cfg(windows)]
    {
        use std::mem::zeroed;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
        };
        let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut info) } == 0 {
            return None;
        }
        return Some((
            QmdObjectIdentity {
                scope: u64::from(info.dwVolumeSerialNumber),
                object: (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            },
            u64::from(info.nNumberOfLinks),
        ));
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = file;
        None
    }
}

fn qmd_source_revision_from_open_file(
    file: &mut File,
    budget: &mut QmdInventoryBudget,
) -> Result<(String, u64), String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|_| "QMD mirror plaintext file could not be hashed")?;
    let mut digest = Sha256::new();
    digest.update(b"minutes-source-revision\0");
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "QMD mirror plaintext file could not be hashed")?;
        if count == 0 {
            break;
        }
        budget.charge_bytes(count)?;
        total = total
            .checked_add(count as u64)
            .ok_or_else(|| "QMD mirror plaintext file exceeded its byte budget".to_string())?;
        if total > MAX_QMD_RETIREMENT_FILE_BYTES {
            return Err("QMD mirror plaintext file exceeded its byte budget".into());
        }
        digest.update(&buffer[..count]);
    }
    Ok((format!("sha256:{:x}", digest.finalize()), total))
}

fn qmd_observe_open_entry(
    relative: PathBuf,
    kind: QmdInventoryKind,
    mut file: File,
    budget: &mut QmdInventoryBudget,
) -> Result<QmdInventoryEntry, String> {
    let metadata = file
        .metadata()
        .map_err(|_| "QMD mirror plaintext could not be inspected")?;
    let expected_kind = match kind {
        QmdInventoryKind::File => metadata.is_file(),
        QmdInventoryKind::Directory => metadata.is_dir(),
    };
    if !expected_kind {
        return Err("QMD mirror plaintext entry changed type".into());
    }
    let (identity, nlink) = qmd_file_identity_and_links(&file)
        .ok_or_else(|| "QMD mirror plaintext identity could not be inspected".to_string())?;
    let size = metadata.len();
    let hash = if kind == QmdInventoryKind::File {
        let max_bytes = if relative == Path::new(QMD_MIRROR_MARKER) {
            MAX_QMD_RETIREMENT_MARKER_BYTES
        } else {
            MAX_QMD_RETIREMENT_FILE_BYTES
        };
        if nlink != 1 || size > max_bytes {
            return Err("QMD mirror plaintext file was not uniquely bounded".into());
        }
        let (first, first_size) = qmd_source_revision_from_open_file(&mut file, budget)?;
        let (second, second_size) = qmd_source_revision_from_open_file(&mut file, budget)?;
        let after = file
            .metadata()
            .map_err(|_| "QMD mirror plaintext could not be inspected")?;
        let (after_identity, after_nlink) = qmd_file_identity_and_links(&file)
            .ok_or_else(|| "QMD mirror plaintext identity could not be inspected".to_string())?;
        if first != second
            || first_size != size
            || second_size != size
            || after.len() != size
            || after_identity != identity
            || after_nlink != nlink
        {
            return Err("QMD mirror plaintext changed while it was inventoried".into());
        }
        Some(first)
    } else {
        None
    };
    Ok(QmdInventoryEntry {
        relative,
        kind,
        identity,
        nlink,
        size,
        hash,
    })
}

fn qmd_build_plaintext_inventory(
    root: &File,
    deadline: Instant,
) -> Result<QmdPlaintextInventory, String> {
    fn visit(
        directory: &CapDir,
        prefix: &Path,
        depth: usize,
        inventory: &mut Vec<QmdInventoryEntry>,
        budget: &mut QmdInventoryBudget,
    ) -> Result<(), String> {
        if depth >= MAX_QMD_RETIREMENT_DEPTH {
            return Err("QMD mirror plaintext exceeded the safe cleanup depth".into());
        }
        budget.require_time()?;
        let mut entries = Vec::new();
        for entry in directory
            .entries()
            .map_err(|_| "QMD mirror plaintext could not be enumerated")?
        {
            budget.charge_entry()?;
            entries.push(
                entry
                    .map_err(|_| "QMD mirror plaintext could not be enumerated")?
                    .file_name(),
            );
        }
        entries.sort();
        for name in entries {
            let relative = prefix.join(&name);
            let metadata = directory
                .symlink_metadata(&name)
                .map_err(|_| "QMD mirror plaintext could not be inspected")?;
            if metadata.file_type().is_symlink() {
                return Err("QMD mirror plaintext contained a symbolic link".into());
            }
            if metadata.is_dir() {
                let opened = directory
                    .open_with(&name, &qmd_open_directory_options())
                    .map(|file| file.into_std())
                    .map_err(|_| "QMD mirror plaintext directory could not be opened safely")?;
                inventory.push(qmd_observe_open_entry(
                    relative.clone(),
                    QmdInventoryKind::Directory,
                    opened
                        .try_clone()
                        .map_err(|_| "QMD mirror plaintext directory could not be inspected")?,
                    budget,
                )?);
                visit(
                    &CapDir::from_std_file(opened),
                    &relative,
                    depth + 1,
                    inventory,
                    budget,
                )?;
            } else if metadata.is_file() {
                let opened = directory
                    .open_with(&name, &qmd_open_file_read_options())
                    .map(|file| file.into_std())
                    .map_err(|_| "QMD mirror plaintext file could not be opened safely")?;
                inventory.push(qmd_observe_open_entry(
                    relative,
                    QmdInventoryKind::File,
                    opened,
                    budget,
                )?);
            } else {
                return Err("QMD mirror plaintext contained an unsupported entry".into());
            }
        }
        Ok(())
    }

    let directory = CapDir::from_std_file(
        root.try_clone()
            .map_err(|_| "QMD mirror plaintext root could not be inspected")?,
    );
    let mut entries = Vec::new();
    let mut budget = QmdInventoryBudget::new(deadline);
    visit(&directory, Path::new(""), 0, &mut entries, &mut budget)?;
    entries.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(QmdPlaintextInventory { entries })
}

fn qmd_marker_relative_path(key: &str) -> Result<PathBuf, String> {
    if key.is_empty() || key.contains('\\') {
        return Err("QMD mirror ownership marker contained an invalid source path".into());
    }
    let path = PathBuf::from(key);
    if path.is_absolute()
        || path.extension().and_then(|extension| extension.to_str()) != Some("md")
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("QMD mirror ownership marker contained an invalid source path".into());
    }
    Ok(path)
}

impl BoundQmdPlaintextDirectory {
    fn open(path: &Path, deadline: Instant) -> Result<Self, String> {
        let lexical = fs::symlink_metadata(path)
            .map_err(|_| "QMD mirror plaintext could not be inspected".to_string())?;
        if lexical.file_type().is_symlink() || !lexical.is_dir() {
            return Err("QMD mirror plaintext was not a safe directory".into());
        }
        let root = qmd_open_directory_no_follow(path)
            .map_err(|_| "QMD mirror plaintext could not be bound".to_string())?;
        let inventory = qmd_build_plaintext_inventory(&root, deadline)?;
        Ok(Self {
            root,
            inventory,
            deadline,
        })
    }

    fn matches_path(&self, path: &Path) -> bool {
        let Ok(root) = qmd_open_directory_no_follow(path) else {
            return false;
        };
        if !qmd_file_handles_match(&self.root, &root) {
            return false;
        }
        qmd_build_plaintext_inventory(&root, self.deadline)
            .is_ok_and(|current| current == self.inventory)
    }

    fn open_parent_capability(
        &self,
        relative: &Path,
        inventory: &QmdPlaintextInventory,
    ) -> Result<(CapDir, OsString), String> {
        let components = relative
            .components()
            .map(|component| match component {
                Component::Normal(name) => Ok(name.to_os_string()),
                _ => Err("QMD mirror plaintext path was invalid".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (leaf, parents) = components
            .split_last()
            .ok_or_else(|| "QMD mirror plaintext path was invalid".to_string())?;
        let mut current = CapDir::from_std_file(
            self.root
                .try_clone()
                .map_err(|_| "QMD mirror plaintext root capability was unavailable")?,
        );
        let mut walked = PathBuf::new();
        for component in parents {
            walked.push(component);
            let expected = inventory
                .entry(&walked)
                .filter(|entry| entry.kind == QmdInventoryKind::Directory)
                .ok_or_else(|| "QMD mirror plaintext parent was not inventoried".to_string())?;
            let opened = current
                .open_with(component, &qmd_open_directory_options())
                .map(|file| file.into_std())
                .map_err(|_| "QMD mirror plaintext parent changed during cleanup")?;
            let (identity, nlink) = qmd_file_identity_and_links(&opened).ok_or_else(|| {
                "QMD mirror plaintext parent identity was unavailable".to_string()
            })?;
            if identity != expected.identity || nlink != expected.nlink {
                return Err("QMD mirror plaintext parent changed during cleanup".into());
            }
            current = CapDir::from_std_file(opened);
        }
        Ok((current, leaf.clone()))
    }

    fn read_reaffirmed_file(
        &self,
        relative: &Path,
        inventory: &QmdPlaintextInventory,
        max_bytes: u64,
    ) -> Result<Vec<u8>, String> {
        let expected = inventory
            .entry(relative)
            .filter(|entry| entry.kind == QmdInventoryKind::File)
            .ok_or_else(|| "QMD mirror plaintext file was not inventoried".to_string())?;
        if expected.size > max_bytes {
            return Err("QMD mirror plaintext file exceeded its byte budget".into());
        }
        let (parent, leaf) = self.open_parent_capability(relative, inventory)?;
        let mut file = parent
            .open_with(&leaf, &qmd_open_file_read_options())
            .map(|file| file.into_std())
            .map_err(|_| "QMD mirror plaintext file changed during cleanup")?;
        let metadata = file
            .metadata()
            .map_err(|_| "QMD mirror plaintext file changed during cleanup")?;
        let (identity, nlink) = qmd_file_identity_and_links(&file)
            .ok_or_else(|| "QMD mirror plaintext file identity was unavailable".to_string())?;
        if !metadata.is_file()
            || identity != expected.identity
            || nlink != 1
            || nlink != expected.nlink
            || metadata.len() != expected.size
        {
            return Err("QMD mirror plaintext file changed during cleanup".into());
        }
        let capacity = usize::try_from(expected.size)
            .map_err(|_| "QMD mirror plaintext file exceeded its addressable byte budget")?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(capacity)
            .map_err(|_| "QMD mirror plaintext allocation failed")?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if Instant::now() >= self.deadline {
                return Err("QMD mirror plaintext cleanup exceeded its time budget".into());
            }
            let count = file
                .read(&mut buffer)
                .map_err(|_| "QMD mirror plaintext file changed during cleanup")?;
            if count == 0 {
                break;
            }
            let projected = bytes
                .len()
                .checked_add(count)
                .ok_or_else(|| "QMD mirror plaintext file length overflowed".to_string())?;
            if projected > capacity {
                return Err("QMD mirror plaintext file changed during cleanup".into());
            }
            bytes.extend_from_slice(&buffer[..count]);
        }
        if u64::try_from(bytes.len()).ok() != Some(expected.size)
            || expected.hash.as_deref() != Some(source_revision_bytes(&bytes).as_str())
            || qmd_file_identity_and_links(&file) != Some((expected.identity, expected.nlink))
            || file.metadata().map(|metadata| metadata.len()).ok() != Some(expected.size)
        {
            return Err("QMD mirror plaintext file changed during cleanup".into());
        }
        Ok(bytes)
    }

    fn validated_ownership_marker(&self) -> Result<QmdMirrorMarker, String> {
        let marker_entry = self
            .inventory
            .entry(Path::new(QMD_MIRROR_MARKER))
            .filter(|entry| entry.kind == QmdInventoryKind::File)
            .ok_or_else(|| "QMD mirror plaintext lacked valid ownership provenance".to_string())?;
        if marker_entry.size > MAX_QMD_RETIREMENT_MARKER_BYTES {
            return Err("QMD mirror ownership marker exceeded its byte budget".into());
        }
        let raw = self.read_reaffirmed_file(
            Path::new(QMD_MIRROR_MARKER),
            &self.inventory,
            MAX_QMD_RETIREMENT_MARKER_BYTES,
        )?;
        let marker: QmdMirrorMarker = serde_json::from_slice(&raw)
            .map_err(|_| "QMD mirror plaintext lacked valid ownership provenance".to_string())?;
        if marker.schema != 2
            || marker.source != "configured-meeting-corpus"
            || marker.policy != "normal-only-strict-frontmatter-no-links"
            || marker.sources.len() + 1 >= MAX_QMD_RETIREMENT_DESCENDANTS
        {
            return Err("QMD mirror plaintext lacked valid ownership provenance".into());
        }

        let mut expected_files = BTreeSet::new();
        let mut expected_directories = BTreeSet::new();
        for (key, expected_hash) in &marker.sources {
            let relative = qmd_marker_relative_path(key)?;
            let entry = self
                .inventory
                .entry(&relative)
                .filter(|entry| entry.kind == QmdInventoryKind::File)
                .ok_or_else(|| "QMD mirror ownership marker was incomplete".to_string())?;
            if entry.hash.as_deref() != Some(expected_hash.as_str()) {
                return Err("QMD mirror ownership marker did not match plaintext bytes".into());
            }
            if !expected_files.insert(relative.clone()) {
                return Err("QMD mirror ownership marker duplicated a source".into());
            }
            let mut parent = relative.parent();
            while let Some(path) = parent.filter(|path| !path.as_os_str().is_empty()) {
                expected_directories.insert(path.to_path_buf());
                parent = path.parent();
            }
        }
        expected_files.insert(PathBuf::from(QMD_MIRROR_MARKER));
        let actual_files = self
            .inventory
            .entries
            .iter()
            .filter(|entry| {
                entry.kind == QmdInventoryKind::File
                    && entry.relative != Path::new(QMD_RETIREMENT_RECEIPT)
            })
            .map(|entry| entry.relative.clone())
            .collect::<BTreeSet<_>>();
        let actual_directories = self
            .inventory
            .entries
            .iter()
            .filter(|entry| entry.kind == QmdInventoryKind::Directory)
            .map(|entry| entry.relative.clone())
            .collect::<BTreeSet<_>>();
        if actual_files != expected_files || actual_directories != expected_directories {
            return Err("QMD mirror ownership marker was incomplete".into());
        }
        Ok(marker)
    }

    fn has_valid_ownership_marker(&self) -> bool {
        self.validated_ownership_marker().is_ok()
    }

    fn has_valid_retirement_receipt(&self) -> bool {
        self.read_reaffirmed_file(
            Path::new(QMD_RETIREMENT_RECEIPT),
            &self.inventory,
            QMD_RETIREMENT_RECEIPT_BYTES.len() as u64,
        )
        .is_ok_and(|raw| raw == QMD_RETIREMENT_RECEIPT_BYTES)
    }

    fn is_retained_private(&self) -> bool {
        let receipt_hash = source_revision_bytes(QMD_RETIREMENT_RECEIPT_BYTES);
        self.has_valid_retirement_receipt()
            && self.has_valid_ownership_marker()
            && self.inventory.entries.iter().all(|entry| {
                entry.kind == QmdInventoryKind::Directory
                    || (entry.nlink == 1
                        && if entry.relative == Path::new(QMD_RETIREMENT_RECEIPT) {
                            entry.size == QMD_RETIREMENT_RECEIPT_BYTES.len() as u64
                                && entry.hash.as_deref() == Some(receipt_hash.as_str())
                        } else {
                            entry.hash.is_some()
                        })
            })
    }

    fn retire(
        &self,
        marker: &QmdMirrorMarker,
        claimed_path: &Path,
        after_file_sanitized: &mut impl FnMut(&Path, &Path),
    ) -> Result<(), String> {
        let mut files = marker
            .sources
            .keys()
            .map(|key| qmd_marker_relative_path(key))
            .collect::<Result<Vec<_>, _>>()?;
        files.sort();
        files.push(PathBuf::from(QMD_MIRROR_MARKER));
        for relative in &files {
            let expected = self
                .inventory
                .entry(relative)
                .filter(|entry| entry.kind == QmdInventoryKind::File)
                .ok_or_else(|| "QMD mirror plaintext file was not inventoried".to_string())?;
            self.read_reaffirmed_file(relative, &self.inventory, expected.size)?;
            after_file_sanitized(claimed_path, relative);
        }

        let current = qmd_build_plaintext_inventory(&self.root, self.deadline)?;
        if current != self.inventory || !self.matches_root_path(claimed_path) {
            return Err(
                "QMD mirror plaintext changed during cleanup; replacement was preserved".into(),
            );
        }

        let directory = CapDir::from_std_file(
            self.root
                .try_clone()
                .map_err(|_| "QMD retirement receipt could not be created")?,
        );
        let mut options = CapOpenOptions::new();
        options.write(true).create_new(true);
        let mut receipt = directory
            .open_with(QMD_RETIREMENT_RECEIPT, &options)
            .map(|file| file.into_std())
            .map_err(|_| "QMD retirement receipt could not be created")?;
        set_restrictive_permissions_file(&receipt)
            .map_err(|_| "QMD retirement receipt permissions failed")?;
        receipt
            .write_all(QMD_RETIREMENT_RECEIPT_BYTES)
            .and_then(|_| receipt.sync_all())
            .and_then(|_| self.root.sync_all())
            .map_err(|_| "QMD retirement receipt could not be persisted")?;

        let retired = Self {
            root: self
                .root
                .try_clone()
                .map_err(|_| "QMD retirement receipt could not be verified")?,
            inventory: qmd_build_plaintext_inventory(&self.root, self.deadline)?,
            deadline: self.deadline,
        };
        let without_receipt = QmdPlaintextInventory {
            entries: retired
                .inventory
                .entries
                .iter()
                .filter(|entry| entry.relative != Path::new(QMD_RETIREMENT_RECEIPT))
                .cloned()
                .collect(),
        };
        if !retired.matches_root_path(claimed_path)
            || without_receipt != self.inventory
            || retired.inventory.entries.len() != self.inventory.entries.len() + 1
            || !retired.has_valid_retirement_receipt()
            || !retired.is_retained_private()
        {
            return Err("QMD retirement receipt could not be verified".into());
        }
        // Intentionally retain the owner-private, byte-bearing claim. Never
        // mutate or unlink a linkable inode after an identity/hash check: a
        // same-UID hard-link race would make in-place sanitization destructive.
        Ok(())
    }

    fn matches_root_path(&self, path: &Path) -> bool {
        qmd_open_directory_no_follow(path)
            .is_ok_and(|current| qmd_file_handles_match(&self.root, &current))
    }
}

fn source_revision_bytes(content: &[u8]) -> String {
    let mut digest = Sha256::new();
    digest.update(b"minutes-source-revision\0");
    digest.update(content);
    format!("sha256:{:x}", digest.finalize())
}

fn qmd_retirement_claim_path(parent: &Path) -> Result<PathBuf, String> {
    let mut random = [0u8; 16];
    getrandom::fill(&mut random)
        .map_err(|_| "QMD retirement claim nonce could not be generated".to_string())?;
    let suffix = random
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(parent.join(format!(".{QMD_MIRROR_DIR}.retired-{suffix}")))
}

#[cfg(test)]
fn purge_qmd_policy_plaintext_at(mirror_path: &Path) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_until(mirror_path, Instant::now() + QMD_RETIREMENT_DEADLINE)
}

fn purge_qmd_policy_plaintext_at_until(
    mirror_path: &Path,
    deadline: Instant,
) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_with_hooks_and_claims(
        mirror_path,
        deadline,
        |_| {},
        |_| {},
        |_, _| {},
        qmd_retirement_claim_path,
    )
}

#[cfg(test)]
fn purge_qmd_policy_plaintext_at_with_hook(
    mirror_path: &Path,
    before_atomic_claim: impl FnMut(&Path),
) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_with_hooks_and_claims(
        mirror_path,
        Instant::now() + QMD_RETIREMENT_DEADLINE,
        before_atomic_claim,
        |_| {},
        |_, _| {},
        qmd_retirement_claim_path,
    )
}

#[cfg(test)]
fn purge_qmd_policy_plaintext_at_with_hooks(
    mirror_path: &Path,
    before_atomic_claim: impl FnMut(&Path),
    after_claim_attestation: impl FnMut(&Path),
) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_with_hooks_and_claims(
        mirror_path,
        Instant::now() + QMD_RETIREMENT_DEADLINE,
        before_atomic_claim,
        after_claim_attestation,
        |_, _| {},
        qmd_retirement_claim_path,
    )
}

#[cfg(test)]
fn purge_qmd_policy_plaintext_at_with_retirement_hook(
    mirror_path: &Path,
    after_file_sanitized: impl FnMut(&Path, &Path),
) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_with_hooks_and_claims(
        mirror_path,
        Instant::now() + QMD_RETIREMENT_DEADLINE,
        |_| {},
        |_| {},
        after_file_sanitized,
        qmd_retirement_claim_path,
    )
}

#[cfg(test)]
fn purge_qmd_policy_plaintext_at_with_claim_paths(
    mirror_path: &Path,
    next_claim_path: impl FnMut(&Path) -> Result<PathBuf, String>,
) -> Result<(), String> {
    purge_qmd_policy_plaintext_at_with_hooks_and_claims(
        mirror_path,
        Instant::now() + QMD_RETIREMENT_DEADLINE,
        |_| {},
        |_| {},
        |_, _| {},
        next_claim_path,
    )
}

fn purge_qmd_policy_plaintext_at_with_hooks_and_claims(
    mirror_path: &Path,
    deadline: Instant,
    mut before_atomic_claim: impl FnMut(&Path),
    mut after_claim_attestation: impl FnMut(&Path),
    mut after_file_sanitized: impl FnMut(&Path, &Path),
    mut next_claim_path: impl FnMut(&Path) -> Result<PathBuf, String>,
) -> Result<(), String> {
    let Some(parent) = mirror_path.parent() else {
        return Err("QMD mirror has no parent directory".into());
    };
    let mut candidates = vec![mirror_path.to_path_buf()];
    match fs::read_dir(parent) {
        Ok(entries) => {
            for (inspected_entries, entry) in entries.enumerate() {
                if Instant::now() >= deadline || inspected_entries >= MAX_QMD_RETIREMENT_DESCENDANTS
                {
                    return Err("QMD mirror cleanup exceeded its enumeration budget".into());
                }
                let entry = entry.map_err(|_| "QMD mirror cleanup could not be enumerated")?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with(&format!(".{QMD_MIRROR_DIR}.staging-"))
                    || name.starts_with(&format!(".{QMD_MIRROR_DIR}.previous-"))
                    || name.starts_with(&format!(".{QMD_MIRROR_DIR}.retired-"))
                {
                    if candidates.len() >= MAX_QMD_RETIREMENT_CANDIDATES {
                        return Err("QMD mirror cleanup exceeded its candidate budget".into());
                    }
                    candidates.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err("QMD mirror cleanup directory could not be inspected".into()),
    }

    let mut bound = Vec::new();
    let mut retained_inventory_entries = 0usize;
    let mut retained_inventory_bytes = 0u64;
    for candidate in candidates {
        match fs::symlink_metadata(&candidate) {
            Ok(_) => {
                let is_retired_residue = candidate
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!(".{QMD_MIRROR_DIR}.retired-")));
                let bound_candidate = BoundQmdPlaintextDirectory::open(&candidate, deadline)?;
                retained_inventory_entries = retained_inventory_entries
                    .checked_add(bound_candidate.inventory.entries.len())
                    .ok_or_else(|| {
                        "QMD mirror cleanup exceeded its retained inventory budget".to_string()
                    })?;
                if retained_inventory_entries > MAX_QMD_RETIREMENT_DESCENDANTS {
                    return Err("QMD mirror cleanup exceeded its retained inventory budget".into());
                }
                retained_inventory_bytes = bound_candidate
                    .inventory
                    .entries
                    .iter()
                    .filter(|entry| entry.kind == QmdInventoryKind::File)
                    .try_fold(retained_inventory_bytes, |total, entry| {
                        total.checked_add(entry.size)
                    })
                    .ok_or_else(|| {
                        "QMD mirror cleanup exceeded its retained byte budget".to_string()
                    })?;
                if retained_inventory_bytes > MAX_QMD_RETIREMENT_SCAN_BYTES / 2 {
                    return Err("QMD mirror cleanup exceeded its retained byte budget".into());
                }
                if !is_retired_residue && !bound_candidate.has_valid_ownership_marker() {
                    return Err(
                        "QMD mirror plaintext lacked valid ownership provenance; it was preserved"
                            .into(),
                    );
                }
                bound.push((candidate.clone(), bound_candidate, is_retired_residue));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err("QMD mirror plaintext could not be inspected".into()),
        }
    }

    for (candidate, bound, is_retired_residue) in bound {
        if is_retired_residue {
            if !bound.matches_path(&candidate) {
                return Err("QMD retirement residue could not be re-attested".into());
            }
            after_claim_attestation(&candidate);
            if !bound.matches_path(&candidate)
                || !bound.has_valid_retirement_receipt()
                || !bound.is_retained_private()
            {
                return Err("QMD retirement residue lacked valid ownership provenance".into());
            }
            continue;
        }
        before_atomic_claim(&candidate);
        let mut claimed = None;
        for _ in 0..QMD_RETIREMENT_CLAIM_RETRIES {
            let candidate_claim = next_claim_path(parent)?;
            match crate::policy_fs::move_entry_no_replace(&candidate, &candidate_claim) {
                Ok(()) => {
                    claimed = Some(candidate_claim);
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return Err("QMD mirror plaintext could not be atomically claimed".into()),
            }
        }
        let Some(claimed) = claimed else {
            return Err("QMD mirror plaintext claim retry budget was exhausted".into());
        };
        if !bound.matches_path(&claimed) {
            return Err(
                "QMD mirror plaintext changed before cleanup; replacement was preserved".into(),
            );
        }
        after_claim_attestation(&claimed);
        if !bound.matches_path(&claimed) {
            return Err(
                "QMD mirror plaintext changed before cleanup; replacement was preserved".into(),
            );
        }
        let marker = bound.validated_ownership_marker()?;
        bound.retire(&marker, &claimed, &mut after_file_sanitized)?;
        qmd_sync_directory(parent)
            .map_err(|_| "QMD mirror cleanup could not be synced".to_string())?;
    }
    qmd_sync_directory(parent).map_err(|_| "QMD mirror cleanup could not be synced".to_string())
}

/// Outcome of a retraction attempt, keeping the reason it failed.
struct QmdRetractionOutcome {
    result: Result<(), String>,
    /// No `qmd` command ever came back successful, so the registry told us
    /// nothing at all: either it is not installed, or it is installed and will
    /// not answer. Nothing was inspectable, as opposed to inspected and found
    /// wanting.
    registry_never_answered: bool,
    /// Whether this machine showed any sign Minutes had registered a
    /// collection, read before the retraction below started deleting the
    /// things that constitute the evidence.
    registration_evidence: bool,
}

fn disable_qmd_persistence_reporting_at<R: QmdRunner>(
    config: &Config,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
    configured_target: Option<&str>,
) -> QmdRetractionOutcome {
    let operation = QmdOperationRunner::new(runner);
    let lock = acquire_policy_lock_at_until(lock_directory, QMD_POLICY_LOCK, operation.deadline);
    let Ok(_lock) = lock else {
        return QmdRetractionOutcome {
            result: Err("QMD policy lock failed".to_string()),
            registry_never_answered: false,
            registration_evidence: true,
        };
    };
    // Read the evidence under the policy lock, before the retraction below
    // starts deleting the mirror directories that constitute it. Reading it
    // outside the lock would be reading it against a machine another retraction
    // may be halfway through changing.
    let registration_evidence = qmd_registration_evidence_at(config, mirror_path, lock_directory);
    let result = disable_qmd_persistence_locked(
        config,
        &operation,
        mirror_path,
        lock_directory,
        configured_target,
    );
    let registry_never_answered = operation.registry_never_answered();
    QmdRetractionOutcome {
        result,
        registry_never_answered,
        registration_evidence,
    }
}

fn disable_qmd_persistence_with_at<R: QmdRunner>(
    config: &Config,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
    configured_target: Option<&str>,
) -> Result<(), String> {
    // One deadline and one command counter cover the initial audit, every
    // retraction/confirmation, and the final audit. Waiting for the policy
    // lock consumes the same budget, so lock contention cannot silently reset
    // a later subprocess deadline.
    let operation = QmdOperationRunner::new(runner);
    let _lock = acquire_policy_lock_at_until(lock_directory, QMD_POLICY_LOCK, operation.deadline)
        .map_err(|_| "QMD policy lock failed")?;
    disable_qmd_persistence_locked(
        config,
        &operation,
        mirror_path,
        lock_directory,
        configured_target,
    )
}

fn disable_qmd_persistence_locked<R: QmdRunner>(
    config: &Config,
    runner: &QmdOperationRunner<'_, R>,
    mirror_path: &Path,
    lock_directory: &Path,
    configured_target: Option<&str>,
) -> Result<(), String> {
    let registry_result = (|| {
        let persisted_target = load_qmd_owned_target(lock_directory)?;
        let audit_target = configured_target
            .or(persisted_target.as_deref())
            .unwrap_or("");
        let audit = qmd_registry_audit(runner, audit_target, mirror_path, &config.output_dir)?;
        let mut owned = audit.safe.clone();
        owned.extend(audit.owned_unsafe.iter().cloned());
        owned.extend(configured_target.map(str::to_owned));
        owned.extend(persisted_target.iter().cloned());
        remove_and_confirm(runner, &owned)?;

        let post = qmd_registry_audit(runner, audit_target, mirror_path, &config.output_dir)?;
        let configured_absent = configured_target
            .is_none_or(|target| post.all.iter().all(|registered| registered != target));
        let persisted_absent = persisted_target
            .as_ref()
            .is_none_or(|target| !post.all.contains(target));
        if !post.safe.is_empty()
            || !post.owned_unsafe.is_empty()
            || !post.unrelated_uninspectable.is_empty()
            || !configured_absent
            || !persisted_absent
        {
            return Err("Minutes-owned QMD registrations could not be confirmed disabled".into());
        }
        clear_qmd_owned_target(lock_directory)
    })();
    let plaintext_result = purge_qmd_policy_plaintext_at_until(mirror_path, runner.deadline);

    match (registry_result, plaintext_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
    }
}

fn disable_unconfigured_qmd(config: &Config) -> Result<(), String> {
    disable_unconfigured_qmd_with_at(
        config,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
    )
}

/// Confirm that every identifiable Minutes-owned QMD registration is removed,
/// then delete its ownership marker and plaintext mirror. This cleanup-only
/// API succeeds after confirmed retraction and never registers or updates QMD.
/// Registry inspection is mandatory even without a marker or mirror so legacy
/// raw-output aliases cannot remain queryable. If QMD cannot be inspected, the
/// plaintext cleanup is still attempted but the attestation fails closed.
/// Whether the retraction succeeded, and if not, whether the only obstacle was
/// that `qmd` is not installed.
pub struct QmdRetractionStatus {
    pub confirmed: bool,
    /// Nothing could be inspected because there is no `qmd` on this machine.
    /// Distinct from an inspection that ran and could not confirm.
    pub registry_never_answered: bool,
    /// Whether this machine showed any sign Minutes had registered a
    /// collection. Callers deciding that a cleanup is confirmed must require
    /// this to be false whenever they are relying on
    /// [`Self::registry_never_answered`], or they will report success on a
    /// machine that agent readiness is still blocking.
    pub registration_evidence: bool,
    pub error: Option<String>,
}

/// Retract persistence and report why it failed, so a caller can tell "qmd is
/// installed and something is wrong" from "there is no qmd here" (#788).
pub fn ensure_qmd_persistence_disabled_status(config: &Config) -> QmdRetractionStatus {
    let outcome = disable_qmd_persistence_reporting_at(
        config,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
        config.search.qmd_collection.as_deref(),
    );
    match outcome.result {
        Ok(()) => QmdRetractionStatus {
            confirmed: true,
            registry_never_answered: outcome.registry_never_answered,
            registration_evidence: outcome.registration_evidence,
            error: None,
        },
        Err(error) => QmdRetractionStatus {
            confirmed: false,
            registry_never_answered: outcome.registry_never_answered,
            registration_evidence: outcome.registration_evidence,
            error: Some(error),
        },
    }
}

pub fn ensure_qmd_persistence_disabled(config: &Config) -> Result<(), String> {
    disable_qmd_persistence_with_at(
        config,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
        config.search.qmd_collection.as_deref(),
    )
}

/// Path of the QMD ownership marker, for tests that need to plant one from
/// outside this crate.
#[doc(hidden)]
pub fn qmd_owned_target_path_for_tests() -> PathBuf {
    Config::minutes_dir().join(QMD_OWNED_TARGET)
}

/// How many directory entries the residue scan will read before it gives up
/// and answers "assume there is residue".
///
/// Deliberately far above any plausible `~/.minutes`, because the cost of
/// hitting it is a machine that stays blocked, and the cost of scanning past a
/// real one would be a machine that reports clean without having looked.
const MAX_QMD_EVIDENCE_ENTRIES: usize = 8192;

/// Whether a mirror transaction directory is sitting next to the live mirror.
///
/// A run that died mid-retirement leaves its staging, previous, or retired
/// directory behind, holding the same plaintext the live mirror does, so it
/// means the same thing about whether a registration ever happened.
///
/// Every uncertain answer here is "yes". A directory we cannot open, an entry
/// we cannot read, or more entries than we are willing to walk all resolve to
/// residue, because this feeds a gate that lets agent tools run and the only
/// safe direction to be wrong in is towards blocking.
fn qmd_mirror_residue_evidence(mirror_path: &Path) -> bool {
    qmd_mirror_residue_evidence_within(mirror_path, MAX_QMD_EVIDENCE_ENTRIES)
}

/// [`qmd_mirror_residue_evidence`] with the walk bound supplied, so the
/// give-up rule can be tested without materializing the real bound.
fn qmd_mirror_residue_evidence_within(mirror_path: &Path, limit: usize) -> bool {
    let Some(parent) = mirror_path.parent() else {
        return true;
    };
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let prefixes = [
        format!(".{QMD_MIRROR_DIR}.staging-"),
        format!(".{QMD_MIRROR_DIR}.previous-"),
        format!(".{QMD_MIRROR_DIR}.retired-"),
    ];
    for (inspected, entry) in entries.enumerate() {
        if inspected >= limit {
            return true;
        }
        let Ok(entry) = entry else {
            return true;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if prefixes.iter().any(|prefix| name.starts_with(prefix)) {
            return true;
        }
    }
    false
}

/// Whether anything on this machine suggests Minutes ever registered a QMD
/// collection. All three signals are Minutes-owned and readable without QMD.
///
/// This exists because the retirement audit fails closed on *any* QMD problem:
/// absent, broken native bindings, or the non-zero Apple Silicon cleanup exits
/// reported upstream. That is correct when Minutes might have left a collection
/// behind, and indefensible when it provably did not, because the remediation
/// we print runs the same audit and fails the same way, leaving no route out
/// at all (#788).
///
/// A marker, a mirror, or a QMD reference in config each mean "we may have
/// registered something, and only QMD can say whether it is still there". None
/// of them present means there is nothing to revoke, whatever state QMD is in.
fn qmd_registration_evidence_at(
    config: &Config,
    mirror_path: &Path,
    lock_directory: &Path,
) -> bool {
    let marker_present = !matches!(
        fs::symlink_metadata(lock_directory.join(QMD_OWNED_TARGET)),
        Err(ref error) if error.kind() == std::io::ErrorKind::NotFound
    );
    let mirror_present = !matches!(
        fs::symlink_metadata(mirror_path),
        Err(ref error) if error.kind() == std::io::ErrorKind::NotFound
    );
    let residue_present = qmd_mirror_residue_evidence(mirror_path);
    let configured =
        config.search.engine.eq_ignore_ascii_case("qmd") || config.search.qmd_collection.is_some();
    marker_present || mirror_present || residue_present || configured
}

fn evaluate_agent_trust_readiness_with_at<R: QmdRunner>(
    config: &Config,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> AgentTrustReadiness {
    let outcome = disable_qmd_persistence_reporting_at(
        config,
        runner,
        mirror_path,
        lock_directory,
        config.search.qmd_collection.as_deref(),
    );
    match outcome.result {
        Ok(()) => match clear_qmd_retirement_pending(lock_directory) {
            Ok(()) => AgentTrustReadiness::ready(QmdRetirementReadiness::ReadyClean),
            Err(_) => AgentTrustReadiness::blocked(),
        },
        Err(_) if outcome.registry_never_answered && !outcome.registration_evidence => {
            // Two things are true at once here: the registry told us nothing
            // (no qmd, or a qmd that will not answer), and nothing on this
            // machine says Minutes ever registered a collection. There is no
            // copy to revoke and no way to look for one, so there is nothing
            // here for the block to protect (#788).
            //
            // The pending marker is kept deliberately. Readiness is
            // re-evaluated on every agent call, so if qmd later appears or
            // starts answering, the audit runs again and can still block. This
            // defers an unanswerable question rather than answering it.
            let _ = mark_qmd_retirement_pending(lock_directory);
            AgentTrustReadiness::ready(QmdRetirementReadiness::ReadyClean)
        }
        Err(_) => {
            // Either the registry answered and we still could not confirm the
            // collection is gone, or this machine shows a registration Minutes
            // made. Both are live questions about a copy that may exist, and
            // both still fail closed.
            let _ = mark_qmd_retirement_pending(lock_directory);
            AgentTrustReadiness::blocked()
        }
    }
}

fn evaluate_agent_trust_readiness_from_strict_config_with_at<R: QmdRunner>(
    strict_config: Result<Config, String>,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> AgentTrustReadiness {
    match strict_config {
        Ok(config) => {
            evaluate_agent_trust_readiness_with_at(&config, runner, mirror_path, lock_directory)
        }
        Err(_) => AgentTrustReadiness::blocked(),
    }
}

/// Establish the process-wide agent readiness boundary before any Recall,
/// CLI-hosted agent, or MCP transport is advertised as ready.
pub fn establish_agent_trust_readiness(config: &Config) -> AgentTrustReadiness {
    let evaluated = evaluate_agent_trust_readiness_with_at(
        config,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
    );
    AGENT_TRUST_BOUNDARY_ESTABLISHED.store(true, Ordering::Release);
    evaluated
}

/// Establish readiness from the authoritative on-disk policy. An existing
/// unreadable or malformed config is itself a blocking trust state: falling
/// back to defaults could erase a configured legacy QMD target before its
/// registry entry has been retired.
pub fn establish_agent_trust_readiness_from_strict_config() -> AgentTrustReadiness {
    let evaluated = revalidate_agent_trust_readiness_from_strict_config();
    AGENT_TRUST_BOUNDARY_ESTABLISHED.store(true, Ordering::Release);
    evaluated
}

/// Revalidate the external QMD registry against the authoritative policy.
///
/// This intentionally performs a fresh bounded registry audit. Local marker or
/// plaintext absence is not an attestation of QMD's external registry state,
/// and no process-lifetime result is safe to reuse after another process may
/// have changed that registry.
pub fn revalidate_agent_trust_readiness_from_strict_config() -> AgentTrustReadiness {
    evaluate_agent_trust_readiness_from_strict_config_with_at(
        Config::load_strict(),
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
    )
}

/// Returns `None` only in callers (primarily unit tests) that have not crossed
/// a trusted application startup boundary yet. Once established, every call
/// performs a fresh bounded registry audit before agent content is admitted.
pub fn current_agent_trust_readiness() -> Option<AgentTrustReadiness> {
    AGENT_TRUST_BOUNDARY_ESTABLISHED
        .load(Ordering::Acquire)
        .then(revalidate_agent_trust_readiness_from_strict_config)
}

fn reject_persistent_qmd_with_at<R: QmdRunner>(
    config: &Config,
    collection: &str,
    runner: &R,
    mirror_path: &Path,
    lock_directory: &Path,
) -> Result<QmdMirrorResult, String> {
    disable_qmd_persistence_with_at(
        config,
        runner,
        mirror_path,
        lock_directory,
        Some(collection),
    )?;
    Err(QMD_PERSISTENCE_DISABLED_REASON.into())
}

pub fn register_qmd_policy_collection(
    config: &Config,
    collection: &str,
) -> Result<QmdMirrorResult, String> {
    reject_persistent_qmd_with_at(
        config,
        collection,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
    )
}

/// Retract a formerly configured persistent QMD collection and report that the
/// persistent integration is disabled. Search must instead use a query-scoped
/// source whose registration and index are destroyed before the call returns.
pub fn refresh_qmd_collection(config: &Config) -> Result<QmdMirrorResult, String> {
    let Some(collection) = config.search.qmd_collection.as_deref() else {
        return Err("no QMD collection is configured".into());
    };
    reject_persistent_qmd_with_at(
        config,
        collection,
        &SystemQmdRunner,
        &qmd_policy_mirror_path(),
        &Config::minutes_dir(),
    )
}

fn generated_wiki_source(line: &str) -> Option<&str> {
    if !line.starts_with("- ") || !line.contains(" *(") || !line.ends_with(")*") {
        return None;
    }
    line.rsplit_once(" — ")
        .and_then(|(_, source)| source.strip_suffix(")*"))
}

struct BoundedString {
    value: String,
    max_bytes: usize,
}

impl BoundedString {
    fn new(max_bytes: u64, initial_capacity: usize) -> Result<Self, Box<dyn std::error::Error>> {
        let max_bytes =
            usize::try_from(max_bytes).map_err(|_| "bounded text is not addressable")?;
        let mut value = String::new();
        value
            .try_reserve_exact(initial_capacity.min(max_bytes))
            .map_err(|_| "bounded text allocation failed")?;
        Ok(Self { value, max_bytes })
    }

    fn push_str(&mut self, value: &str) -> Result<(), Box<dyn std::error::Error>> {
        let projected = self
            .value
            .len()
            .checked_add(value.len())
            .ok_or("bounded text length overflowed")?;
        if projected > self.max_bytes {
            return Err("knowledge reconciliation output exceeded its byte bound".into());
        }
        self.value
            .try_reserve(value.len())
            .map_err(|_| "bounded text allocation failed")?;
        self.value.push_str(value);
        Ok(())
    }

    fn push_char(&mut self, value: char) -> Result<(), Box<dyn std::error::Error>> {
        let mut encoded = [0u8; 4];
        self.push_str(value.encode_utf8(&mut encoded))
    }

    fn push_single_line(&mut self, value: &str) -> Result<(), Box<dyn std::error::Error>> {
        let mut first = true;
        for word in value.split_whitespace() {
            if !first {
                self.push_char(' ')?;
            }
            self.push_str(word)?;
            first = false;
        }
        Ok(())
    }

    fn into_string(self) -> String {
        self.value
    }
}

struct BoundedVecWriter {
    bytes: Vec<u8>,
    max_bytes: usize,
}

impl BoundedVecWriter {
    fn new(max_bytes: u64) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            bytes: Vec::new(),
            max_bytes: usize::try_from(max_bytes)
                .map_err(|_| "bounded JSON output is not addressable")?,
        })
    }
}

impl Write for BoundedVecWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let projected = self
            .bytes
            .len()
            .checked_add(buffer.len())
            .ok_or_else(|| std::io::Error::other("bounded JSON length overflowed"))?;
        if projected > self.max_bytes {
            return Err(std::io::Error::other(
                "knowledge reconciliation JSON exceeded its byte bound",
            ));
        }
        self.bytes
            .try_reserve(buffer.len())
            .map_err(|_| std::io::Error::other("bounded JSON allocation failed"))?;
        self.bytes.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn bounded_json_to_vec_pretty<T: Serialize + ?Sized>(
    value: &T,
    max_bytes: u64,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let mut writer = BoundedVecWriter::new(max_bytes)?;
    serde_json::to_writer_pretty(&mut writer, value)?;
    Ok(writer.bytes)
}

fn record_id_for_json(
    kind: &str,
    value: &serde_json::Value,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut writer = BoundedVecWriter::new(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
    serde_json::to_writer(&mut writer, value)?;
    let mut digest = Sha256::new();
    digest.update(kind.as_bytes());
    digest.update([0]);
    digest.update(&writer.bytes);
    Ok(format!("{kind}:{:x}", digest.finalize()))
}

fn for_each_wiki_chunk(
    content: &str,
    mut visit: impl FnMut(&str) -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut offset = 0usize;
    let mut fact_start: Option<usize> = None;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset = offset
            .checked_add(line.len())
            .ok_or("wiki reconciliation offset overflowed")?;
        let begins_boundary = line.starts_with("- ") || line.starts_with("## ");
        if begins_boundary {
            if let Some(start) = fact_start.take() {
                visit(&content[start..line_start])?;
            }
        }
        if line.starts_with("- ") || fact_start.is_some() {
            let start = *fact_start.get_or_insert(line_start);
            if generated_wiki_source(content[start..offset].trim_end_matches(['\r', '\n']))
                .is_some()
            {
                visit(&content[start..offset])?;
                fact_start = None;
            }
        } else {
            visit(line)?;
        }
    }
    if let Some(start) = fact_start {
        visit(&content[start..])?;
    }
    Ok(())
}

fn knowledge_log_path(knowledge: &KnowledgeConfig) -> PathBuf {
    let base = if knowledge.adapter.eq_ignore_ascii_case("para") {
        knowledge.path.join("memory")
    } else {
        knowledge.path.clone()
    };
    base.join(&knowledge.log_file)
}

fn managed_log_relative_path(config: &Config) -> Option<String> {
    let path = knowledge_log_path(&config.knowledge);
    let relative = path.strip_prefix(&config.knowledge.path).ok()?;
    if !path_has_only_normal_components(relative) || relative.extension()? != "md" {
        return None;
    }
    relative
        .to_str()
        .map(|relative| relative.replace('\\', "/"))
}

fn managed_log_path(config: &Config, relative: &str) -> Option<PathBuf> {
    let relative = Path::new(relative);
    if !path_has_only_normal_components(relative) || relative.extension()? != "md" {
        return None;
    }
    Some(config.knowledge.path.join(relative))
}

fn log_section_source(section: &str) -> Option<&str> {
    section.lines().find_map(|line| {
        line.strip_prefix("- Source: `")
            .and_then(|value| value.strip_suffix('`'))
    })
}

fn remove_empty_wiki_sections(content: &str) -> Result<String, Box<dyn std::error::Error>> {
    let mut output = BoundedString::new(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES, content.len())?;
    let mut offset = 0usize;
    let mut section_start = None;
    let mut section_body_start = 0usize;
    for line in content.split_inclusive('\n') {
        let line_start = offset;
        offset = offset
            .checked_add(line.len())
            .ok_or("wiki section offset overflowed")?;
        if line.starts_with("## ") {
            if let Some(start) = section_start {
                if !content[section_body_start..line_start].trim().is_empty() {
                    output.push_str(&content[start..line_start])?;
                }
            } else {
                output.push_str(&content[..line_start])?;
            }
            section_start = Some(line_start);
            section_body_start = offset;
        }
    }
    if let Some(start) = section_start {
        if !content[section_body_start..].trim().is_empty() {
            output.push_str(&content[start..])?;
        }
    } else {
        output.push_str(content)?;
    }
    Ok(output.into_string())
}

fn wiki_profile_has_derived_content(content: &str) -> bool {
    content
        .lines()
        .enumerate()
        .any(|(index, line)| index > 0 && !line.trim().is_empty())
}

fn rewrite_wiki_people<F>(
    config: &Config,
    disposition: F,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
{
    rewrite_wiki_people_with_hook(config, disposition, |_| {})
}

fn rewrite_wiki_people_with_hook<F, H>(
    config: &Config,
    mut disposition: F,
    mut before_mutation: H,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
    H: FnMut(&Path),
{
    let knowledge = &config.knowledge;
    let people = knowledge.path.join("people");
    if !people.exists() {
        return Ok(0);
    }
    set_restrictive_directory_permissions(&knowledge.path)?;
    set_restrictive_directory_permissions(&people)?;
    let mut removed = 0usize;
    for entry in fs::read_dir(&people)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            let identity = preserve_file_before_retraction(config, &entry.path())?;
            remove_preserved_file(config, &entry.path(), &identity)?;
            removed += 1;
        }
    }
    for entry in WalkDir::new(&people)
        .max_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "md")
        })
    {
        let mut identity = preserved_source_identity_with_limit(
            entry.path(),
            MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
        )?;
        let content = match std::str::from_utf8(&identity.bytes) {
            Ok(content) => content,
            Err(_) => {
                identity = preserve_bound_file_before_retraction(config, entry.path(), identity)?;
                before_mutation(entry.path());
                remove_preserved_file(config, entry.path(), &identity)?;
                removed += 1;
                continue;
            }
        };
        let mut rewritten =
            BoundedString::new(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES, content.len())?;
        let mut quarantine = false;
        let mut saw_record = false;
        for_each_wiki_chunk(content, |chunk| {
            let trimmed = chunk.trim_end_matches(['\r', '\n']);
            let source = generated_wiki_source(trimmed);
            if let Some(source) = source {
                saw_record = true;
                match disposition(source, &record_id("wiki", trimmed)) {
                    RecordDisposition::Keep => rewritten.push_str(chunk)?,
                    RecordDisposition::RemoveOwned => removed += 1,
                    RecordDisposition::Quarantine => {
                        quarantine = true;
                        removed += 1;
                    }
                }
            } else if trimmed.is_empty()
                || trimmed.starts_with("# ")
                || matches!(
                    trimmed,
                    "## Decision"
                        | "## Commitment"
                        | "## Context"
                        | "## Preference"
                        | "## Relationship"
                )
            {
                rewritten.push_str(chunk)?;
            } else {
                quarantine = true;
                removed += 1;
            }
            Ok(())
        })?;
        if !saw_record {
            quarantine = true;
        }
        let rewritten = remove_empty_wiki_sections(&rewritten.into_string())?;
        let has_derived_content = wiki_profile_has_derived_content(&rewritten);
        let content_changed = rewritten != content;
        if quarantine {
            identity = preserve_bound_file_before_retraction(config, entry.path(), identity)?;
        }
        if has_derived_content {
            if content_changed {
                before_mutation(entry.path());
                replace_preserved_file(
                    config,
                    entry.path(),
                    &identity,
                    Some(rewritten.as_bytes()),
                )?;
            }
        } else {
            before_mutation(entry.path());
            remove_preserved_file(config, entry.path(), &identity)?;
        }
    }
    Ok(removed)
}

fn render_para_summary<'a>(
    name: &str,
    items: impl IntoIterator<Item = &'a serde_json::Value>,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut summary = BoundedString::new(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES, 4096)?;
    summary.push_str("# ")?;
    summary.push_single_line(name)?;
    summary.push_str("\n\n")?;
    let mut by_category: BTreeMap<String, Vec<&serde_json::Value>> = BTreeMap::new();
    for item in items
        .into_iter()
        .filter(|item| item.get("status").and_then(|status| status.as_str()) != Some("superseded"))
    {
        let category = item
            .get("category")
            .and_then(|category| category.as_str())
            .unwrap_or("context");
        by_category
            .entry(category.to_string())
            .or_default()
            .push(item);
    }
    for (category, category_items) in by_category {
        summary.push_str("## ")?;
        summary.push_str(&capitalize(&category))?;
        summary.push_str("\n\n")?;
        for item in category_items {
            let text = item
                .get("fact")
                .and_then(|fact| fact.as_str())
                .unwrap_or("");
            summary.push_str("- ")?;
            summary.push_single_line(text)?;
            summary.push_char('\n')?;
        }
        summary.push_char('\n')?;
    }
    Ok(summary.into_string())
}

fn para_value_within_depth(value: &serde_json::Value, depth: usize) -> bool {
    if depth > MAX_PARA_RECONCILIATION_VALUE_DEPTH {
        return false;
    }
    match value {
        serde_json::Value::Array(values) => values
            .iter()
            .all(|value| para_value_within_depth(value, depth + 1)),
        serde_json::Value::Object(values) => values
            .values()
            .all(|value| para_value_within_depth(value, depth + 1)),
        _ => true,
    }
}

fn validate_para_items(items: &[serde_json::Value]) -> Result<(), Box<dyn std::error::Error>> {
    if items.len() > MAX_PARA_RECONCILIATION_ITEMS {
        return Err("PARA person exceeded the reconciliation item bound".into());
    }
    if !items.iter().all(|item| para_value_within_depth(item, 1)) {
        return Err("PARA person exceeded the reconciliation JSON depth bound".into());
    }
    Ok(())
}

fn revalidate_para_public_items(
    config: &Config,
    items: &[&serde_json::Value],
) -> Result<(), Box<dyn std::error::Error>> {
    revalidate_para_public_items_with_pending(config, items, None)
}

fn revalidate_para_public_items_with_pending(
    config: &Config,
    items: &[&serde_json::Value],
    pending: Option<(&AuthorizedMeeting, &BTreeSet<String>)>,
) -> Result<(), Box<dyn std::error::Error>> {
    validate_para_items(&items.iter().map(|item| (*item).clone()).collect::<Vec<_>>())?;
    let (manifest, valid) = load_provenance_manifest(config);
    if !valid {
        return Err("PARA publication provenance manifest is invalid".into());
    }
    let mut required = BTreeMap::<String, BTreeSet<String>>::new();
    let pending_source = pending
        .as_ref()
        .and_then(|(meeting, _)| exact_source_key(&meeting.path, config));
    for item in items {
        let source = item
            .get("source")
            .and_then(|value| value.as_str())
            .filter(|source| source.starts_with("v2:"))
            .ok_or("PARA publication lacks exact source provenance")?;
        let id = record_id_for_json("para", item)?;
        let pending_owned = pending_source.as_deref() == Some(source)
            && pending
                .as_ref()
                .is_some_and(|(_, records)| records.contains(&id));
        if !pending_owned
            && !manifest
                .records
                .get(source)
                .is_some_and(|records| records.contains(&id))
        {
            return Err("PARA publication record is absent from live provenance".into());
        }
        required.entry(source.to_string()).or_default().insert(id);
    }
    let mut live = BTreeMap::new();
    for entry in corpus_markdown_entries(config) {
        let Ok(meeting) = authorized_meeting(entry.path(), config) else {
            continue;
        };
        let Some(source) = exact_source_key(&meeting.path, config) else {
            continue;
        };
        if required.contains_key(&source) {
            live.insert(source, source_revision(&meeting.content));
        }
    }
    if let Some((meeting, _)) = pending {
        let current = authorized_meeting(&meeting.path, config)?;
        if !same_authorized_snapshot(meeting, &current) {
            return Err("PARA publication source changed at the publication boundary".into());
        }
        if let Some(source) = pending_source.as_ref() {
            live.insert(source.clone(), source_revision(&current.content));
        }
    }
    if required.keys().any(|source| {
        if pending_source.as_deref() == Some(source.as_str()) {
            pending.as_ref().is_none_or(|(meeting, _)| {
                live.get(source) != Some(&source_revision(&meeting.content))
            })
        } else {
            manifest
                .sources
                .get(source)
                .is_none_or(|revision| live.get(source) != Some(revision))
        }
    }) {
        return Err("PARA publication source is no longer strictly authorized".into());
    }
    Ok(())
}

#[cfg(test)]
fn create_private_publication_temporary(
    parent: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<(PathBuf, File), Box<dyn std::error::Error>> {
    let seed = format!(
        "{}:{}:{}",
        destination.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    for attempt in 0..100u64 {
        let temporary = parent.join(format!(
            ".minutes-publish-{:016x}.tmp",
            content_revision(&format!("{seed}:{attempt}"))
        ));
        let mut options = OpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = match options.open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };
        set_restrictive_permissions_file(&file)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        return Ok((temporary, file));
    }
    Err("could not allocate a private knowledge publication temporary".into())
}

#[cfg(test)]
fn retain_fresh_private_publication(
    parent: &Path,
    destination: &Path,
    bytes: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    require_retained_slot_budget(
        parent,
        ".minutes-publish-",
        u64::try_from(bytes.len()).map_err(|_| "knowledge publication is too large")?,
        MAX_RETAINED_PUBLICATION_TEMPS,
        MAX_RETAINED_PUBLICATION_BYTES,
    )?;
    let (temporary, file) = create_private_publication_temporary(parent, destination, bytes)?;
    attest_visible_exact_file(&temporary, &file, bytes)?;
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
fn private_publication_failure(
    parent: &Path,
    destination: &Path,
    bytes: &[u8],
    proof_error: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    match retain_fresh_private_publication(parent, destination, bytes) {
        Ok(()) => format!(
            "knowledge publication proof failed ({proof_error}); intended bytes were retained privately"
        )
        .into(),
        Err(retention_error) => format!(
            "knowledge publication proof failed ({proof_error}); intended bytes could not be freshly retained ({retention_error})"
        )
        .into(),
    }
}

#[cfg(test)]
fn claim_failed_private_publication(
    parent: &Path,
    destination: &Path,
    published: &File,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    let visible = match open_regular_file_no_follow(destination) {
        Ok(visible) if qmd_file_handles_match(published, &visible) => visible,
        Ok(_) => return Ok(None),
        Err(_) => return Ok(None),
    };
    drop(visible);

    let seed = format!(
        "{}:{}:{}",
        destination.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    for attempt in 0..100u64 {
        let claim = parent.join(format!(
            ".minutes-publish-failed-{:016x}.tmp",
            content_revision(&format!("{seed}:{attempt}"))
        ));
        match crate::policy_fs::move_entry_no_replace(destination, &claim) {
            Ok(()) => {
                let claimed = open_regular_file_no_follow(&claim)?;
                if qmd_file_handles_match(published, &claimed) {
                    #[cfg(unix)]
                    File::open(parent)?.sync_all()?;
                    return Ok(Some(claim));
                }
                // A pathname winner was claimed after the pre-check. Restore
                // it only with no-replace semantics. If another winner now
                // occupies the public name, both objects remain preserved.
                let _ = crate::policy_fs::move_entry_no_replace(&claim, destination);
                return Err("knowledge publication changed during exact failure claim".into());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate a failed knowledge publication claim".into())
}

#[cfg(test)]
fn recover_failed_private_publication(
    parent: &Path,
    destination: &Path,
    published: &File,
    bytes: &[u8],
    proof_error: Box<dyn std::error::Error>,
) -> Box<dyn std::error::Error> {
    let claim = claim_failed_private_publication(parent, destination, published);
    let retention = retain_fresh_private_publication(parent, destination, bytes);
    match (claim, retention) {
        (Ok(Some(claim)), Ok(())) => format!(
            "knowledge publication proof failed ({proof_error}); suspect public bytes were claimed at {} and intended bytes were retained privately",
            claim.display()
        )
        .into(),
        (Ok(None), Ok(())) => format!(
            "knowledge publication proof failed ({proof_error}); a pathname winner was preserved and intended bytes were retained privately"
        )
        .into(),
        (Err(claim_error), Ok(())) => format!(
            "knowledge publication proof failed ({proof_error}); exact public claim was incomplete ({claim_error}); intended bytes were retained privately"
        )
        .into(),
        (Ok(_), Err(retention_error)) => format!(
            "knowledge publication proof failed ({proof_error}); intended bytes could not be freshly retained ({retention_error})"
        )
        .into(),
        (Err(claim_error), Err(retention_error)) => format!(
            "knowledge publication proof failed ({proof_error}); exact public claim was incomplete ({claim_error}) and intended bytes could not be freshly retained ({retention_error})"
        )
        .into(),
    }
}

#[cfg(test)]
fn publish_private_file_no_replace_with_hook<F>(
    destination: &Path,
    bytes: &[u8],
    before_publish: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
{
    publish_private_file_no_replace_with_hooks(destination, bytes, before_publish, |_| {})
}

#[cfg(test)]
fn publish_private_file_no_replace_with_hooks<F, G>(
    destination: &Path,
    bytes: &[u8],
    before_publish: F,
    after_move_before_attestation: G,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce(&Path),
    G: FnOnce(&Path),
{
    if u64::try_from(bytes.len())
        .map(|len| len > MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)
        .unwrap_or(true)
    {
        return Err("knowledge publication exceeds the semantic output bound".into());
    }
    let parent = destination
        .parent()
        .ok_or("knowledge publication has no parent")?;
    require_retained_slot_budget(
        parent,
        ".minutes-publish-",
        u64::try_from(bytes.len()).map_err(|_| "knowledge publication is too large")?,
        MAX_RETAINED_PUBLICATION_TEMPS,
        MAX_RETAINED_PUBLICATION_BYTES,
    )?;
    let (temporary, file) = create_private_publication_temporary(parent, destination, bytes)?;
    before_publish(&temporary);
    if let Err(error) = attest_visible_exact_file(&temporary, &file, bytes) {
        return Err(private_publication_failure(
            parent,
            destination,
            bytes,
            error,
        ));
    }
    if let Err(error) = crate::policy_fs::move_entry_no_replace(&temporary, destination) {
        if attest_visible_exact_file(&temporary, &file, bytes).is_ok() {
            #[cfg(unix)]
            File::open(parent)?.sync_all()?;
            return Err(format!(
                "knowledge publication move failed ({error}); intended bytes were retained privately"
            )
            .into());
        }
        return Err(private_publication_failure(
            parent,
            destination,
            bytes,
            error.into(),
        ));
    }
    after_move_before_attestation(destination);
    if let Err(error) = attest_visible_exact_file(destination, &file, bytes) {
        return Err(recover_failed_private_publication(
            parent,
            destination,
            &file,
            bytes,
            error,
        ));
    }
    #[cfg(unix)]
    File::open(parent)?.sync_all()?;
    Ok(())
}

struct ParaPersonSnapshot {
    directory: File,
    entry_names: BTreeSet<OsString>,
    items: PreservedIdentity,
    summary: Option<PreservedIdentity>,
}

fn para_open_directory_no_follow(path: &Path) -> std::io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_DIRECTORY);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options
            // Parent capabilities create/sync children but are never renamed
            // themselves. Do not request DELETE: retained policy boundaries
            // intentionally deny delete sharing. Keep FILE_SHARE_DELETE so a
            // later exact child rename handle can coexist with snapshots.
            .access_mode(GENERIC_READ | GENERIC_WRITE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            );
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_dir() {
        return Err(std::io::Error::other("PARA path is not a directory"));
    }
    Ok(file)
}

fn para_open_directory_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY);
    #[cfg(windows)]
    {
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
            );
    }
    options
}

type ParaExpectedFiles = Vec<(OsString, Vec<u8>)>;

struct ParaPersonSuccessor {
    path: PathBuf,
    directory: File,
    expected: ParaExpectedFiles,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ParaFileProof {
    len: u64,
    sha256: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ParaGenerationProof {
    entry_names: Vec<String>,
    items: ParaFileProof,
    summary: Option<ParaFileProof>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ParaTransactionManifest {
    schema: u32,
    target_name: String,
    stage_name: Option<String>,
    capture_name: String,
    old: ParaGenerationProof,
    intended: Option<ParaGenerationProof>,
    #[serde(default)]
    slot_directory_identity: Option<QmdObjectIdentity>,
    #[serde(default)]
    slot_items_identity: Option<QmdObjectIdentity>,
    #[serde(default)]
    slot_summary_identity: Option<QmdObjectIdentity>,
    #[serde(default)]
    sequence: u64,
    #[serde(default)]
    journal_state: Option<RecyclableParaJournalState>,
    #[serde(default)]
    baseline_deleted: bool,
    #[serde(default)]
    baseline_parked: bool,
    #[serde(default)]
    prior_sequence: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum RecyclableParaJournalState {
    Baseline,
    Active,
    Completed,
}

struct ActiveParaTransaction {
    path: PathBuf,
    file: File,
    manifest: ParaTransactionManifest,
}

fn attest_para_directory_scopes_match(
    public: QmdObjectIdentity,
    private: QmdObjectIdentity,
) -> Result<(), Box<dyn std::error::Error>> {
    if public.scope != private.scope {
        return Err(
            "deterministic PARA recovery namespace is on another filesystem or volume".into(),
        );
    }
    Ok(())
}

fn para_private_root(config: &Config) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let knowledge = config.knowledge.path.canonicalize().map_err(|error| {
        format!("configured knowledge derivative cannot be bound canonically: {error}")
    })?;
    let output = config.output_dir.canonicalize().map_err(|error| {
        format!("configured meeting corpus cannot be bound canonically: {error}")
    })?;
    #[cfg(unix)]
    let digest = {
        use std::os::unix::ffi::OsStrExt;
        Sha256::digest(knowledge.as_os_str().as_bytes())
    };
    #[cfg(windows)]
    let digest = {
        use std::os::windows::ffi::OsStrExt;
        let units = knowledge
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>();
        Sha256::digest(units)
    };
    #[cfg(not(any(unix, windows)))]
    let digest = Sha256::digest(knowledge.to_string_lossy().as_bytes());
    let namespace = format!("{PARA_PRIVATE_ROOT}-{digest:x}");
    let parent = knowledge
        .parent()
        .ok_or("configured knowledge derivative has no parent")?;
    let private = parent.join(namespace);
    if private.starts_with(&knowledge) || private.starts_with(&output) {
        return Err("deterministic PARA recovery namespace overlaps a public corpus".into());
    }
    let directory = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&private)
        .map_err(|error| format!("deterministic PARA recovery namespace is unsafe: {error}"))?;
    let canonical = private.canonicalize()?;
    if canonical != private || directory.display_path() != private {
        return Err("deterministic PARA recovery namespace changed while it was bound".into());
    }
    directory.recovery_directory_proof()?;

    let people = knowledge.join("areas/people");
    let public_scope_path = match fs::symlink_metadata(&people) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => &people,
        Ok(_) => return Err("public PARA people path is not a real directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => &knowledge,
        Err(error) => return Err(error.into()),
    };
    let public_directory = para_open_directory_no_follow(public_scope_path)?;
    let private_directory = para_open_directory_no_follow(&private)?;
    let public_scope = qmd_file_identity_and_links(&public_directory)
        .map(|value| value.0.scope)
        .ok_or("public PARA directory scope could not be proven")?;
    let private_scope = qmd_file_identity_and_links(&private_directory)
        .map(|value| value.0.scope)
        .ok_or("private PARA directory scope could not be proven")?;
    attest_para_directory_scopes_match(
        QmdObjectIdentity {
            scope: public_scope,
            object: 0,
        },
        QmdObjectIdentity {
            scope: private_scope,
            object: 0,
        },
    )?;
    Ok(private)
}

#[cfg(test)]
fn para_private_root_for_test_knowledge(
    knowledge: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    create_private_dir_all(knowledge)?;
    let parent = knowledge
        .parent()
        .ok_or("test knowledge derivative has no parent")?;
    let output = parent.join(format!(
        ".minutes-para-test-output-{:016x}",
        content_revision(&knowledge.to_string_lossy())
    ));
    create_private_dir_all(&output)?;
    para_private_root(&Config {
        output_dir: output,
        knowledge: KnowledgeConfig {
            enabled: true,
            path: knowledge.to_path_buf(),
            adapter: "para".into(),
            ..KnowledgeConfig::default()
        },
        ..Config::default()
    })
}

fn unscoped_para_private_root(knowledge: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    #[cfg(test)]
    {
        para_private_root_for_test_knowledge(knowledge)
    }
    #[cfg(not(test))]
    {
        let _ = knowledge;
        Err("unscoped PARA updates are disabled; publication requires the full source and output policy"
            .into())
    }
}

fn prepare_para_private_root(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    Ok(())
}

fn para_file_proof(bytes: &[u8]) -> Result<ParaFileProof, Box<dyn std::error::Error>> {
    Ok(ParaFileProof {
        len: u64::try_from(bytes.len())?,
        sha256: Sha256::digest(bytes).into(),
    })
}

fn para_snapshot_proof(
    snapshot: &ParaPersonSnapshot,
) -> Result<ParaGenerationProof, Box<dyn std::error::Error>> {
    let entry_names = snapshot
        .entry_names
        .iter()
        .map(|name| {
            name.to_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| "PARA transaction cannot encode a non-UTF-8 entry".into())
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;
    Ok(ParaGenerationProof {
        entry_names,
        items: para_file_proof(&snapshot.items.bytes)?,
        summary: snapshot
            .summary
            .as_ref()
            .map(|summary| para_file_proof(&summary.bytes))
            .transpose()?,
    })
}

fn para_successor_proof(
    expected: &[(OsString, Vec<u8>)],
) -> Result<ParaGenerationProof, Box<dyn std::error::Error>> {
    let items = expected
        .iter()
        .find(|(name, _)| name == "items.json")
        .ok_or("PARA successor proof has no items.json")?;
    let summary = expected
        .iter()
        .find(|(name, _)| name == "summary.md")
        .ok_or("PARA successor proof has no summary.md")?;
    Ok(ParaGenerationProof {
        entry_names: vec!["items.json".to_string(), "summary.md".to_string()],
        items: para_file_proof(&items.1)?,
        summary: Some(para_file_proof(&summary.1)?),
    })
}

fn para_bytes_match_proof(bytes: &[u8], proof: &ParaFileProof) -> bool {
    u64::try_from(bytes.len()).ok() == Some(proof.len)
        && <[u8; 32]>::from(Sha256::digest(bytes)) == proof.sha256
}

enum ParaCapturedMemberState {
    RetainedOld(PreservedIdentity),
    LegacyZero,
}

fn inspect_para_captured_member(
    path: &Path,
    proof: &ParaFileProof,
) -> Result<ParaCapturedMemberState, Box<dyn std::error::Error>> {
    let identity =
        preserved_source_identity_with_limit(path, MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
    if identity.symlink || identity.links != 1 || identity.object_identity.is_none() {
        return Err("PARA captured member is not an exact single-link regular file".into());
    }
    // A durable zero-length single-link file is the retirement tombstone. It
    // is deliberately accepted independently for each member so recovery is
    // idempotent after a crash between member fsyncs.
    if identity.bytes.is_empty() {
        return Ok(ParaCapturedMemberState::LegacyZero);
    }
    if para_bytes_match_proof(&identity.bytes, proof) {
        return Ok(ParaCapturedMemberState::RetainedOld(identity));
    }
    Err("PARA captured member matched neither manifest bytes nor a zero tombstone".into())
}

fn attest_para_capture_shape(
    capture: &Path,
    proof: &ParaGenerationProof,
) -> Result<(), Box<dyn std::error::Error>> {
    let directory = para_open_directory_no_follow(capture)?;
    let expected = proof
        .entry_names
        .iter()
        .map(OsString::from)
        .collect::<BTreeSet<_>>();
    if expected.len() != proof.entry_names.len()
        || bounded_para_entry_names(capture, MAX_PARA_RECONCILIATION_ITEMS)? != expected
        || !file_identity_matches_path(&directory, capture)
    {
        return Err("PARA capture changed from its manifest directory proof".into());
    }
    Ok(())
}

fn retain_para_captured_member<S>(
    path: &Path,
    proof: &ParaFileProof,
    mut attest_successor: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: FnMut() -> Result<(), Box<dyn std::error::Error>>,
{
    match inspect_para_captured_member(path, proof)? {
        ParaCapturedMemberState::LegacyZero => {
            attest_successor()?;
            if !matches!(
                inspect_para_captured_member(path, proof)?,
                ParaCapturedMemberState::LegacyZero
            ) {
                return Err("PARA retirement tombstone changed during successor proof".into());
            }
        }
        ParaCapturedMemberState::RetainedOld(expected) => {
            let mut captured = open_exact_capture_for_retirement(path, &expected)?;
            attest_retained_capture_with_successor_proof(
                path,
                captured.as_mut(),
                &expected,
                |_| Ok(()),
                |_| Ok(()),
                &mut attest_successor,
            )?;
            if !matches!(
                inspect_para_captured_member(path, proof)?,
                ParaCapturedMemberState::RetainedOld(_)
            ) {
                return Err("PARA captured member was not retained as exact manifest bytes".into());
            }
        }
    }
    Ok(())
}

fn para_snapshot_matches_proof(snapshot: &ParaPersonSnapshot, proof: &ParaGenerationProof) -> bool {
    let names = snapshot
        .entry_names
        .iter()
        .filter_map(|name| name.to_str())
        .collect::<BTreeSet<_>>();
    names.len() == snapshot.entry_names.len()
        && proof.entry_names.len() == names.len()
        && names == proof.entry_names.iter().map(String::as_str).collect()
        && para_bytes_match_proof(&snapshot.items.bytes, &proof.items)
        && match (&snapshot.summary, &proof.summary) {
            (Some(summary), Some(expected)) => para_bytes_match_proof(&summary.bytes, expected),
            (None, None) => true,
            _ => false,
        }
}

fn inspect_para_generation_with_proof(
    path: &Path,
    proof: &ParaGenerationProof,
) -> Result<ParaPersonSnapshot, Box<dyn std::error::Error>> {
    let snapshot = inspect_para_person(path)?;
    if !para_snapshot_matches_proof(&snapshot, proof) {
        return Err("PARA generation did not match its durable transaction proof".into());
    }
    Ok(snapshot)
}

fn has_private_para_generation_with_proof(
    private_root: &Path,
    prefix: &str,
    proof: &ParaGenerationProof,
) -> Result<bool, Box<dyn std::error::Error>> {
    let mut scanned = 0usize;
    for entry in fs::read_dir(private_root)? {
        scanned = scanned
            .checked_add(1)
            .ok_or("PARA private generation scan overflowed")?;
        if scanned > MAX_PARA_RECONCILIATION_ITEMS {
            return Err("PARA private generation scan exceeded its bound".into());
        }
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(prefix))
            && inspect_para_generation_with_proof(&entry.path(), proof).is_ok()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn valid_para_transaction_component(name: &str, prefix: Option<&str>) -> bool {
    !name.is_empty()
        && Path::new(name).components().count() == 1
        && matches!(
            Path::new(name).components().next(),
            Some(Component::Normal(_))
        )
        && prefix.is_none_or(|prefix| name.starts_with(prefix))
}

fn para_transaction_path(private_root: &Path, target_name: &str) -> PathBuf {
    private_root.join(format!(
        "{PARA_PERSON_TRANSACTION_PREFIX}{:016x}",
        content_revision(target_name)
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_para_transaction_paths(private_root: &Path, target_name: &str) -> (PathBuf, PathBuf) {
    let digest = Sha256::digest(target_name.as_bytes());
    let base = private_root.join(format!("{PARA_PERSON_TRANSACTION_PREFIX}v2-{digest:x}"));
    (
        PathBuf::from(format!("{}.a", base.to_string_lossy())),
        PathBuf::from(format!("{}.b", base.to_string_lossy())),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
enum RecyclableParaRecordRead {
    Empty,
    Malformed(File),
    Valid(Box<ParaTransactionManifest>, File),
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn read_recyclable_para_record(
    path: &Path,
) -> Result<RecyclableParaRecordRead, Box<dyn std::error::Error>> {
    let identity = match preserved_source_identity_with_limit(path, MAX_PARA_TRANSACTION_BYTES) {
        Ok(identity) => identity,
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            return Ok(RecyclableParaRecordRead::Empty)
        }
        Err(error) => return Err(error),
    };
    let file = open_regular_file_no_follow_for_update(path)?;
    attest_visible_exact_file(path, &file, &identity.bytes)?;
    if identity.bytes.is_empty() {
        return Ok(RecyclableParaRecordRead::Empty);
    }
    match serde_json::from_slice::<ParaTransactionManifest>(&identity.bytes) {
        Ok(manifest) => Ok(RecyclableParaRecordRead::Valid(Box::new(manifest), file)),
        Err(_) => Ok(RecyclableParaRecordRead::Malformed(file)),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_recyclable_para_record(
    path: &Path,
    manifest: &ParaTransactionManifest,
    allow_nonempty: bool,
) -> Result<File, Box<dyn std::error::Error>> {
    let parent = path
        .parent()
        .ok_or("PARA recyclable journal has no parent")?;
    let name = path
        .file_name()
        .ok_or("PARA recyclable journal has no name")?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(parent)?;
    let file = match boundary.bind_exact_file(name) {
        Ok(bound) => {
            if !allow_nonempty && !bound.is_empty()? {
                return Err("inactive PARA recyclable journal was not empty".into());
            }
            bound.try_clone_exact_file()?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            boundary.create_new_exact_file(name)?
        }
        Err(error) => return Err(error.into()),
    };
    let bytes = bounded_json_to_vec_pretty(manifest, MAX_PARA_TRANSACTION_BYTES)?;
    file.set_len(0)?;
    let mut writer = file.try_clone()?;
    writer.seek(SeekFrom::Start(0))?;
    writer.write_all(&bytes)?;
    writer.sync_all()?;
    let visible = open_regular_file_no_follow_for_update(path)?;
    if !qmd_file_handles_match(&file, &visible) {
        return Err("PARA recyclable journal pathname changed during publication".into());
    }
    attest_visible_exact_file(path, &file, &bytes)?;
    sync_para_directory(parent)?;
    Ok(file)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn reset_exact_recyclable_record(
    path: &Path,
    file: &File,
) -> Result<(), Box<dyn std::error::Error>> {
    file.set_len(0)?;
    file.sync_all()?;
    let visible = open_regular_file_no_follow_for_update(path)?;
    if !qmd_file_handles_match(file, &visible) || visible.metadata()?.len() != 0 {
        return Err("PARA recyclable journal changed during exact reset".into());
    }
    sync_para_directory(path.parent().ok_or("PARA journal has no parent")?)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn attest_recyclable_journal_layout(
    people: &Path,
    private_root: &Path,
    manifest: &ParaTransactionManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = people.join(&manifest.target_name);
    let slot = private_root.join(&manifest.capture_name);
    let (_, parked) = recyclable_para_generation_paths(private_root, &target)?;
    match manifest.journal_state {
        Some(RecyclableParaJournalState::Baseline) if manifest.baseline_deleted => {
            attest_path_is_absent(&target)?;
            attest_recyclable_para_tombstone(&slot)?;
            if manifest.baseline_parked {
                attest_recyclable_para_tombstone(&parked)?;
            } else {
                attest_path_is_absent(&parked)?;
            }
        }
        Some(RecyclableParaJournalState::Baseline) => {
            inspect_para_generation_with_proof(&target, &manifest.old)?;
            attest_recyclable_para_tombstone(&slot)?;
            attest_path_is_absent(&parked)?;
        }
        Some(RecyclableParaJournalState::Completed) => {
            if let Some(intended) = manifest.intended.as_ref() {
                inspect_para_generation_with_proof(&target, intended)?;
                attest_recyclable_para_tombstone(&slot)?;
                attest_path_is_absent(&parked)?;
            } else {
                attest_path_is_absent(&target)?;
                attest_recyclable_para_tombstone(&slot)?;
                attest_recyclable_para_tombstone(&parked)?;
            }
        }
        _ => return Err("PARA journal record is not a terminal layout receipt".into()),
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn attest_recyclable_slot_identity_from_manifest(
    slot: &Path,
    manifest: &ParaTransactionManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected_directory = manifest
        .slot_directory_identity
        .ok_or("PARA recyclable journal has no slot directory identity")?;
    let expected_items = manifest
        .slot_items_identity
        .ok_or("PARA recyclable journal has no items slot identity")?;
    let expected_summary = manifest
        .slot_summary_identity
        .ok_or("PARA recyclable journal has no summary slot identity")?;
    let directory = para_open_directory_no_follow(slot)?;
    if qmd_file_identity_and_links(&directory).map(|value| value.0) != Some(expected_directory) {
        return Err("PARA recyclable slot directory changed after its durable intent".into());
    }
    for (name, expected) in [
        (OsStr::new("items.json"), expected_items),
        (OsStr::new("summary.md"), expected_summary),
    ] {
        let member = open_regular_file_no_follow(&slot.join(name))?;
        if qmd_file_identity_and_links(&member).map(|value| value.0) != Some(expected) {
            return Err("PARA recyclable slot member changed after its durable intent".into());
        }
    }
    attest_recyclable_para_tombstone(slot)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_active_is_exact_pre_mutation(
    people: &Path,
    private_root: &Path,
    transaction: &ActiveParaTransaction,
) -> Result<bool, Box<dyn std::error::Error>> {
    let manifest = &transaction.manifest;
    if manifest.journal_state != Some(RecyclableParaJournalState::Active) {
        return Ok(false);
    }
    let Some(prior_sequence) = manifest.prior_sequence else {
        return Ok(false);
    };
    let (a, b) = recyclable_para_transaction_paths(private_root, &manifest.target_name);
    let terminal_path = if transaction.path == a {
        &b
    } else if transaction.path == b {
        &a
    } else {
        return Ok(false);
    };
    let terminal = match read_recyclable_para_record(terminal_path)? {
        RecyclableParaRecordRead::Valid(record, _)
            if record.schema == 2
                && record.target_name == manifest.target_name
                && record.capture_name == manifest.capture_name
                && record.sequence == prior_sequence
                && matches!(
                    record.journal_state,
                    Some(
                        RecyclableParaJournalState::Baseline
                            | RecyclableParaJournalState::Completed
                    )
                ) =>
        {
            record
        }
        _ => return Ok(false),
    };
    if attest_recyclable_journal_layout(people, private_root, &terminal).is_err() {
        return Ok(false);
    }
    if !manifest.baseline_deleted {
        let terminal_public = match terminal.journal_state {
            Some(RecyclableParaJournalState::Baseline) => Some(&terminal.old),
            Some(RecyclableParaJournalState::Completed) => terminal.intended.as_ref(),
            _ => None,
        };
        if terminal_public != Some(&manifest.old) {
            return Ok(false);
        }
    }
    let target = people.join(&manifest.target_name);
    if manifest.baseline_deleted {
        if attest_path_is_absent(&target).is_err() {
            return Ok(false);
        }
    } else if inspect_para_generation_with_proof(&target, &manifest.old).is_err() {
        return Ok(false);
    }
    let slot = private_root.join(&manifest.capture_name);
    if attest_recyclable_slot_identity_from_manifest(&slot, manifest).is_err() {
        return Ok(false);
    }
    Ok(true)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn begin_recyclable_para_transaction(
    people: &Path,
    private_root: &Path,
    mut manifest: ParaTransactionManifest,
) -> Result<ActiveParaTransaction, Box<dyn std::error::Error>> {
    let (a, b) = recyclable_para_transaction_paths(private_root, &manifest.target_name);
    let records = [
        (&a, read_recyclable_para_record(&a)?),
        (&b, read_recyclable_para_record(&b)?),
    ];
    let target = people.join(&manifest.target_name);
    let expected_slot = recyclable_para_generation_paths(private_root, &target)?.0;
    let expected_slot_name = expected_slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("PARA recyclable slot name is invalid")?;
    if manifest.capture_name != expected_slot_name {
        return Err("PARA recyclable journal selected another target's slot".into());
    }
    for (_, record) in &records {
        if let RecyclableParaRecordRead::Valid(record, _) = record {
            if record.schema != 2
                || record.target_name != manifest.target_name
                || record.capture_name != manifest.capture_name
            {
                return Err("PARA recyclable journal belongs to another target".into());
            }
        }
    }
    let mut terminal = records
        .iter()
        .filter_map(|(path, record)| {
            if let RecyclableParaRecordRead::Valid(record, _) = record {
                matches!(
                    record.journal_state,
                    Some(
                        RecyclableParaJournalState::Baseline
                            | RecyclableParaJournalState::Completed
                    )
                )
                .then_some(((*path).clone(), record.as_ref().clone()))
            } else {
                None
            }
        })
        .max_by_key(|(_, record)| record.sequence);
    if terminal.is_none() {
        let mut baseline = manifest.clone();
        baseline.sequence = 0;
        baseline.journal_state = Some(RecyclableParaJournalState::Baseline);
        baseline.prior_sequence = None;
        attest_recyclable_journal_layout(people, private_root, &baseline)?;
        write_recyclable_para_record(&a, &baseline, false)?;
        terminal = Some((a.clone(), baseline));
    }
    let (mut terminal_path, mut terminal_record) =
        terminal.ok_or("PARA terminal receipt vanished")?;
    if let Err(stale_error) =
        attest_recyclable_journal_layout(people, private_root, &terminal_record)
    {
        let inactive_path = if terminal_path == a { &b } else { &a };
        let inactive_is_empty = records.iter().any(|(path, record)| {
            *path == inactive_path && matches!(record, RecyclableParaRecordRead::Empty)
        });
        let terminal_file = records.iter().find_map(|(path, record)| {
            if *path != &terminal_path {
                return None;
            }
            match record {
                RecyclableParaRecordRead::Valid(record, file)
                    if record.sequence == terminal_record.sequence =>
                {
                    file.try_clone().ok()
                }
                _ => None,
            }
        });
        if manifest.baseline_deleted || !inactive_is_empty || terminal_file.is_none() {
            return Err(stale_error);
        }

        // A user or reconciliation pass may have legitimately changed the
        // still-public generation after the prior terminal receipt. Refresh
        // that receipt without creating a one-slot crash window: first write
        // the current exact P/S/D baseline into the empty sibling, then reset
        // the retained old receipt by its exact handle. A malformed/non-empty
        // sibling is never treated as this benign refresh case.
        let mut refreshed = manifest.clone();
        refreshed.sequence = terminal_record
            .sequence
            .checked_add(2)
            .ok_or("PARA recyclable journal sequence overflowed")?;
        refreshed.journal_state = Some(RecyclableParaJournalState::Baseline);
        refreshed.prior_sequence = None;
        refreshed.baseline_deleted = false;
        refreshed.baseline_parked = false;
        attest_recyclable_journal_layout(people, private_root, &refreshed)?;
        write_recyclable_para_record(inactive_path, &refreshed, false)?;
        reset_exact_recyclable_record(
            &terminal_path,
            &terminal_file.ok_or("PARA prior terminal receipt vanished")?,
        )?;
        terminal_path = inactive_path.clone();
        terminal_record = refreshed;
    }
    attest_recyclable_journal_layout(people, private_root, &terminal_record)?;
    let active_path = if terminal_path == a { b } else { a };
    manifest.sequence = terminal_record
        .sequence
        .checked_add(1)
        .ok_or("PARA recyclable journal sequence overflowed")?;
    manifest.journal_state = Some(RecyclableParaJournalState::Active);
    manifest.prior_sequence = Some(terminal_record.sequence);
    let file = write_recyclable_para_record(&active_path, &manifest, false)?;
    Ok(ActiveParaTransaction {
        path: active_path,
        file,
        manifest,
    })
}

#[allow(dead_code)]
fn begin_para_transaction(
    private_root: &Path,
    manifest: ParaTransactionManifest,
) -> Result<ActiveParaTransaction, Box<dyn std::error::Error>> {
    let path = para_transaction_path(private_root, &manifest.target_name);
    let name = path.file_name().ok_or("PARA transaction has no name")?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(private_root)?;
    boundary.attest_for_source_cleanup()?;
    let mut file = match boundary.bind_owner_private_exact_file(name) {
        Ok(existing) => existing.try_clone_exact_file()?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            boundary.create_new_exact_file(name)?
        }
        Err(error) => return Err(error.into()),
    };
    if qmd_file_identity_and_links(&file).is_none_or(|(_, links)| links != 1) {
        return Err("PARA transaction manifest is not a single-link regular file".into());
    }
    if file.metadata()?.len() != 0 {
        return Err("PARA transaction recovery must complete before a new mutation".into());
    }
    let bytes = bounded_json_to_vec_pretty(&manifest, MAX_PARA_TRANSACTION_BYTES)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    boundary.attest_for_source_cleanup()?;
    attest_visible_exact_file(&path, &file, &bytes)?;
    sync_para_directory(private_root)?;
    Ok(ActiveParaTransaction {
        path,
        file,
        manifest,
    })
}

fn finish_para_transaction(
    transaction: &mut ActiveParaTransaction,
    people: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    finish_para_transaction_with_metadata_sync(transaction, people, sync_para_directory)
}

fn finish_para_transaction_with_metadata_sync<S>(
    transaction: &mut ActiveParaTransaction,
    people: &Path,
    mut sync_metadata: S,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: FnMut(&Path) -> Result<(), Box<dyn std::error::Error>>,
{
    let parent = transaction
        .path
        .parent()
        .ok_or("PARA transaction manifest has no parent")?;
    // Renames are the transaction's publication boundary. Flush directory
    // metadata before erasing the journal. On platforms/filesystems where a
    // directory handle cannot be flushed, this returns an error and retains
    // the non-empty journal for recovery rather than claiming durability.
    sync_metadata(parent)?;
    let manifest_bytes =
        bounded_json_to_vec_pretty(&transaction.manifest, MAX_PARA_TRANSACTION_BYTES)?;
    let visible = open_regular_file_no_follow(&transaction.path)?;
    if !qmd_file_handles_match(&transaction.file, &visible)
        || attest_visible_exact_file(&transaction.path, &transaction.file, &manifest_bytes).is_err()
    {
        return Err("PARA transaction manifest pathname changed before completion".into());
    }
    drop(visible);

    let boundary = crate::policy_fs::BoundRecoveryDirectory::bind_existing(parent)?;
    let parent_file = para_open_directory_no_follow(parent)?;
    boundary.attest_for_source_cleanup()?;
    let parent_cap = CapDir::from_std_file(parent_file);
    let source_name = transaction
        .path
        .file_name()
        .ok_or("PARA transaction manifest has no filename")?;
    for attempt in 0..100u64 {
        let completed_path = unique_para_generation_path(
            parent,
            PARA_PERSON_COMPLETED_TRANSACTION_PREFIX,
            &transaction.path,
            attempt,
        );
        let completed_name = completed_path
            .file_name()
            .ok_or("PARA completed transaction has no filename")?;
        match crate::policy_fs::move_entry_at_no_replace(
            &parent_cap,
            source_name,
            &transaction.file,
            &parent_cap,
            completed_name,
        ) {
            Ok(()) => {
                boundary.attest_for_source_cleanup()?;
                attest_visible_exact_file(&completed_path, &transaction.file, &manifest_bytes)?;
                sync_metadata(parent)?;
                cleanup_completed_para_transaction(
                    people,
                    parent,
                    &completed_path,
                    &transaction.file,
                    &transaction.manifest,
                )?;
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("PARA completed transaction retention slots are exhausted".into())
}

#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
fn remove_owner_private_para_file(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("private PARA file has no parent")?;
    let name = path.file_name().ok_or("private PARA file has no name")?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(parent)?;
    let file = boundary.bind_file_allow_links(name)?;
    boundary.remove_owned_private_file(file)?;
    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn remove_owner_private_para_capture(
    private_root: &Path,
    capture: &Path,
    proof: &ParaGenerationProof,
) -> Result<(), Box<dyn std::error::Error>> {
    remove_owner_private_para_capture_with_hook(private_root, capture, proof, || {})
}

#[cfg(any(test, not(any(target_os = "linux", target_os = "macos"))))]
fn remove_owner_private_para_capture_with_hook(
    private_root: &Path,
    capture: &Path,
    proof: &ParaGenerationProof,
    before_final_identity_check: impl FnOnce(),
) -> Result<(), Box<dyn std::error::Error>> {
    let root_boundary =
        crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(private_root)?;
    root_boundary.attest_for_source_cleanup()?;
    let capture_name = capture.file_name().ok_or("PARA capture has no name")?;
    let exact_capture = root_boundary.bind_existing_owner_private_child(capture_name)?;
    let expected = proof
        .entry_names
        .iter()
        .map(OsString::from)
        .collect::<BTreeSet<_>>();
    let present = bounded_para_entry_names(capture, MAX_PARA_RECONCILIATION_ITEMS)?;
    if !present.is_subset(&expected) {
        return Err("completed PARA capture retained unexpected entries".into());
    }
    if present.contains(&OsString::from("items.json")) {
        inspect_para_captured_member(&capture.join("items.json"), &proof.items)?;
        let file = exact_capture.bind_file_allow_links(OsStr::new("items.json"))?;
        exact_capture.remove_owned_private_file(file)?;
    }
    if let Some(summary) = proof.summary.as_ref() {
        if present.contains(&OsString::from("summary.md")) {
            inspect_para_captured_member(&capture.join("summary.md"), summary)?;
            let file = exact_capture.bind_file_allow_links(OsStr::new("summary.md"))?;
            exact_capture.remove_owned_private_file(file)?;
        }
    }
    let capture_directory = para_open_directory_no_follow(capture)?;
    if !bounded_para_entry_names(capture, 1)?.is_empty() {
        return Err("completed PARA capture retained unexpected entries".into());
    }
    root_boundary.attest_for_source_cleanup()?;
    exact_capture.attest_exact_directory_file(&capture_directory)?;
    drop(capture_directory);
    root_boundary
        .remove_owned_private_empty_child_with_hook(exact_capture, before_final_identity_check)?;
    root_boundary.attest_for_source_cleanup()?;
    sync_para_directory(private_root)?;
    Ok(())
}

fn cleanup_completed_para_transaction(
    people: &Path,
    private_root: &Path,
    completed_path: &Path,
    completed_file: &File,
    manifest: &ParaTransactionManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let target = people.join(&manifest.target_name);
    if let Some(intended) = manifest.intended.as_ref() {
        inspect_para_generation_with_proof(&target, intended)?;
    } else {
        attest_path_is_absent(&target)?;
    }
    let capture = private_root.join(&manifest.capture_name);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        // Schema-1 migration residue predates the recyclable directory slots.
        // POSIX cannot unlink these exact names safely. Scrub each proven
        // member by retained handle, leave the now-zero legacy directory as a
        // bounded one-time tombstone, and exactly reset the completed control.
        let expected = manifest
            .old
            .entry_names
            .iter()
            .map(OsString::from)
            .collect::<BTreeSet<_>>();
        let mut proven = BTreeSet::from([OsString::from("items.json")]);
        if manifest.old.summary.is_some() {
            proven.insert(OsString::from("summary.md"));
        }
        if expected != proven {
            return Err("completed PARA capture proof names are inconsistent".into());
        }
        let present = bounded_para_entry_names(&capture, MAX_PARA_RECONCILIATION_ITEMS)?;
        if !present.is_subset(&expected) {
            return Err("completed PARA capture retained unexpected entries".into());
        }
        let capture_boundary =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&capture)?;
        for (name, proof) in [
            (OsStr::new("items.json"), Some(&manifest.old.items)),
            (OsStr::new("summary.md"), manifest.old.summary.as_ref()),
        ] {
            if let Some(proof) = proof {
                let member = capture_boundary.bind_exact_file(name)?;
                match inspect_para_captured_member(&capture.join(name), proof)? {
                    ParaCapturedMemberState::LegacyZero => {
                        member.recovery_proof_for_exact_bytes_bounded(
                            &[],
                            MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                            Instant::now() + Duration::from_secs(5),
                        )?;
                    }
                    ParaCapturedMemberState::RetainedOld(expected) => {
                        member.recovery_proof_for_exact_bytes_bounded(
                            &expected.bytes,
                            MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                            Instant::now() + Duration::from_secs(5),
                        )?;
                        member.zero_exact_for_retirement()?;
                    }
                }
            }
        }
        sync_para_directory(&capture)?;
        let manifest_bytes = bounded_json_to_vec_pretty(manifest, MAX_PARA_TRANSACTION_BYTES)?;
        attest_visible_exact_file(completed_path, completed_file, &manifest_bytes)?;
        completed_file.set_len(0)?;
        completed_file.sync_all()?;
        sync_para_directory(private_root)?;
        Ok(())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        remove_owner_private_para_capture(private_root, &capture, &manifest.old)?;
        let manifest_bytes = bounded_json_to_vec_pretty(manifest, MAX_PARA_TRANSACTION_BYTES)?;
        attest_visible_exact_file(completed_path, completed_file, &manifest_bytes)?;
        remove_owner_private_para_file(completed_path)?;
        Ok(())
    }
}

fn para_expected_from_snapshot(
    snapshot: &ParaPersonSnapshot,
) -> Result<ParaExpectedFiles, Box<dyn std::error::Error>> {
    Ok(vec![
        (OsString::from("items.json"), snapshot.items.bytes.clone()),
        (
            OsString::from("summary.md"),
            snapshot
                .summary
                .as_ref()
                .ok_or("PARA intended generation lost summary.md")?
                .bytes
                .clone(),
        ),
    ])
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_generation_is_tombstone(path: &Path) -> bool {
    attest_recyclable_para_tombstone(path).is_ok()
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_generation_has_partial_retirement(path: &Path, proof: &ParaGenerationProof) -> bool {
    let expected = [OsString::from("items.json"), OsString::from("summary.md")]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if bounded_para_entry_names(path, 2).ok().as_ref() != Some(&expected) {
        return false;
    }
    let Some(summary) = proof.summary.as_ref() else {
        return false;
    };
    let states = [
        inspect_para_captured_member(&path.join("items.json"), &proof.items),
        inspect_para_captured_member(&path.join("summary.md"), summary),
    ];
    states.iter().all(|state| {
        matches!(
            state,
            Ok(ParaCapturedMemberState::RetainedOld(_) | ParaCapturedMemberState::LegacyZero)
        )
    }) && states
        .iter()
        .any(|state| matches!(state, Ok(ParaCapturedMemberState::LegacyZero)))
        && states
            .iter()
            .any(|state| matches!(state, Ok(ParaCapturedMemberState::RetainedOld(_))))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn repair_recyclable_para_tombstone(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            let expected = [OsString::from("items.json"), OsString::from("summary.md")]
                .into_iter()
                .collect::<BTreeSet<_>>();
            let present = bounded_para_entry_names(path, 2)?;
            if !present.is_subset(&expected) {
                return Err("partial PARA tombstone contains an unexpected entry".into());
            }
            let directory = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
            for name in &present {
                let member = directory.bind_exact_file(name)?;
                member.recovery_proof_for_exact_bytes_bounded(
                    &[],
                    MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                    Instant::now() + Duration::from_secs(5),
                )?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Ok(_) => return Err("partial PARA tombstone path is not a real directory".into()),
        Err(error) => return Err(error.into()),
    }
    ensure_recyclable_para_members(path)?;
    attest_recyclable_para_tombstone(path)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn finish_revoked_intended_cleanup_if_started(
    people: &Path,
    private_root: &Path,
    target: &Path,
    slot: &Path,
    parked: &Path,
    intended: &ParaGenerationProof,
    transaction: &mut ActiveParaTransaction,
) -> Result<bool, Box<dyn std::error::Error>> {
    let target_tombstone = recyclable_generation_is_tombstone(target);
    let target_absent = fs::symlink_metadata(target)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    let parked_tombstone = recyclable_generation_is_tombstone(parked);
    if !(target_tombstone || target_absent && parked_tombstone) {
        return Ok(false);
    }

    if target_absent && parked_tombstone {
        if !recyclable_generation_is_tombstone(slot) {
            // This same P=absent/D=tombstone layout can be a valid recreation
            // before publication, with intended bytes still staged in S.
            // Cleanup always scrubs S before parking P, so only an exact zero
            // S is an unambiguous post-park deletion state.
            return Ok(false);
        }
        attest_recyclable_para_tombstone(slot)?;
        finish_recyclable_para_transaction_as_deletion(transaction, people, private_root)?;
        return Ok(true);
    }

    if inspect_para_generation_with_proof(slot, intended).is_ok()
        || recyclable_generation_has_partial_retirement(slot, intended)
    {
        scrub_recyclable_para_generation(slot, intended)?;
    } else if !recyclable_generation_is_tombstone(slot) {
        return Ok(false);
    }

    if target_tombstone {
        park_recyclable_public_tombstone(target, parked)?;
    }
    attest_path_is_absent(target)?;
    attest_recyclable_para_tombstone(slot)?;
    attest_recyclable_para_tombstone(parked)?;
    finish_recyclable_para_transaction_as_deletion(transaction, people, private_root)?;
    Ok(true)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn revalidate_recyclable_generation(
    config: Option<&Config>,
    snapshot: &ParaPersonSnapshot,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(config) = config else {
        return Ok(());
    };
    let items: Vec<serde_json::Value> = serde_json::from_slice(&snapshot.items.bytes)
        .map_err(|_| "recovered recyclable PARA items are malformed")?;
    validate_para_items(&items)?;
    let refs = items.iter().collect::<Vec<_>>();
    revalidate_para_public_items(config, &refs)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn scrub_partial_recyclable_slot_from_journal(
    path: &Path,
    manifest: &ParaTransactionManifest,
) -> Result<(), Box<dyn std::error::Error>> {
    let expected_directory = manifest
        .slot_directory_identity
        .ok_or("PARA recyclable journal has no slot directory identity")?;
    let expected_items = manifest
        .slot_items_identity
        .ok_or("PARA recyclable journal has no items slot identity")?;
    let expected_summary = manifest
        .slot_summary_identity
        .ok_or("PARA recyclable journal has no summary slot identity")?;
    let directory_file = para_open_directory_no_follow(path)?;
    if qmd_file_identity_and_links(&directory_file).map(|value| value.0) != Some(expected_directory)
    {
        return Err("PARA recyclable slot directory changed after its durable intent".into());
    }
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    for (name, expected) in [
        (OsStr::new("items.json"), expected_items),
        (OsStr::new("summary.md"), expected_summary),
    ] {
        let member = boundary.bind_exact_file(name)?;
        let exact = member.try_clone_exact_file()?;
        if qmd_file_identity_and_links(&exact).map(|value| value.0) != Some(expected) {
            return Err("PARA recyclable slot member changed after its durable intent".into());
        }
        exact.set_len(0)?;
        exact.sync_all()?;
    }
    sync_para_directory_handle(&directory_file)?;
    sync_para_directory(path.parent().ok_or("PARA slot has no parent")?)?;
    attest_recyclable_para_tombstone(path)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn exchange_recyclable_para_generations(
    people: &Path,
    target: &Path,
    private_root: &Path,
    slot: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let public_parent = crate::policy_fs::BoundRecoveryDirectory::bind_existing(people)?;
    let private_parent =
        crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(private_root)?;
    public_parent.exchange_exact_private_children_with_hook(
        target.file_name().ok_or("PARA target has no name")?,
        &private_parent,
        slot.file_name().ok_or("PARA slot has no name")?,
        || {},
    )?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn park_recyclable_public_tombstone(
    target: &Path,
    parked: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if fs::symlink_metadata(parked).is_ok() {
        return Err("PARA parked tombstone name is already occupied".into());
    }
    let tombstone = attest_recyclable_para_tombstone(target)?;
    move_para_directory_to_claim(target, parked, &tombstone)?
        .ok_or("PARA public tombstone changed before parking")?;
    attest_path_is_absent(target)?;
    attest_recyclable_para_tombstone(parked)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recover_recyclable_para_transaction(
    people: &Path,
    private_root: &Path,
    config: Option<&Config>,
    transaction: &mut ActiveParaTransaction,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = transaction.manifest.clone();
    let (journal_a, journal_b) =
        recyclable_para_transaction_paths(private_root, &manifest.target_name);
    if !valid_para_transaction_component(&manifest.target_name, None)
        || !valid_para_transaction_component(&manifest.capture_name, Some(PARA_PERSON_SLOT_PREFIX))
        || manifest
            .stage_name
            .as_ref()
            .is_some_and(|name| name != &manifest.capture_name)
        || (transaction.path != journal_a && transaction.path != journal_b)
    {
        return Err("PARA recyclable transaction manifest is invalid".into());
    }
    let target = people.join(&manifest.target_name);
    let slot = private_root.join(&manifest.capture_name);
    let (_, parked) = recyclable_para_generation_paths(private_root, &target)?;
    if slot != recyclable_para_generation_paths(private_root, &target)?.0 {
        return Err("PARA recyclable transaction selected the wrong fixed slot".into());
    }
    if recyclable_active_is_exact_pre_mutation(people, private_root, transaction)? {
        abort_recyclable_para_transaction(transaction, private_root)?;
        return Ok(());
    }
    let target_absent = fs::symlink_metadata(&target)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    let parked_tombstone = recyclable_generation_is_tombstone(&parked);
    let slot_tombstone = recyclable_generation_is_tombstone(&slot);
    let target_tombstone = recyclable_generation_is_tombstone(&target);
    let target_old = inspect_para_generation_with_proof(&target, &manifest.old).ok();
    let slot_old = inspect_para_generation_with_proof(&slot, &manifest.old).ok();
    let target_intended = manifest
        .intended
        .as_ref()
        .and_then(|proof| inspect_para_generation_with_proof(&target, proof).ok());
    let slot_intended = manifest
        .intended
        .as_ref()
        .and_then(|proof| inspect_para_generation_with_proof(&slot, proof).ok());

    if let Some(intended) = manifest.intended.as_ref() {
        if finish_revoked_intended_cleanup_if_started(
            people,
            private_root,
            &target,
            &slot,
            &parked,
            intended,
            transaction,
        )? {
            return Ok(());
        }
        if manifest.baseline_deleted {
            if target_absent && slot_intended.is_none() && !slot_tombstone {
                scrub_partial_recyclable_slot_from_journal(&slot, &manifest)?;
                abort_recyclable_para_transaction(transaction, private_root)?;
                return Ok(());
            }
            if target_absent {
                if let Some(staged) = slot_intended.as_ref() {
                    if revalidate_recyclable_generation(config, staged).is_err() {
                        scrub_recyclable_para_generation(&slot, intended)?;
                        abort_recyclable_para_transaction(transaction, private_root)?;
                        return Ok(());
                    }
                    move_para_directory_to_claim(&slot, &target, &staged.directory)?
                        .ok_or("recreated PARA successor changed before publication")?;
                }
            }
            if inspect_para_generation_with_proof(&target, intended).is_ok() {
                if fs::symlink_metadata(&slot)
                    .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                {
                    if recyclable_generation_is_tombstone(&parked) {
                        let parked_file = attest_recyclable_para_tombstone(&parked)?;
                        move_para_directory_to_claim(&parked, &slot, &parked_file)?
                            .ok_or("parked PARA tombstone changed during crash recovery")?;
                    } else {
                        repair_recyclable_para_tombstone(&slot)?;
                    }
                } else {
                    repair_recyclable_para_tombstone(&slot)?;
                }
                attest_recyclable_para_tombstone(&slot)?;
                attest_path_is_absent(&parked)?;
                let published = inspect_para_generation_with_proof(&target, intended)?;
                if revalidate_recyclable_generation(config, &published).is_err() {
                    exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
                    scrub_recyclable_para_generation(&slot, intended)?;
                    park_recyclable_public_tombstone(&target, &parked)?;
                    return finish_recyclable_para_transaction_as_deletion(
                        transaction,
                        people,
                        private_root,
                    );
                }
                return finish_recyclable_para_transaction(transaction, people, private_root);
            }
            return Err("PARA recreation journal did not match any safe fixed-slot layout".into());
        }
        if target_tombstone && recyclable_generation_has_partial_retirement(&slot, intended) {
            scrub_recyclable_para_generation(&slot, intended)?;
            park_recyclable_public_tombstone(&target, &parked)?;
            return finish_recyclable_para_transaction_as_deletion(
                transaction,
                people,
                private_root,
            );
        }
        if target_old.is_some() && recyclable_generation_has_partial_retirement(&slot, intended) {
            scrub_recyclable_para_generation(&slot, intended)?;
            let old = inspect_para_generation_with_proof(&target, &manifest.old)?;
            if revalidate_recyclable_generation(config, &old).is_err() {
                exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
                scrub_recyclable_para_generation(&slot, &manifest.old)?;
                park_recyclable_public_tombstone(&target, &parked)?;
                return finish_recyclable_para_transaction_as_deletion(
                    transaction,
                    people,
                    private_root,
                );
            }
            abort_recyclable_para_transaction(transaction, private_root)?;
            return Ok(());
        }
        if target_tombstone && recyclable_generation_has_partial_retirement(&slot, &manifest.old) {
            scrub_recyclable_para_generation(&slot, &manifest.old)?;
            park_recyclable_public_tombstone(&target, &parked)?;
            return finish_recyclable_para_transaction_as_deletion(
                transaction,
                people,
                private_root,
            );
        }
        if target_old.is_some()
            && target_intended.is_none()
            && slot_intended.is_none()
            && !slot_tombstone
        {
            // The process died while refilling the inactive fixed slot. The
            // public generation never moved. Only the exact directory and
            // member identities recorded before the first write may be
            // scrubbed; a pathname winner is preserved and fails closed.
            scrub_partial_recyclable_slot_from_journal(&slot, &manifest).map_err(|error| {
                format!("failed to scrub the partially refilled PARA slot: {error}")
            })?;
            let old = inspect_para_generation_with_proof(&target, &manifest.old)?;
            if revalidate_recyclable_generation(config, &old).is_err() {
                exchange_recyclable_para_generations(people, &target, private_root, &slot)
                    .map_err(|error| {
                        format!("failed to exchange the revoked PARA generation: {error}")
                    })?;
                scrub_recyclable_para_generation(&slot, &manifest.old).map_err(|error| {
                    format!("failed to scrub the revoked PARA generation: {error}")
                })?;
                park_recyclable_public_tombstone(&target, &parked).map_err(|error| {
                    format!("failed to park the revoked PARA tombstone: {error}")
                })?;
                return finish_recyclable_para_transaction_as_deletion(
                    transaction,
                    people,
                    private_root,
                )
                .map_err(|error| {
                    format!("failed to finish the revoked PARA transaction: {error}").into()
                });
            }
            abort_recyclable_para_transaction(transaction, private_root)?;
            return Ok(());
        }
        if let Some(published) = target_intended.as_ref() {
            if recyclable_generation_has_partial_retirement(&slot, &manifest.old) {
                if revalidate_recyclable_generation(config, published).is_ok() {
                    scrub_recyclable_para_generation(&slot, &manifest.old)?;
                    return finish_recyclable_para_transaction(transaction, people, private_root);
                }
                // A partially retired old generation cannot be republished.
                // Complete its exact per-member scrub, exchange the private
                // tombstone into P, then scrub the revoked intended bytes now
                // held at S and park P as a genuine deletion.
                scrub_recyclable_para_generation(&slot, &manifest.old)?;
                exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
                scrub_recyclable_para_generation(&slot, intended)?;
                park_recyclable_public_tombstone(&target, &parked)?;
                return finish_recyclable_para_transaction_as_deletion(
                    transaction,
                    people,
                    private_root,
                );
            }
        }
        if let (Some(published), true) = (target_intended.as_ref(), slot_tombstone) {
            if revalidate_recyclable_generation(config, published).is_err() {
                exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
                scrub_recyclable_para_generation(&slot, intended)?;
                park_recyclable_public_tombstone(&target, &parked)?;
                return finish_recyclable_para_transaction_as_deletion(
                    transaction,
                    people,
                    private_root,
                );
            }
            return finish_recyclable_para_transaction(transaction, people, private_root);
        }
        if let (Some(published), Some(_old)) = (target_intended.as_ref(), slot_old.as_ref()) {
            if revalidate_recyclable_generation(config, published).is_ok() {
                scrub_recyclable_para_generation(&slot, &manifest.old)?;
                return finish_recyclable_para_transaction(transaction, people, private_root);
            }
            exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
            let restored_old = inspect_para_generation_with_proof(&target, &manifest.old)?;
            let revoked_intended = inspect_para_generation_with_proof(&slot, intended)?;
            let _ = (restored_old, revoked_intended);
            scrub_recyclable_para_generation(&slot, intended)?;
        }
        if let (Some(old), Some(staged)) = (target_old.as_ref(), slot_intended.as_ref()) {
            if revalidate_recyclable_generation(config, staged).is_ok() {
                exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
                scrub_recyclable_para_generation(&slot, &manifest.old)?;
                return finish_recyclable_para_transaction(transaction, people, private_root);
            }
            let _ = old;
            scrub_recyclable_para_generation(&slot, intended)?;
        }
        if inspect_para_generation_with_proof(&target, &manifest.old).is_ok() && slot_tombstone {
            let old = inspect_para_generation_with_proof(&target, &manifest.old)?;
            if revalidate_recyclable_generation(config, &old).is_ok() {
                abort_recyclable_para_transaction(transaction, private_root)?;
                return Ok(());
            }
        }

        // Either the intended successor or the still-public old generation
        // lost current policy authority. Exchange an exact private tombstone
        // into the public name, scrub the captured bytes by handle, and park
        // the public tombstone so the deletion is a genuine absence.
        if !recyclable_generation_is_tombstone(&slot) {
            return Err("PARA revoked recyclable slot could not be reduced to a tombstone".into());
        }
        if inspect_para_generation_with_proof(&target, &manifest.old).is_ok() {
            exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
            scrub_recyclable_para_generation(&slot, &manifest.old)?;
        }
        if recyclable_generation_is_tombstone(&target) {
            park_recyclable_public_tombstone(&target, &parked)?;
        }
        if !fs::symlink_metadata(&target)
            .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Err("revoked recyclable PARA generation remained public".into());
        }
        return finish_recyclable_para_transaction_as_deletion(transaction, people, private_root);
    }

    // Deletion has four durable layouts: before exchange, after exchange,
    // after exact scrub, and after the public tombstone is parked.
    if target_absent && parked_tombstone && slot_tombstone {
        return finish_recyclable_para_transaction(transaction, people, private_root);
    }
    if target_old.is_some() && slot_tombstone {
        exchange_recyclable_para_generations(people, &target, private_root, &slot)?;
    }
    if recyclable_generation_has_partial_retirement(&slot, &manifest.old) {
        scrub_recyclable_para_generation(&slot, &manifest.old)?;
    }
    if inspect_para_generation_with_proof(&slot, &manifest.old).is_ok() {
        scrub_recyclable_para_generation(&slot, &manifest.old)?;
    }
    if target_tombstone || recyclable_generation_is_tombstone(&target) {
        park_recyclable_public_tombstone(&target, &parked)?;
    }
    if attest_path_is_absent(&target).is_err()
        || !recyclable_generation_is_tombstone(&slot)
        || !recyclable_generation_is_tombstone(&parked)
    {
        return Err(
            "PARA recyclable deletion recovery did not reach its fixed terminal state".into(),
        );
    }
    finish_recyclable_para_transaction(transaction, people, private_root)
}

fn recover_one_para_transaction(
    people: &Path,
    private_root: &Path,
    config: Option<&Config>,
    transaction: &mut ActiveParaTransaction,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    if transaction.manifest.schema == 2 {
        return recover_recyclable_para_transaction(people, private_root, config, transaction);
    }
    let manifest = &transaction.manifest;
    if manifest.schema != 1
        || !valid_para_transaction_component(&manifest.target_name, None)
        || !valid_para_transaction_component(
            &manifest.capture_name,
            Some(PARA_PERSON_CAPTURE_PREFIX),
        )
        || manifest.stage_name.as_ref().is_some_and(|name| {
            !valid_para_transaction_component(name, Some(PARA_PERSON_STAGE_PREFIX))
        })
        || manifest.stage_name.is_some() != manifest.intended.is_some()
        || para_transaction_path(private_root, &manifest.target_name) != transaction.path
    {
        return Err("PARA transaction manifest is invalid".into());
    }
    let target = people.join(&manifest.target_name);
    let capture = private_root.join(&manifest.capture_name);
    let stage = manifest
        .stage_name
        .as_ref()
        .map(|name| private_root.join(name));

    let target_is_intended = manifest
        .intended
        .as_ref()
        .is_some_and(|proof| inspect_para_generation_with_proof(&target, proof).is_ok());
    let target_is_old = inspect_para_generation_with_proof(&target, &manifest.old).is_ok();
    let target_absent = fs::symlink_metadata(&target)
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
    if !target_is_intended && !target_is_old && !target_absent {
        return Err("PARA transaction target is occupied by an unproven generation".into());
    }

    if target_is_old {
        if fs::symlink_metadata(&capture).is_ok() {
            return Err("PARA transaction capture name is unexpectedly occupied".into());
        }
        let old = inspect_para_generation_with_proof(&target, &manifest.old)?;
        move_para_directory_to_claim(&target, &capture, &old.directory)?
            .ok_or("PARA transaction old generation changed before recovery claim")?;
    }

    attest_para_capture_shape(&capture, &manifest.old)?;
    if !target_is_intended {
        if let (Some(stage), Some(intended)) = (stage.as_ref(), manifest.intended.as_ref()) {
            let staged = match inspect_para_generation_with_proof(stage, intended) {
                Ok(staged) => staged,
                Err(_) => {
                    if fs::symlink_metadata(&target)
                        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
                    {
                        // A failed publication retains a fresh, exact intended
                        // successor under a new private stage name. That is a
                        // deliberate terminal quarantine, not a pre-publication
                        // crash: keep both the journal and old capture hidden.
                        if has_private_para_generation_with_proof(
                            private_root,
                            PARA_PERSON_STAGE_PREFIX,
                            intended,
                        )? {
                            return Ok(());
                        }
                        // Restoration is permitted only while every captured
                        // member is still the exact old generation. A partial
                        // retirement is never republished.
                        if let Ok(capture_snapshot) =
                            inspect_para_generation_with_proof(&capture, &manifest.old)
                        {
                            move_para_directory_to_claim(
                                &capture,
                                &target,
                                &capture_snapshot.directory,
                            )?
                            .ok_or("PARA transaction capture changed before safe restoration")?;
                            sync_para_directory(people)?;
                            finish_para_transaction(transaction, people)?;
                        }
                    }
                    return Err("PARA intended stage was unavailable; exact old bytes were restored only when the capture was wholly unretired, otherwise the journal and private capture were retained".into());
                }
            };
            let expected = para_expected_from_snapshot(&staged)?;
            if let Some(config) = config {
                let items: Vec<serde_json::Value> = serde_json::from_slice(&staged.items.bytes)?;
                validate_para_items(&items)?;
                let refs = items.iter().collect::<Vec<_>>();
                if revalidate_para_public_items(config, &refs).is_err() {
                    // The exact intended generation remains owner-private. A
                    // crash journal is not independent authority to republish
                    // sources that have since been revoked or changed.
                    return Ok(());
                }
            }
            move_para_directory_to_claim(stage, &target, &staged.directory)?
                .ok_or("PARA recovered stage changed before publication")?;
            if inspect_para_generation_with_proof(&target, intended).is_err() {
                let failed = move_para_directory_to_unique_claim(
                    &target,
                    private_root,
                    PARA_PERSON_FAILED_PREFIX,
                    &staged.directory,
                );
                let retained = retain_fresh_para_successor(private_root, &target, &expected);
                let transaction_status =
                    "transaction manifest was retained because captured members were not retired";
                return Err(format!(
                    "PARA recovered successor proof failed; failed claim: {}; intended retention: {}; {transaction_status}",
                    failed
                        .map(|path| path.map_or_else(|| "winner preserved".to_string(), |path| path.display().to_string()))
                        .unwrap_or_else(|error| error.to_string()),
                    retained
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|error| error.to_string())
                )
                .into());
            }
        } else if !target_absent && !target_is_old {
            return Err("PARA deletion transaction target changed during recovery".into());
        }
    }

    // A durable journal and byte-identical public generation are not current
    // source-policy authority. Revalidate even when recovery starts after the
    // stage was already published: the source may have become restricted in
    // the crash window. On any policy or parse failure, atomically claim the
    // exact public generation back into the owner-private namespace and retain
    // the journal so no revoked fact can be accepted as a completed successor.
    if let (Some(config), Some(intended)) = (config, manifest.intended.as_ref()) {
        let published = inspect_para_generation_with_proof(&target, intended)?;
        let current_authority = (|| -> Result<(), Box<dyn std::error::Error>> {
            let items: Vec<serde_json::Value> = serde_json::from_slice(&published.items.bytes)
                .map_err(|_| "recovered PARA items are malformed")?;
            validate_para_items(&items)?;
            let refs = items.iter().collect::<Vec<_>>();
            revalidate_para_public_items(config, &refs)
        })();
        if current_authority.is_err() {
            let retained = move_para_directory_to_unique_claim(
                &target,
                private_root,
                PARA_PERSON_FAILED_PREFIX,
                &published.directory,
            )?;
            if retained.is_none() {
                return Err(
                    "revoked recovered PARA generation changed before private quarantine".into(),
                );
            }
            sync_para_directory(people)?;
            sync_para_directory(private_root)?;
            return Ok(());
        }
    }

    let public_proof = || -> Result<(), Box<dyn std::error::Error>> {
        if let Some(intended) = manifest.intended.as_ref() {
            inspect_para_generation_with_proof(&target, intended).map(|_| ())
        } else {
            attest_path_is_absent(&target)
        }
    };
    retain_para_captured_member(
        &capture.join("items.json"),
        &manifest.old.items,
        &public_proof,
    )?;
    if let Some(summary) = manifest.old.summary.as_ref() {
        retain_para_captured_member(&capture.join("summary.md"), summary, &public_proof)?;
    }
    attest_para_capture_shape(&capture, &manifest.old)?;
    if !matches!(
        inspect_para_captured_member(&capture.join("items.json"), &manifest.old.items)?,
        ParaCapturedMemberState::RetainedOld(_) | ParaCapturedMemberState::LegacyZero
    ) || manifest.old.summary.as_ref().is_some_and(|summary| {
        !matches!(
            inspect_para_captured_member(&capture.join("summary.md"), summary),
            Ok(ParaCapturedMemberState::RetainedOld(_) | ParaCapturedMemberState::LegacyZero)
        )
    }) {
        return Err(
            "PARA transaction cannot complete before every member is exactly retained".into(),
        );
    }
    public_proof()?;
    sync_para_directory(&capture)?;
    sync_para_directory(people)?;
    sync_para_directory(private_root)?;
    finish_para_transaction(transaction, people)
}

fn recover_para_transactions(
    people: &Path,
    private_root: &Path,
    config: Option<&Config>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !people.exists() || !private_root.exists() {
        return Ok(());
    }
    let mut manifests = Vec::new();
    let mut completed = Vec::new();
    manifests
        .try_reserve(MAX_PARA_RECONCILIATION_ITEMS)
        .map_err(|_| "PARA transaction scan allocation failed")?;
    let mut scanned = 0usize;
    for entry in fs::read_dir(private_root)? {
        scanned = scanned
            .checked_add(1)
            .ok_or("PARA people entry count overflowed")?;
        if scanned > MAX_PARA_RECONCILIATION_ITEMS {
            return Err("PARA people directory exceeded its bounded entry scan".into());
        }
        let entry = entry?;
        if let Some(name) = entry.file_name().to_str() {
            if name.starts_with(PARA_PERSON_TRANSACTION_PREFIX) {
                manifests.push(entry.path());
            } else if name.starts_with(PARA_PERSON_COMPLETED_TRANSACTION_PREFIX) {
                completed.push(entry.path());
            }
        }
    }
    completed.sort();
    for path in completed {
        let identity = preserved_source_identity_with_limit(&path, MAX_PARA_TRANSACTION_BYTES)?;
        if identity.bytes.is_empty() {
            continue;
        }
        let file = open_regular_file_no_follow_for_update(&path)?;
        attest_visible_exact_file(&path, &file, &identity.bytes)?;
        let manifest: ParaTransactionManifest = serde_json::from_slice(&identity.bytes)?;
        cleanup_completed_para_transaction(people, private_root, &path, &file, &manifest)?;
    }
    manifests.sort();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mut handled_recyclable = BTreeSet::new();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let mut bases = BTreeSet::new();
        for path in &manifests {
            let text = path.to_string_lossy();
            if let Some(base) = text.strip_suffix(".a").or_else(|| text.strip_suffix(".b")) {
                bases.insert(PathBuf::from(base));
            }
        }
        for base in bases {
            let a = PathBuf::from(format!("{}.a", base.to_string_lossy()));
            let b = PathBuf::from(format!("{}.b", base.to_string_lossy()));
            handled_recyclable.insert(a.clone());
            handled_recyclable.insert(b.clone());
            let reads = [
                (a.clone(), read_recyclable_para_record(&a)?),
                (b.clone(), read_recyclable_para_record(&b)?),
            ];
            let mut valid = reads
                .iter()
                .filter_map(|(path, read)| match read {
                    RecyclableParaRecordRead::Valid(record, file) if record.schema == 2 => Some((
                        path.clone(),
                        record.as_ref().clone(),
                        file.try_clone().ok()?,
                    )),
                    _ => None,
                })
                .collect::<Vec<_>>();
            valid.sort_by_key(|(_, record, _)| record.sequence);
            let Some((latest_path, latest, latest_file)) = valid.pop() else {
                if reads
                    .iter()
                    .any(|(_, read)| matches!(read, RecyclableParaRecordRead::Malformed(_)))
                {
                    return Err("PARA recyclable journals are malformed without an independent terminal receipt".into());
                }
                continue;
            };
            if recyclable_para_transaction_paths(private_root, &latest.target_name)
                != (a.clone(), b.clone())
            {
                return Err("PARA recyclable journal pathname does not match its target".into());
            }
            match latest.journal_state {
                Some(RecyclableParaJournalState::Active) => {
                    if latest.sequence % 2 != 1
                        || latest.prior_sequence != latest.sequence.checked_sub(1)
                    {
                        return Err("PARA active journal sequence is not monotonic".into());
                    }
                    let mut transaction = ActiveParaTransaction {
                        path: latest_path,
                        file: latest_file,
                        manifest: latest,
                    };
                    recover_recyclable_para_transaction(
                        people,
                        private_root,
                        config,
                        &mut transaction,
                    )?;
                }
                Some(
                    RecyclableParaJournalState::Baseline | RecyclableParaJournalState::Completed,
                ) => {
                    if latest.sequence % 2 != 0 {
                        return Err("PARA terminal journal sequence is not monotonic".into());
                    }
                    // A malformed sibling is reset only while this independent
                    // receipt still proves the exact terminal P/S/D layout.
                    // If a corrupted active record already advanced the
                    // filesystem, this proof fails and nothing is reset.
                    if let Err(stale_error) =
                        attest_recyclable_journal_layout(people, private_root, &latest)
                    {
                        let sibling_is_empty = reads.iter().any(|(path, read)| {
                            path != &latest_path && matches!(read, RecyclableParaRecordRead::Empty)
                        });
                        let target = people.join(&latest.target_name);
                        let slot = private_root.join(&latest.capture_name);
                        let (_, parked) = recyclable_para_generation_paths(private_root, &target)?;
                        let canonical_names =
                            [OsString::from("items.json"), OsString::from("summary.md")]
                                .into_iter()
                                .collect::<BTreeSet<_>>();
                        let current_public_is_reconcilable =
                            inspect_para_person(&target).is_ok_and(|snapshot| {
                                snapshot.entry_names == canonical_names
                                    && snapshot.summary.is_some()
                            }) || fs::symlink_metadata(&target)
                                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound);
                        if !sibling_is_empty
                            || !current_public_is_reconcilable
                            || !recyclable_generation_is_tombstone(&slot)
                            || attest_path_is_absent(&parked).is_err()
                        {
                            return Err(stale_error);
                        }
                        // Public person bytes may legitimately change between
                        // reconciliations. With an exactly empty sibling and
                        // unchanged terminal topology, defer receipt refresh
                        // until the policy pass has classified those bytes and
                        // starts a concrete mutation. No record is reset here.
                        continue;
                    }
                    for (path, record, file) in &valid {
                        if record.journal_state == Some(RecyclableParaJournalState::Active)
                            && record.sequence.checked_add(1) == Some(latest.sequence)
                        {
                            reset_exact_recyclable_record(path, file)?;
                        } else if matches!(
                            record.journal_state,
                            Some(
                                RecyclableParaJournalState::Baseline
                                    | RecyclableParaJournalState::Completed
                            )
                        ) && record.sequence.checked_add(2) == Some(latest.sequence)
                        {
                            // Receipt refresh writes the new exact baseline
                            // before retiring the stale terminal record. If a
                            // crash lands between those operations, the newer
                            // independently proven receipt authorizes exact
                            // reset of only its retained predecessor.
                            reset_exact_recyclable_record(path, file)?;
                        }
                    }
                    for (path, read) in &reads {
                        if let RecyclableParaRecordRead::Malformed(file) = read {
                            reset_exact_recyclable_record(path, file)?;
                        }
                    }
                }
                None => return Err("schema-2 PARA journal has no state".into()),
            }
        }
    }
    let mut active = 0usize;
    for path in manifests {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if handled_recyclable.contains(&path) {
            continue;
        }
        let identity = preserved_source_identity_with_limit(&path, MAX_PARA_TRANSACTION_BYTES)?;
        if identity.bytes.is_empty() {
            continue;
        }
        active = active
            .checked_add(1)
            .ok_or("PARA transaction count overflowed")?;
        if active > MAX_PARA_PERSON_GENERATIONS {
            return Err("PARA transaction recovery capacity is exhausted".into());
        }
        let file = open_regular_file_no_follow_for_update(&path)?;
        attest_visible_exact_file(&path, &file, &identity.bytes)?;
        let manifest: ParaTransactionManifest = serde_json::from_slice(&identity.bytes)?;
        let mut transaction = ActiveParaTransaction {
            path,
            file,
            manifest,
        };
        recover_one_para_transaction(people, private_root, config, &mut transaction)?;
    }
    Ok(())
}

fn sync_para_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let directory = para_open_directory_no_follow(path)?;
    directory.sync_all().map_err(|error| {
        format!(
            "PARA directory metadata durability could not be established for {} ({error}); the operation remains uncommitted and any existing transaction journal is retained",
            path.display()
        )
    })?;
    Ok(())
}

fn sync_para_directory_handle(directory: &File) -> Result<(), Box<dyn std::error::Error>> {
    directory.sync_all().map_err(|error| {
        format!(
            "PARA directory metadata durability could not be established ({error}); the operation remains uncommitted and any existing transaction journal is retained"
        )
    })?;
    Ok(())
}

fn bounded_para_entry_names(
    directory: &Path,
    limit: usize,
) -> Result<BTreeSet<OsString>, Box<dyn std::error::Error>> {
    let mut names = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        if names.len() >= limit {
            return Err("PARA directory exceeded its bounded entry scan".into());
        }
        let name = entry?.file_name();
        if !names.insert(name) {
            return Err("PARA directory duplicated an entry during inspection".into());
        }
    }
    Ok(names)
}

fn para_private_residue_name(name: &str) -> bool {
    name == ".minutes-private-reconcile"
        || name.starts_with(".minutes-publish-")
        || name == "items.json.tmp"
        || name == "summary.md.tmp"
}

fn para_public_person_directory(path: &Path, people: &Path) -> bool {
    path.parent() == Some(people)
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| !name.starts_with(".minutes-person-"))
}

fn inspect_para_person(directory: &Path) -> Result<ParaPersonSnapshot, Box<dyn std::error::Error>> {
    let opened_directory = para_open_directory_no_follow(directory)?;
    let mut public_entries = 0usize;
    let mut entry_names = BTreeSet::new();
    let mut items = None;
    let mut summary = None;
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        public_entries = public_entries
            .checked_add(1)
            .ok_or("PARA person entry count overflowed")?;
        if public_entries > MAX_PARA_RECONCILIATION_ITEMS {
            return Err("PARA person exceeded the reconciliation entry bound".into());
        }
        let name = entry.file_name();
        if !entry_names.insert(name.clone()) {
            return Err("PARA person duplicated a directory entry".into());
        }
        let Some(name_text) = name.to_str() else {
            return Err("PARA person contained a non-UTF-8 entry".into());
        };
        if para_private_residue_name(name_text) {
            // Existing owner-private evidence remains with the exact old
            // directory claim. It is never copied into a public successor and
            // never deleted through this mutable pathname.
            continue;
        }
        match name_text {
            "items.json" => {
                if items.is_some() {
                    return Err("PARA person duplicated items.json".into());
                }
                items = Some(preserved_source_identity_with_limit(
                    &entry.path(),
                    MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                )?);
            }
            "summary.md" => {
                if summary.is_some() {
                    return Err("PARA person duplicated summary.md".into());
                }
                summary = Some(preserved_source_identity_with_limit(
                    &entry.path(),
                    MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                )?);
            }
            _ => {
                return Err(format!(
                    "PARA person contains unsupported entry {name_text}; refusing a non-atomic privacy rewrite"
                )
                .into());
            }
        }
    }
    let items = items.ok_or("PARA person has no items.json")?;
    let combined = items
        .len
        .checked_add(summary.as_ref().map_or(0, |identity| identity.len))
        .ok_or("PARA person input size overflowed")?;
    if combined > MAX_PARA_PERSON_RECONCILIATION_BYTES {
        return Err("PARA person exceeded the aggregate reconciliation byte bound".into());
    }
    let final_names = bounded_para_entry_names(directory, MAX_PARA_RECONCILIATION_ITEMS)?;
    if final_names != entry_names
        || !file_identity_matches_path(&opened_directory, directory)
        || !preserved_identity_matches(&directory.join("items.json"), &items)
        || summary.as_ref().is_some_and(|identity| {
            !preserved_identity_matches(&directory.join("summary.md"), identity)
        })
    {
        return Err("PARA person changed while its generation was inspected".into());
    }
    Ok(ParaPersonSnapshot {
        directory: opened_directory,
        entry_names,
        items,
        summary,
    })
}

fn para_snapshot_matches_path(path: &Path, snapshot: &ParaPersonSnapshot) -> bool {
    if !file_identity_matches_path(&snapshot.directory, path) {
        return false;
    }
    let Ok(names) = bounded_para_entry_names(path, MAX_PARA_RECONCILIATION_ITEMS) else {
        return false;
    };
    names == snapshot.entry_names
        && preserved_identity_matches(&path.join("items.json"), &snapshot.items)
        && snapshot
            .summary
            .as_ref()
            .is_none_or(|identity| preserved_identity_matches(&path.join("summary.md"), identity))
}

fn require_para_generation_capacity(
    private_root: &Path,
    additional_slots: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut retained = 0usize;
    let mut scanned = 0usize;
    for entry in fs::read_dir(private_root)? {
        scanned = scanned
            .checked_add(1)
            .ok_or("PARA people entry count overflowed")?;
        if scanned > MAX_PARA_RECONCILIATION_ITEMS {
            return Err("PARA people directory exceeded its bounded entry scan".into());
        }
        let entry = entry?;
        if entry.file_name().to_str().is_some_and(|name| {
            name.starts_with(PARA_PERSON_STAGE_PREFIX)
                || name.starts_with(PARA_PERSON_CAPTURE_PREFIX)
                || name.starts_with(PARA_PERSON_FAILED_PREFIX)
        }) {
            retained = retained
                .checked_add(1)
                .ok_or("PARA generation count overflowed")?;
        }
    }
    if retained
        .checked_add(additional_slots)
        .is_none_or(|required| required > MAX_PARA_PERSON_GENERATIONS)
    {
        return Err("PARA person generation recovery capacity is exhausted".into());
    }
    Ok(())
}

fn unique_para_generation_path(
    people: &Path,
    prefix: &str,
    seed_path: &Path,
    attempt: u64,
) -> PathBuf {
    let seed = format!(
        "{}:{}:{}:{attempt}",
        seed_path.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    people.join(format!("{prefix}{:016x}", content_revision(&seed)))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_para_generation_paths(
    private_root: &Path,
    directory: &Path,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let target = directory
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_para_transaction_component(name, None))
        .ok_or("PARA recyclable target name is invalid")?;
    let digest = Sha256::digest(target.as_bytes());
    Ok((
        private_root.join(format!("{PARA_PERSON_SLOT_PREFIX}{digest:x}")),
        private_root.join(format!("{PARA_PERSON_PARKED_PREFIX}{digest:x}")),
    ))
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn recyclable_para_expected(
    items: Vec<u8>,
    summary: Vec<u8>,
) -> Result<ParaExpectedFiles, Box<dyn std::error::Error>> {
    let combined = u64::try_from(items.len())?
        .checked_add(u64::try_from(summary.len())?)
        .ok_or("PARA successor size overflowed")?;
    if combined > MAX_PARA_PERSON_RECONCILIATION_BYTES {
        return Err("PARA successor exceeded the aggregate reconciliation byte bound".into());
    }
    Ok(vec![
        (OsString::from("items.json"), items),
        (OsString::from("summary.md"), summary),
    ])
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn create_recyclable_para_directory(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let parent = path.parent().ok_or("PARA slot has no parent")?;
    let name = path.file_name().ok_or("PARA slot has no name")?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(parent)?;
    let parent_file = para_open_directory_no_follow(parent)?;
    let parent_cap = CapDir::from_std_file(parent_file);
    match parent_cap.create_dir(name) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let opened = parent_cap
        .open_with(name, &para_open_directory_options())?
        .into_std();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        opened.set_permissions(fs::Permissions::from_mode(0o700))?;
    }
    boundary.attest_for_source_cleanup()?;
    sync_para_directory_handle(&opened)?;
    sync_para_directory(parent)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ensure_recyclable_para_members(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    create_recyclable_para_directory(path)?;
    let directory = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    let opened = para_open_directory_no_follow(path)?;
    let cap = CapDir::from_std_file(opened.try_clone()?);
    for name in [OsStr::new("items.json"), OsStr::new("summary.md")] {
        match cap.symlink_metadata(name) {
            Ok(_) => {
                let member = directory.bind_exact_file(name)?;
                if !member.is_empty()? {
                    return Err(
                        "PARA recyclable slot was not an authenticated zero tombstone".into(),
                    );
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = CapOpenOptions::new();
                options.create_new(true).read(true).write(true);
                #[cfg(unix)]
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
                let file = cap.open_with(name, &options)?.into_std();
                set_restrictive_permissions_file(&file)?;
                file.sync_all()?;
            }
            Err(error) => return Err(error.into()),
        }
    }
    let expected = [OsString::from("items.json"), OsString::from("summary.md")]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if bounded_para_entry_names(path, 2)? != expected {
        return Err("PARA recyclable slot contains unrecognized retained entries".into());
    }
    sync_para_directory_handle(&opened)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn attest_recyclable_para_tombstone(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    let directory = para_open_directory_no_follow(path)?;
    let expected = [OsString::from("items.json"), OsString::from("summary.md")]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if bounded_para_entry_names(path, 2)? != expected {
        return Err("PARA recyclable tombstone has an unexpected shape".into());
    }
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    for name in [OsStr::new("items.json"), OsStr::new("summary.md")] {
        if !boundary.bind_exact_file(name)?.is_empty()? {
            return Err("PARA recyclable tombstone retained plaintext".into());
        }
    }
    if !file_identity_matches_path(&directory, path) {
        return Err("PARA recyclable tombstone directory changed".into());
    }
    Ok(directory)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn fill_recyclable_para_tombstone(
    path: &Path,
    expected: &ParaExpectedFiles,
) -> Result<ParaPersonSuccessor, Box<dyn std::error::Error>> {
    ensure_recyclable_para_members(path)?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    let opened = para_open_directory_no_follow(path)?;
    for (name, bytes) in expected {
        let member = boundary.bind_exact_file(name)?;
        if !member.is_empty()? {
            return Err("PARA recyclable slot changed before refill".into());
        }
        let mut exact = member.try_clone_exact_file()?;
        exact.seek(SeekFrom::Start(0))?;
        exact.write_all(bytes)?;
        exact.sync_all()?;
        member.recovery_proof_for_exact_bytes_bounded(
            bytes,
            MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
            Instant::now() + Duration::from_secs(5),
        )?;
    }
    sync_para_directory_handle(&opened)?;
    sync_para_directory(path.parent().ok_or("PARA recyclable slot has no parent")?)?;
    let successor = ParaPersonSuccessor {
        path: path.to_path_buf(),
        directory: opened,
        expected: expected.clone(),
    };
    attest_para_successor(path, &successor.directory, &successor.expected)?;
    Ok(successor)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn scrub_recyclable_para_generation(
    path: &Path,
    proof: &ParaGenerationProof,
) -> Result<(), Box<dyn std::error::Error>> {
    scrub_recyclable_para_generation_with_hooks(path, proof, |_| {}, |_| {})
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn scrub_recyclable_para_generation_with_hooks(
    path: &Path,
    proof: &ParaGenerationProof,
    after_items_sync: impl FnOnce(&Path),
    after_summary_sync: impl FnOnce(&Path),
) -> Result<(), Box<dyn std::error::Error>> {
    let expected = [OsString::from("items.json"), OsString::from("summary.md")]
        .into_iter()
        .collect::<BTreeSet<_>>();
    if proof
        .entry_names
        .iter()
        .map(OsString::from)
        .collect::<BTreeSet<_>>()
        != expected
        || proof.summary.is_none()
        || bounded_para_entry_names(path, 2)? != expected
    {
        return Err("legacy PARA generation cannot enter the recyclable fixed slot".into());
    }
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(path)?;
    for (name, member_proof, after_sync) in [
        (
            OsStr::new("items.json"),
            &proof.items,
            Box::new(after_items_sync) as Box<dyn FnOnce(&Path)>,
        ),
        (
            OsStr::new("summary.md"),
            proof
                .summary
                .as_ref()
                .ok_or("PARA generation lost summary")?,
            Box::new(after_summary_sync) as Box<dyn FnOnce(&Path)>,
        ),
    ] {
        let member_path = path.join(name);
        let member = boundary.bind_exact_file(name)?;
        match inspect_para_captured_member(&member_path, member_proof)? {
            ParaCapturedMemberState::LegacyZero => {
                member.recovery_proof_for_exact_bytes_bounded(
                    &[],
                    MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                    Instant::now() + Duration::from_secs(5),
                )?;
            }
            ParaCapturedMemberState::RetainedOld(expected) => {
                member.recovery_proof_for_exact_bytes_bounded(
                    &expected.bytes,
                    MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES,
                    Instant::now() + Duration::from_secs(5),
                )?;
                member.zero_exact_for_retirement()?;
            }
        }
        after_sync(&member_path);
    }
    sync_para_directory(path)?;
    sync_para_directory(path.parent().ok_or("PARA slot has no parent")?)?;
    attest_recyclable_para_tombstone(path)?;
    Ok(())
}

fn create_para_successor(
    private_root: &Path,
    directory: &Path,
    items: Vec<u8>,
    summary: Vec<u8>,
) -> Result<ParaPersonSuccessor, Box<dyn std::error::Error>> {
    let combined = u64::try_from(items.len())?
        .checked_add(u64::try_from(summary.len())?)
        .ok_or("PARA successor size overflowed")?;
    if combined > MAX_PARA_PERSON_RECONCILIATION_BYTES {
        return Err("PARA successor exceeded the aggregate reconciliation byte bound".into());
    }
    require_para_generation_capacity(private_root, 1)?;
    let boundary = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(private_root)?;
    boundary.attest_for_source_cleanup()?;
    let stage = (0..100u64)
        .find_map(|attempt| {
            let path = unique_para_generation_path(
                private_root,
                PARA_PERSON_STAGE_PREFIX,
                directory,
                attempt,
            );
            let name = path.file_name().expect("generated PARA stage has a name");
            match boundary.create_new_owner_private_child(name) {
                Ok(stage) => Some(Ok(stage)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => Some(Err(error)),
            }
        })
        .transpose()
        .map_err(|error| format!("private PARA stage allocation failed: {error}"))?
        .ok_or("could not allocate a private PARA successor generation")?;
    let path = stage.display_path().to_path_buf();
    let stage_name = path.file_name().ok_or("PARA stage has no name")?;
    let stage_proof = stage
        .recovery_directory_proof()
        .map_err(|error| format!("private PARA stage identity proof failed: {error}"))?;
    drop(stage);
    let people_file = para_open_directory_no_follow(private_root)
        .map_err(|error| format!("private PARA parent reopen failed: {error}"))?;
    let people_cap = CapDir::from_std_file(people_file);
    let opened = people_cap
        .open_with(stage_name, &para_open_directory_options())
        .map_err(|error| format!("private PARA stage reopen failed: {error}"))?
        .into_std();
    stage_proof
        .attest_exact_owner_private_directory_file(&opened)
        .map_err(|error| format!("private PARA stage handle comparison failed: {error}"))?;
    boundary
        .attest_for_source_cleanup()
        .map_err(|error| format!("private PARA parent recheck failed: {error}"))?;
    let stage_cap = CapDir::from_std_file(opened.try_clone()?);
    let expected = vec![
        (OsString::from("items.json"), items),
        (OsString::from("summary.md"), summary),
    ];
    for (name, bytes) in &expected {
        let destination = path.join(name);
        let mut options = CapOpenOptions::new();
        options.create_new(true).read(true).write(true);
        #[cfg(unix)]
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        let mut file = stage_cap
            .open_with(name, &options)
            .map_err(|error| format!("private PARA member creation failed: {error}"))?
            .into_std();
        set_restrictive_permissions_file(&file)
            .map_err(|error| format!("private PARA member permission check failed: {error}"))?;
        file.write_all(bytes)
            .map_err(|error| format!("private PARA member write failed: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("private PARA member durability failed: {error}"))?;
        attest_visible_exact_file(&destination, &file, bytes)
            .map_err(|error| format!("private PARA member attestation failed: {error}"))?;
    }
    boundary
        .attest_for_source_cleanup()
        .map_err(|error| format!("private PARA parent final recheck failed: {error}"))?;
    sync_para_directory_handle(&opened)?;
    sync_para_directory(private_root)?;
    let successor = ParaPersonSuccessor {
        path,
        directory: opened,
        expected,
    };
    attest_para_successor(&successor.path, &successor.directory, &successor.expected)
        .map_err(|error| format!("private PARA successor final attestation failed: {error}"))?;
    Ok(successor)
}

fn attest_para_successor(
    path: &Path,
    directory: &File,
    expected: &[(OsString, Vec<u8>)],
) -> Result<(), Box<dyn std::error::Error>> {
    if !file_identity_matches_path(directory, path) {
        return Err("PARA successor directory identity changed".into());
    }
    let names = bounded_para_entry_names(path, expected.len())?;
    let mut expected_names = expected
        .iter()
        .map(|(name, _)| name.clone())
        .collect::<Vec<_>>();
    expected_names.sort();
    if names != expected_names.iter().cloned().collect() {
        return Err("PARA successor directory entries changed".into());
    }
    for (name, bytes) in expected {
        let file = open_regular_file_no_follow(&path.join(name))?;
        attest_visible_exact_file(&path.join(name), &file, bytes)?;
    }
    if !file_identity_matches_path(directory, path) {
        return Err("PARA successor directory changed at final attestation".into());
    }
    let final_names = bounded_para_entry_names(path, expected.len())?;
    if final_names != expected_names.into_iter().collect() {
        return Err("PARA successor directory changed at the entry boundary".into());
    }
    Ok(())
}

fn move_para_directory_to_unique_claim(
    source: &Path,
    private_root: &Path,
    prefix: &str,
    expected: &File,
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    if !file_identity_matches_path(expected, source) {
        return Ok(None);
    }
    for attempt in 0..100u64 {
        let claim = unique_para_generation_path(private_root, prefix, source, attempt);
        match move_para_directory_to_claim(source, &claim, expected) {
            Ok(Some(())) => return Ok(Some(claim)),
            Ok(None) => return Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Err("could not allocate a private PARA generation claim".into())
}

fn move_para_directory_to_claim(
    source: &Path,
    claim: &Path,
    expected: &File,
) -> std::io::Result<Option<()>> {
    move_para_directory_to_claim_with_hook(source, claim, expected, |_| {})
}

fn move_para_directory_to_claim_with_hook(
    source: &Path,
    claim: &Path,
    expected: &File,
    after_move_before_attestation: impl FnOnce(&Path),
) -> std::io::Result<Option<()>> {
    if !file_identity_matches_path(expected, source) {
        return Ok(None);
    }
    let source_name = source
        .file_name()
        .ok_or_else(|| std::io::Error::other("PARA source has no name"))?;
    let claim_name = claim
        .file_name()
        .ok_or_else(|| std::io::Error::other("PARA claim has no name"))?;
    let source_parent = source
        .parent()
        .ok_or_else(|| std::io::Error::other("PARA source has no parent"))?;
    let claim_parent = claim
        .parent()
        .ok_or_else(|| std::io::Error::other("PARA claim has no parent"))?;
    let source_boundary = crate::policy_fs::BoundRecoveryDirectory::bind_existing(source_parent)?;
    let claim_boundary = crate::policy_fs::BoundRecoveryDirectory::bind_existing(claim_parent)?;
    let source_file = para_open_directory_no_follow(source_parent)?;
    let claim_file = para_open_directory_no_follow(claim_parent)?;
    source_boundary.attest_for_source_cleanup()?;
    claim_boundary.attest_for_source_cleanup()?;
    let source_parent_cap = CapDir::from_std_file(source_file);
    let claim_parent_cap = CapDir::from_std_file(claim_file);
    let source_cap = source_parent_cap.open_with(source_name, &para_open_directory_options())?;
    let source_file = source_cap.into_std();
    if !qmd_file_handles_match(expected, &source_file) {
        return Ok(None);
    }
    match crate::policy_fs::move_entry_at_no_replace(
        &source_parent_cap,
        source_name,
        &source_file,
        &claim_parent_cap,
        claim_name,
    ) {
        Ok(()) => {
            after_move_before_attestation(claim);
            source_boundary.attest_for_source_cleanup()?;
            claim_boundary.attest_for_source_cleanup()?;
            let claimed = claim_parent_cap
                .open_with(claim_name, &para_open_directory_options())?
                .into_std();
            if qmd_file_handles_match(expected, &claimed) {
                sync_para_directory(source_parent)
                    .and_then(|_| sync_para_directory(claim_parent))
                    .map_err(|error| std::io::Error::other(error.to_string()))?;
                Ok(Some(()))
            } else {
                Err(std::io::Error::other(
                    "PARA directory changed during exact generation claim; the mismatched private claim was preserved and public rollback was refused",
                ))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn attest_para_public_successor(
    directory: &Path,
    successor: Option<&ParaPersonSuccessor>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(successor) = successor {
        attest_para_successor(directory, &successor.directory, &successor.expected)
    } else {
        attest_path_is_absent(directory)
    }
}

fn retain_fresh_para_successor(
    private_root: &Path,
    directory: &Path,
    expected: &[(OsString, Vec<u8>)],
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let items = expected
        .iter()
        .find(|(name, _)| name == "items.json")
        .map(|(_, bytes)| bytes.clone())
        .ok_or("PARA intended generation lost its items.json proof")?;
    let summary = expected
        .iter()
        .find(|(name, _)| name == "summary.md")
        .map(|(_, bytes)| bytes.clone())
        .ok_or("PARA intended generation lost its summary.md proof")?;
    let retained = create_para_successor(private_root, directory, items, summary)?;
    Ok(retained.path)
}

fn replace_para_person_generation_with_hook(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_publication_before_attestation: impl FnOnce(&Path),
) -> Result<(), Box<dyn std::error::Error>> {
    replace_para_person_generation_with_hooks(
        directory,
        private_root,
        snapshot,
        successor_items,
        successor_summary,
        |_| {},
        after_publication_before_attestation,
    )
}

fn replace_para_person_generation_with_hooks(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_old_claim_before_publication: impl FnOnce(&Path),
    after_publication_before_attestation: impl FnOnce(&Path),
) -> Result<(), Box<dyn std::error::Error>> {
    replace_para_person_generation_with_retirement_hooks(
        directory,
        private_root,
        snapshot,
        successor_items,
        successor_summary,
        after_old_claim_before_publication,
        after_publication_before_attestation,
        (|_| {}, |_| {}),
    )
}

#[allow(clippy::too_many_arguments)]
fn replace_para_person_generation_with_retirement_hooks(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_old_claim_before_publication: impl FnOnce(&Path),
    after_publication_before_attestation: impl FnOnce(&Path),
    retirement_sync_hooks: (impl FnOnce(&Path), impl FnOnce(&Path)),
) -> Result<(), Box<dyn std::error::Error>> {
    replace_para_person_generation_with_authorization(
        directory,
        private_root,
        snapshot,
        successor_items,
        successor_summary,
        after_old_claim_before_publication,
        after_publication_before_attestation,
        retirement_sync_hooks,
        || Ok(()),
    )
}

#[allow(clippy::too_many_arguments)]
fn replace_para_person_generation_with_authorization(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_old_claim_before_publication: impl FnOnce(&Path),
    after_publication_before_attestation: impl FnOnce(&Path),
    retirement_sync_hooks: (impl FnOnce(&Path), impl FnOnce(&Path)),
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        replace_para_person_generation_recyclable(
            directory,
            private_root,
            snapshot,
            successor_items,
            successor_summary,
            after_old_claim_before_publication,
            after_publication_before_attestation,
            retirement_sync_hooks,
            authorize_successor,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        replace_para_person_generation_legacy(
            directory,
            private_root,
            snapshot,
            successor_items,
            successor_summary,
            after_old_claim_before_publication,
            after_publication_before_attestation,
            retirement_sync_hooks,
            authorize_successor,
        )
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn replace_para_person_generation_legacy(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_old_claim_before_publication: impl FnOnce(&Path),
    after_publication_before_attestation: impl FnOnce(&Path),
    retirement_sync_hooks: (impl FnOnce(&Path), impl FnOnce(&Path)),
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (after_items_retirement_sync, after_summary_retirement_sync) = retirement_sync_hooks;
    let people = directory.parent().ok_or("PARA person has no parent")?;
    require_para_generation_capacity(private_root, 3)?;
    let mut successor = match (successor_items, successor_summary) {
        (Some(items), Some(summary)) => Some(
            create_para_successor(private_root, directory, items, summary)
                .map_err(|error| format!("PARA private successor creation failed: {error}"))?,
        ),
        (None, None) => None,
        _ => return Err("PARA successor generation was incomplete".into()),
    };
    if !para_snapshot_matches_path(directory, snapshot) {
        return Err("PARA person changed before its generation claim".into());
    }
    let capture = (0..100u64)
        .map(|attempt| {
            unique_para_generation_path(
                private_root,
                PARA_PERSON_CAPTURE_PREFIX,
                directory,
                attempt,
            )
        })
        .find(|path| {
            fs::symlink_metadata(path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        })
        .ok_or("could not reserve a private PARA capture generation name")?;
    let target_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_para_transaction_component(name, None))
        .ok_or("PARA person target name is invalid")?
        .to_string();
    let stage_name = successor
        .as_ref()
        .map(|successor| {
            successor
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| {
                    valid_para_transaction_component(name, Some(PARA_PERSON_STAGE_PREFIX))
                })
                .map(ToOwned::to_owned)
                .ok_or("PARA successor stage name is invalid")
        })
        .transpose()?;
    let capture_name = capture
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| valid_para_transaction_component(name, Some(PARA_PERSON_CAPTURE_PREFIX)))
        .ok_or("PARA capture name is invalid")?
        .to_string();
    let mut transaction = begin_para_transaction(
        private_root,
        ParaTransactionManifest {
            schema: 1,
            target_name,
            stage_name,
            capture_name,
            old: para_snapshot_proof(snapshot)?,
            intended: successor
                .as_ref()
                .map(|successor| para_successor_proof(&successor.expected))
                .transpose()?,
            slot_directory_identity: None,
            slot_items_identity: None,
            slot_summary_identity: None,
            sequence: 0,
            journal_state: None,
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        },
    )
    .map_err(|error| format!("PARA private transaction start failed: {error}"))?;
    move_para_directory_to_claim(directory, &capture, &snapshot.directory)
        .map_err(|error| format!("PARA old-generation claim failed: {error}"))?
        .ok_or("PARA person changed before its exact generation could be claimed")?;
    if !para_snapshot_matches_path(&capture, snapshot) {
        return Err("PARA person changed inside its exact generation claim".into());
    }
    after_old_claim_before_publication(&capture);

    if let Some(staged) = successor.as_mut() {
        authorize_successor()?;
        match move_para_directory_to_claim(
            &staged.path,
            directory,
            &staged.directory,
        ) {
            Err(error) => return Err(format!(
                "PARA successor publication failed ({error}); old and intended generations were retained privately"
            )
            .into()),
            Ok(None) => return Err("PARA successor changed before publication; old and intended generations were retained privately".into()),
            Ok(Some(())) => {}
        }
        staged.path = directory.to_path_buf();
        after_publication_before_attestation(directory);
        if let Err(error) = attest_para_public_successor(directory, Some(staged)) {
            let claim = move_para_directory_to_unique_claim(
                directory,
                private_root,
                PARA_PERSON_FAILED_PREFIX,
                &staged.directory,
            );
            let retention = retain_fresh_para_successor(private_root, directory, &staged.expected);
            let claim_status = match claim {
                Ok(Some(path)) => format!("suspect generation was retained at {}", path.display()),
                Ok(None) => "a pathname winner was preserved".to_string(),
                Err(claim_error) => {
                    format!("exact failure claim was incomplete ({claim_error})")
                }
            };
            let retention_status = match retention {
                Ok(path) => format!(
                    "intended generation was freshly retained at {}",
                    path.display()
                ),
                Err(retention_error) => {
                    format!("intended generation could not be freshly retained ({retention_error})")
                }
            };
            let transaction_status =
                "transaction manifest was retained because captured members were not retired";
            return Err(format!(
                "PARA successor proof failed ({error}); {claim_status}; {retention_status}; {transaction_status}; exact old generation remains private"
            )
            .into());
        }
    } else {
        after_publication_before_attestation(directory);
        attest_path_is_absent(directory)?;
    }

    let old_proof = &transaction.manifest.old;
    let retirement = (|| -> Result<(), Box<dyn std::error::Error>> {
        retain_para_captured_member(&capture.join("items.json"), &old_proof.items, || {
            attest_para_public_successor(directory, successor.as_ref())
        })?;
        after_items_retirement_sync(&capture.join("items.json"));
        if let Some(summary_proof) = old_proof.summary.as_ref() {
            retain_para_captured_member(&capture.join("summary.md"), summary_proof, || {
                attest_para_public_successor(directory, successor.as_ref())
            })?;
            after_summary_retirement_sync(&capture.join("summary.md"));
        }
        Ok(())
    })();
    if let Err(error) = retirement {
        let transaction_status = "transaction manifest was retained for idempotent recovery";
        if let Some(staged) = successor.as_ref() {
            return match retain_fresh_para_successor(private_root, directory, &staged.expected) {
                Ok(path) => Err(format!(
                    "PARA old-generation retirement failed ({error}); intended successor was freshly retained at {}; {transaction_status}",
                    path.display(),
                )
                .into()),
                Err(retention_error) => Err(format!(
                    "PARA old-generation retirement failed ({error}); intended successor could not be freshly retained ({retention_error}); {transaction_status}"
                )
                .into()),
            };
        }
        return Err(format!(
            "PARA old-generation retirement failed ({error}); {transaction_status}"
        )
        .into());
    }
    attest_para_capture_shape(&capture, old_proof)?;
    if !matches!(
        inspect_para_captured_member(&capture.join("items.json"), &old_proof.items)?,
        ParaCapturedMemberState::RetainedOld(_) | ParaCapturedMemberState::LegacyZero
    ) {
        return Err("PARA items retirement was not durably proven".into());
    }
    if let Some(summary) = old_proof.summary.as_ref() {
        if !matches!(
            inspect_para_captured_member(&capture.join("summary.md"), summary)?,
            ParaCapturedMemberState::RetainedOld(_) | ParaCapturedMemberState::LegacyZero
        ) {
            return Err("PARA summary retirement was not durably proven".into());
        }
    }
    attest_para_public_successor(directory, successor.as_ref())?;
    sync_para_directory(&capture)?;
    sync_para_directory(people)?;
    sync_para_directory(private_root)?;
    finish_para_transaction(&mut transaction, people)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn finish_recyclable_para_transaction(
    transaction: &mut ActiveParaTransaction,
    people: &Path,
    private_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    finish_recyclable_para_transaction_with_terminal_state(transaction, people, private_root, false)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn finish_recyclable_para_transaction_as_deletion(
    transaction: &mut ActiveParaTransaction,
    people: &Path,
    private_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    finish_recyclable_para_transaction_with_terminal_state(transaction, people, private_root, true)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn finish_recyclable_para_transaction_with_terminal_state(
    transaction: &mut ActiveParaTransaction,
    people: &Path,
    private_root: &Path,
    terminal_deleted: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    sync_para_directory(people)?;
    sync_para_directory(private_root)?;
    let bytes = bounded_json_to_vec_pretty(&transaction.manifest, MAX_PARA_TRANSACTION_BYTES)?;
    attest_visible_exact_file(&transaction.path, &transaction.file, &bytes)?;
    let (a, b) = recyclable_para_transaction_paths(private_root, &transaction.manifest.target_name);
    let completed_path = if transaction.path == a { b } else { a };
    let mut completed = transaction.manifest.clone();
    if terminal_deleted {
        completed.intended = None;
    }
    completed.sequence = completed
        .sequence
        .checked_add(1)
        .ok_or("PARA recyclable journal sequence overflowed")?;
    completed.journal_state = Some(RecyclableParaJournalState::Completed);
    attest_recyclable_journal_layout(people, private_root, &completed)?;
    // Publish the new terminal receipt while the exact active intent remains
    // valid. If this write tears, recovery still has the active record; only
    // after the completed receipt is durable may the active slot be reset.
    write_recyclable_para_record(&completed_path, &completed, true)?;
    transaction.file.set_len(0)?;
    transaction.file.sync_all()?;
    let visible = open_regular_file_no_follow_for_update(&transaction.path)?;
    if !qmd_file_handles_match(&transaction.file, &visible) || visible.metadata()?.len() != 0 {
        return Err("PARA recyclable journal changed while it was retired".into());
    }
    sync_para_directory(private_root)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn abort_recyclable_para_transaction(
    transaction: &ActiveParaTransaction,
    private_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes = bounded_json_to_vec_pretty(&transaction.manifest, MAX_PARA_TRANSACTION_BYTES)?;
    attest_visible_exact_file(&transaction.path, &transaction.file, &bytes)?;
    reset_exact_recyclable_record(&transaction.path, &transaction.file)?;
    sync_para_directory(private_root)?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn replace_para_person_generation_recyclable(
    directory: &Path,
    private_root: &Path,
    snapshot: &ParaPersonSnapshot,
    successor_items: Option<Vec<u8>>,
    successor_summary: Option<Vec<u8>>,
    after_old_claim_before_publication: impl FnOnce(&Path),
    after_publication_before_attestation: impl FnOnce(&Path),
    retirement_sync_hooks: (impl FnOnce(&Path), impl FnOnce(&Path)),
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let people = directory.parent().ok_or("PARA person has no parent")?;
    let mut retirement_sync_hooks = Some(retirement_sync_hooks);
    if !para_snapshot_matches_path(directory, snapshot) {
        return Err("PARA person changed before its recyclable exchange".into());
    }
    let old_proof = para_snapshot_proof(snapshot)?;
    if old_proof.summary.is_none()
        || snapshot.entry_names
            != [OsString::from("items.json"), OsString::from("summary.md")]
                .into_iter()
                .collect()
    {
        return Err("legacy PARA generation must be reconciled before fixed-slot recycling".into());
    }
    let (slot, parked) = recyclable_para_generation_paths(private_root, directory)?;
    if fs::symlink_metadata(&parked).is_ok() {
        return Err("PARA parked deletion tombstone is inconsistent with an active person".into());
    }
    let intended = match (successor_items, successor_summary) {
        (Some(items), Some(summary)) => Some(recyclable_para_expected(items, summary)?),
        (None, None) => None,
        _ => return Err("PARA successor generation was incomplete".into()),
    };
    ensure_recyclable_para_members(&slot)?;
    let slot_tombstone = attest_recyclable_para_tombstone(&slot)?;
    let target_name = directory
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("PARA target name is invalid")?
        .to_string();
    let slot_name = slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("PARA slot name is invalid")?
        .to_string();
    let mut transaction = begin_recyclable_para_transaction(
        people,
        private_root,
        ParaTransactionManifest {
            schema: 2,
            target_name,
            stage_name: intended.as_ref().map(|_| slot_name.clone()),
            capture_name: slot_name,
            old: old_proof.clone(),
            intended: intended
                .as_ref()
                .map(|expected| para_successor_proof(expected))
                .transpose()?,
            slot_directory_identity: qmd_file_identity_and_links(&slot_tombstone)
                .map(|value| value.0),
            slot_items_identity: qmd_file_identity_and_links(&open_regular_file_no_follow(
                &slot.join("items.json"),
            )?)
            .map(|value| value.0),
            slot_summary_identity: qmd_file_identity_and_links(&open_regular_file_no_follow(
                &slot.join("summary.md"),
            )?)
            .map(|value| value.0),
            sequence: 0,
            journal_state: Some(RecyclableParaJournalState::Active),
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        },
    )?;
    // The durable intent precedes the first in-place slot byte. A crash while
    // refilling therefore leaves the still-public old generation untouched
    // and an exact, target-specific record that recovery can fail closed on.
    let mut successor = intended
        .as_ref()
        .map(|expected| fill_recyclable_para_tombstone(&slot, expected))
        .transpose()?;
    if let Err(error) = authorize_successor() {
        if let Some(intended_proof) = transaction.manifest.intended.as_ref() {
            scrub_recyclable_para_generation(&slot, intended_proof)?;
        }
        exchange_recyclable_para_generations(people, directory, private_root, &slot)?;
        let (after_items, after_summary) = retirement_sync_hooks
            .take()
            .ok_or("PARA retirement hooks were consumed more than once")?;
        scrub_recyclable_para_generation_with_hooks(&slot, &old_proof, after_items, after_summary)?;
        park_recyclable_public_tombstone(directory, &parked)?;
        finish_recyclable_para_transaction_as_deletion(&mut transaction, people, private_root)?;
        return Err(format!(
            "PARA successor authorization failed ({error}); the prior generation was retracted into fixed zero tombstones"
        )
        .into());
    }

    let public_parent = crate::policy_fs::BoundRecoveryDirectory::bind_existing(people)?;
    let private_parent =
        crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(private_root)?;
    let target_name = directory.file_name().ok_or("PARA target has no name")?;
    let slot_name = slot.file_name().ok_or("PARA slot has no name")?;
    public_parent.exchange_exact_private_children_with_hook(
        target_name,
        &private_parent,
        slot_name,
        || {},
    )?;
    if !para_snapshot_matches_path(&slot, snapshot) {
        return Err("PARA exact old generation did not arrive in its fixed slot".into());
    }
    after_old_claim_before_publication(&slot);
    if let Some(staged) = successor.as_mut() {
        staged.path = directory.to_path_buf();
        after_publication_before_attestation(directory);
        if let Err(error) = attest_para_public_successor(directory, Some(staged)) {
            let claimed = move_para_directory_to_unique_claim(
                directory,
                private_root,
                PARA_PERSON_FAILED_PREFIX,
                &staged.directory,
            )?;
            let retained = retain_fresh_para_successor(private_root, directory, &staged.expected)?;
            if claimed.is_some() {
                let (after_items, after_summary) = retirement_sync_hooks
                    .take()
                    .ok_or("PARA retirement hooks were consumed more than once")?;
                scrub_recyclable_para_generation_with_hooks(
                    &slot,
                    &old_proof,
                    after_items,
                    after_summary,
                )?;
                finish_recyclable_para_transaction(&mut transaction, people, private_root)?;
            }
            return Err(format!(
                "PARA recyclable successor proof failed ({error}); suspect exact generation claim={}; intended bytes retained at {}",
                claimed
                    .as_ref()
                    .map_or_else(|| "pathname winner preserved".to_string(), |path| path.display().to_string()),
                retained.display()
            )
            .into());
        }
    } else {
        if !file_identity_matches_path(&slot_tombstone, directory) {
            return Err("PARA deletion did not publish its exact tombstone".into());
        }
        after_publication_before_attestation(directory);
    }

    let (after_items, after_summary) = retirement_sync_hooks
        .take()
        .ok_or("PARA retirement hooks were consumed more than once")?;
    scrub_recyclable_para_generation_with_hooks(&slot, &old_proof, after_items, after_summary)?;
    if successor.is_none() {
        if fs::symlink_metadata(&parked).is_ok() {
            return Err("PARA parked tombstone name is already occupied".into());
        }
        move_para_directory_to_claim(directory, &parked, &tombstone_for_path(directory)?)?
            .ok_or("PARA public tombstone changed before it could be parked")?;
        attest_path_is_absent(directory)?;
        attest_recyclable_para_tombstone(&parked)?;
    } else {
        attest_para_public_successor(directory, successor.as_ref())?;
        attest_recyclable_para_tombstone(&slot)?;
    }
    finish_recyclable_para_transaction(&mut transaction, people, private_root)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn tombstone_for_path(path: &Path) -> Result<File, Box<dyn std::error::Error>> {
    let tombstone = attest_recyclable_para_tombstone(path)?;
    Ok(tombstone)
}

fn publish_new_para_person_generation(
    directory: &Path,
    private_root: &Path,
    items: Vec<u8>,
    summary: Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    publish_new_para_person_generation_with_authorization(
        directory,
        private_root,
        items,
        summary,
        || Ok(()),
    )
}

fn publish_new_para_person_generation_with_authorization(
    directory: &Path,
    private_root: &Path,
    items: Vec<u8>,
    summary: Vec<u8>,
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        publish_new_para_person_generation_recyclable(
            directory,
            private_root,
            items,
            summary,
            authorize_successor,
        )
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        publish_new_para_person_generation_legacy(
            directory,
            private_root,
            items,
            summary,
            authorize_successor,
        )
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn publish_new_para_person_generation_legacy(
    directory: &Path,
    private_root: &Path,
    items: Vec<u8>,
    summary: Vec<u8>,
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    let people = directory.parent().ok_or("PARA person has no parent")?;
    require_para_generation_capacity(private_root, 2)?;
    let mut successor = create_para_successor(private_root, directory, items, summary)?;
    authorize_successor()?;
    match move_para_directory_to_claim(&successor.path, directory, &successor.directory) {
        Err(error) => {
            return Err(format!(
            "PARA person publication failed ({error}); intended generation was retained privately"
        )
            .into())
        }
        Ok(None) => return Err("PARA person publication lost its exact staged generation".into()),
        Ok(Some(())) => {}
    }
    successor.path = directory.to_path_buf();
    if let Err(error) = attest_para_public_successor(directory, Some(&successor)) {
        let claim = move_para_directory_to_unique_claim(
            directory,
            private_root,
            PARA_PERSON_FAILED_PREFIX,
            &successor.directory,
        );
        let retention = retain_fresh_para_successor(private_root, directory, &successor.expected);
        let claim_status = match claim {
            Ok(Some(path)) => format!("suspect generation was retained at {}", path.display()),
            Ok(None) => "a pathname winner was preserved".to_string(),
            Err(claim_error) => format!("exact failure claim was incomplete ({claim_error})"),
        };
        let retention_status = match retention {
            Ok(path) => format!(
                "intended generation was freshly retained at {}",
                path.display()
            ),
            Err(retention_error) => {
                format!("intended generation could not be freshly retained ({retention_error})")
            }
        };
        return Err(format!(
            "PARA person publication proof failed ({error}); {claim_status}; {retention_status}"
        )
        .into());
    }
    sync_para_directory(people)?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn publish_new_para_person_generation_recyclable(
    directory: &Path,
    private_root: &Path,
    items: Vec<u8>,
    summary: Vec<u8>,
    authorize_successor: impl FnOnce() -> Result<(), Box<dyn std::error::Error>>,
) -> Result<(), Box<dyn std::error::Error>> {
    attest_path_is_absent(directory)?;
    let people = directory.parent().ok_or("PARA person has no parent")?;
    let (slot, parked) = recyclable_para_generation_paths(private_root, directory)?;
    ensure_recyclable_para_members(&slot)?;
    let expected = recyclable_para_expected(items, summary)?;
    let intended = para_successor_proof(&expected)?;
    let slot_directory = attest_recyclable_para_tombstone(&slot)?;
    let slot_name = slot
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("PARA slot name is invalid")?
        .to_string();
    let mut transaction = begin_recyclable_para_transaction(
        people,
        private_root,
        ParaTransactionManifest {
            schema: 2,
            target_name: directory
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or("PARA target name is invalid")?
                .to_string(),
            stage_name: Some(slot_name.clone()),
            capture_name: slot_name,
            old: intended.clone(),
            intended: Some(intended.clone()),
            slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                .map(|value| value.0),
            slot_items_identity: qmd_file_identity_and_links(&open_regular_file_no_follow(
                &slot.join("items.json"),
            )?)
            .map(|value| value.0),
            slot_summary_identity: qmd_file_identity_and_links(&open_regular_file_no_follow(
                &slot.join("summary.md"),
            )?)
            .map(|value| value.0),
            sequence: 0,
            journal_state: Some(RecyclableParaJournalState::Active),
            baseline_deleted: true,
            baseline_parked: fs::symlink_metadata(&parked).is_ok(),
            prior_sequence: None,
        },
    )?;
    let mut successor = fill_recyclable_para_tombstone(&slot, &expected)?;
    if let Err(error) = authorize_successor() {
        scrub_recyclable_para_generation(&slot, &intended)?;
        abort_recyclable_para_transaction(&transaction, private_root)?;
        return Err(error);
    }
    move_para_directory_to_claim(&slot, directory, &successor.directory)?
        .ok_or("PARA fixed successor changed before publication")?;
    successor.path = directory.to_path_buf();
    attest_para_public_successor(directory, Some(&successor))?;

    if fs::symlink_metadata(&parked).is_ok() {
        let parked_tombstone = attest_recyclable_para_tombstone(&parked)?;
        move_para_directory_to_claim(&parked, &slot, &parked_tombstone)?
            .ok_or("PARA parked tombstone changed during recreation")?;
    } else {
        ensure_recyclable_para_members(&slot)?;
    }
    attest_recyclable_para_tombstone(&slot)?;
    attest_path_is_absent(&parked)?;
    sync_para_directory(people)?;
    sync_para_directory(private_root)?;
    finish_recyclable_para_transaction(&mut transaction, people, private_root)
}

fn rewrite_para_people<F>(
    config: &Config,
    disposition: F,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
{
    rewrite_para_people_with_hook(config, disposition, |_| {})
}

fn rewrite_para_people_with_hook<F, H>(
    config: &Config,
    disposition: F,
    before_mutation: H,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
    H: FnMut(&Path),
{
    rewrite_para_people_with_hooks(config, disposition, before_mutation, |_| {})
}

fn rewrite_para_people_with_hooks<F, H, P>(
    config: &Config,
    mut disposition: F,
    mut before_mutation: H,
    mut after_person_publication: P,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
    H: FnMut(&Path),
    P: FnMut(&Path),
{
    let knowledge = &config.knowledge;
    let people = knowledge.path.join("areas").join("people");
    if !people.exists() {
        return Ok(0);
    }
    set_restrictive_directory_permissions(&knowledge.path)?;
    set_restrictive_directory_permissions(&knowledge.path.join("areas"))?;
    set_restrictive_directory_permissions(&people)?;
    let private_root = para_private_root(config)?;
    prepare_para_private_root(&private_root)?;
    recover_para_transactions(&people, &private_root, Some(config))?;
    let mut removed = 0usize;
    for entry in fs::read_dir(&people)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            let identity = preserve_file_before_retraction(config, &entry.path())?;
            remove_preserved_file(config, &entry.path(), &identity)?;
            removed += 1;
        }
    }
    for entry in WalkDir::new(&people)
        .min_depth(2)
        .max_depth(2)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry.file_name() == "items.json"
                && entry
                    .path()
                    .parent()
                    .is_some_and(|parent| para_public_person_directory(parent, &people))
        })
    {
        let directory = entry.path().parent().ok_or("PARA item has no parent")?;
        set_restrictive_directory_permissions(directory)?;
        let summary_path = entry.path().with_file_name("summary.md");
        let mut snapshot = inspect_para_person(directory)?;
        let raw = match std::str::from_utf8(&snapshot.items.bytes) {
            Ok(raw) => raw,
            Err(_) => {
                preserve_bound_file_before_retraction_in_place(
                    config,
                    entry.path(),
                    &mut snapshot.items,
                )?;
                if let Some(summary_identity) = snapshot.summary.as_mut() {
                    preserve_bound_file_before_retraction_in_place(
                        config,
                        &summary_path,
                        summary_identity,
                    )?;
                }
                before_mutation(entry.path());
                if snapshot.summary.is_some() {
                    before_mutation(&summary_path);
                }
                replace_para_person_generation_with_hook(
                    directory,
                    &private_root,
                    &snapshot,
                    None,
                    None,
                    |path| after_person_publication(path),
                )?;
                removed += 1;
                continue;
            }
        };
        let items: Vec<serde_json::Value> = match serde_json::from_str(raw) {
            Ok(items) => items,
            Err(_) => {
                preserve_bound_file_before_retraction_in_place(
                    config,
                    entry.path(),
                    &mut snapshot.items,
                )?;
                if let Some(summary_identity) = snapshot.summary.as_mut() {
                    preserve_bound_file_before_retraction_in_place(
                        config,
                        &summary_path,
                        summary_identity,
                    )?;
                }
                before_mutation(entry.path());
                if snapshot.summary.is_some() {
                    before_mutation(&summary_path);
                }
                replace_para_person_generation_with_hook(
                    directory,
                    &private_root,
                    &snapshot,
                    None,
                    None,
                    |path| after_person_publication(path),
                )?;
                removed += 1;
                continue;
            }
        };
        validate_para_items(&items)?;
        let fallback = entry
            .path()
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("Person");
        let mut retained = Vec::new();
        retained
            .try_reserve(items.len())
            .map_err(|_| "PARA retained-item allocation failed")?;
        let mut quarantine_items = false;
        for item in &items {
            let Some(source) = item.get("source").and_then(|source| source.as_str()) else {
                quarantine_items = true;
                removed += 1;
                continue;
            };
            let id = record_id_for_json("para", item)?;
            match disposition(source, &id) {
                RecordDisposition::Keep => retained.push(item),
                RecordDisposition::RemoveOwned => removed += 1,
                RecordDisposition::Quarantine => {
                    quarantine_items = true;
                    removed += 1;
                }
            }
        }
        if quarantine_items {
            preserve_bound_file_before_retraction_in_place(
                config,
                entry.path(),
                &mut snapshot.items,
            )?;
        }

        let display_name = match snapshot.summary.as_mut() {
            Some(identity) => {
                let assessment = std::str::from_utf8(&identity.bytes)
                    .ok()
                    .and_then(|summary| {
                        let name = summary
                            .lines()
                            .next()
                            .and_then(|line| line.strip_prefix("# "))
                            .unwrap_or(fallback);
                        if name.len() > 512 {
                            return None;
                        }
                        render_para_summary(name, items.iter())
                            .ok()
                            .map(|rendered| (summary == rendered, name.to_string()))
                    });
                match assessment {
                    Some((true, name)) => name,
                    Some((false, _)) | None => {
                        preserve_bound_file_before_retraction_in_place(
                            config,
                            &summary_path,
                            identity,
                        )?;
                        fallback.to_string()
                    }
                }
            }
            None => fallback.to_string(),
        };

        if retained.is_empty() {
            before_mutation(entry.path());
            if snapshot.summary.is_some() {
                before_mutation(&summary_path);
            }
            replace_para_person_generation_with_hook(
                directory,
                &private_root,
                &snapshot,
                None,
                None,
                |path| after_person_publication(path),
            )?;
            continue;
        }
        let rewritten_items =
            bounded_json_to_vec_pretty(&retained, MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
        let rendered = render_para_summary(&display_name, retained.iter().copied())?.into_bytes();
        before_mutation(entry.path());
        before_mutation(&summary_path);
        replace_para_person_generation_with_authorization(
            directory,
            &private_root,
            &snapshot,
            Some(rewritten_items),
            Some(rendered),
            |_| {},
            |path| after_person_publication(path),
            (|_| {}, |_| {}),
            || revalidate_para_public_items(config, &retained),
        )?;
    }
    for entry in WalkDir::new(&people)
        .min_depth(1)
        .max_depth(1)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_dir() && para_public_person_directory(entry.path(), &people)
        })
    {
        let items = entry.path().join("items.json");
        let summary = entry.path().join("summary.md");
        let valid_items = fs::symlink_metadata(&items)
            .is_ok_and(|metadata| metadata.is_file() && !metadata.file_type().is_symlink());
        if fs::symlink_metadata(&items).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            let identity = preserve_file_before_retraction(config, &items)?;
            remove_preserved_file(config, &items, &identity)?;
            removed += 1;
        }
        if !valid_items && summary.exists() {
            let identity = preserve_file_before_retraction(config, &summary)?;
            remove_preserved_file(config, &summary, &identity)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn rewrite_knowledge_log_at<F>(
    config: &Config,
    log_path: &Path,
    mut disposition: F,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
{
    let knowledge = &config.knowledge;
    let base = log_path.parent().ok_or("knowledge log has no parent")?;
    match fs::symlink_metadata(log_path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let identity = preserve_file_before_retraction(config, log_path)?;
            remove_preserved_file(config, log_path, &identity)?;
            return Ok(1);
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    }
    set_restrictive_directory_permissions(&knowledge.path)?;
    if base != knowledge.path {
        set_restrictive_directory_permissions(base)?;
    }
    let mut identity =
        preserved_source_identity_with_limit(log_path, MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
    let content = match std::str::from_utf8(&identity.bytes) {
        Ok(content) => content,
        Err(_) => {
            identity = preserve_bound_file_before_retraction(config, log_path, identity)?;
            remove_preserved_file(config, log_path, &identity)?;
            return Ok(1);
        }
    };
    let mut removed = 0usize;
    let mut sections = content.split("\n## [");
    let header = sections.next().unwrap_or_default();
    let mut rewritten = BoundedString::new(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES, content.len())?;
    rewritten.push_str("# Knowledge Log\n\n")?;
    let mut quarantine = header.trim() != "# Knowledge Log";
    for section in sections {
        let Some(source) = log_section_source(section) else {
            quarantine = true;
            removed += 1;
            continue;
        };
        match disposition(source, &record_id("log", section.trim_end())) {
            RecordDisposition::Keep => {
                rewritten.push_str("## [")?;
                rewritten.push_str(section.trim_end())?;
                rewritten.push_str("\n\n")?;
            }
            RecordDisposition::RemoveOwned => removed += 1,
            RecordDisposition::Quarantine => {
                quarantine = true;
                removed += 1;
            }
        }
    }
    if quarantine {
        identity = preserve_bound_file_before_retraction(config, log_path, identity)?;
    }
    if removed > 0 || quarantine {
        let rewritten = rewritten.into_string();
        replace_preserved_file(config, log_path, &identity, Some(rewritten.as_bytes()))?;
    }
    Ok(removed)
}

fn remove_empty_knowledge_log(
    config: &Config,
    path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let identity = match fs::symlink_metadata(path) {
        Ok(_) => preserved_source_identity(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if identity.bytes == b"# Knowledge Log\n\n" {
        remove_preserved_file(config, path, &identity)?;
    }
    Ok(())
}

fn rewrite_knowledge_sources<F>(
    config: &Config,
    manifest: &KnowledgeProvenanceManifest,
    mut disposition: F,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
{
    let inactive = |source: &str, id: &str| {
        if manifest
            .records
            .get(source)
            .is_some_and(|records| records.contains(id))
        {
            RecordDisposition::RemoveOwned
        } else {
            RecordDisposition::Quarantine
        }
    };
    if config.knowledge.adapter.eq_ignore_ascii_case("para") {
        let wiki = rewrite_wiki_people(config, inactive)
            .map_err(|error| format!("inactive Wiki retirement failed: {error}"))?;
        let para = rewrite_para_people(config, |source, id| disposition(source, id))
            .map_err(|error| format!("active PARA retirement failed: {error}"))?;
        Ok(wiki + para)
    } else {
        let para = rewrite_para_people(config, inactive)
            .map_err(|error| format!("inactive PARA retirement failed: {error}"))?;
        let wiki = rewrite_wiki_people(config, |source, id| disposition(source, id))
            .map_err(|error| format!("active Wiki retirement failed: {error}"))?;
        Ok(para + wiki)
    }
}

fn rewrite_knowledge_logs<F>(
    config: &Config,
    manifest: &KnowledgeProvenanceManifest,
    mut disposition: F,
) -> Result<usize, Box<dyn std::error::Error>>
where
    F: FnMut(&str, &str) -> RecordDisposition,
{
    let current = knowledge_log_path(&config.knowledge);
    let mut candidates = BTreeSet::from([
        config.knowledge.path.join("log.md"),
        config.knowledge.path.join("memory/log.md"),
        current.clone(),
    ]);
    candidates.extend(
        manifest
            .managed_logs
            .iter()
            .filter_map(|relative| managed_log_path(config, relative)),
    );
    let mut removed = 0usize;
    for candidate in candidates {
        if candidate == current {
            removed +=
                rewrite_knowledge_log_at(config, &candidate, |source, id| disposition(source, id))?;
        } else {
            removed += rewrite_knowledge_log_at(config, &candidate, |source, id| {
                if manifest
                    .records
                    .get(source)
                    .is_some_and(|records| records.contains(id))
                {
                    RecordDisposition::RemoveOwned
                } else {
                    RecordDisposition::Quarantine
                }
            })?;
            remove_empty_knowledge_log(config, &candidate)?;
        }
    }
    Ok(removed)
}

/// Retract facts and log entries derived from one source before refusing it or
/// replacing its current facts. Both the new corpus-relative provenance and
/// the legacy basename provenance are recognized.
pub fn retract_meeting_derivatives(
    meeting_path: &Path,
    config: &Config,
) -> Result<KnowledgeReconcileResult, Box<dyn std::error::Error>> {
    let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK)?;
    retract_meeting_derivatives_locked(meeting_path, config)
}

fn retract_meeting_derivatives_locked(
    meeting_path: &Path,
    config: &Config,
) -> Result<KnowledgeReconcileResult, Box<dyn std::error::Error>> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        return Ok(KnowledgeReconcileResult::default());
    }
    let mut keys = HashSet::new();
    if let Some(exact) = exact_source_key(meeting_path, config) {
        keys.insert(exact);
    }
    // Legacy basename provenance is intrinsically ambiguous. It may only be
    // targeted for a lexically contained active-corpus path; an arbitrary
    // outside path with the same basename must never delete a normal fact.
    // For a contained deleted/restricted duplicate we remove the ambiguous
    // legacy record conservatively, while v2 facts remain source-exact.
    if lexically_contained_active_path(meeting_path, config) {
        if let Some(legacy) = legacy_source_key(meeting_path) {
            keys.insert(legacy);
        }
    }
    let mut path_candidates = keys.clone();
    path_candidates.insert(meeting_path.display().to_string());
    if let Ok(canonical) = meeting_path.canonicalize() {
        path_candidates.insert(canonical.display().to_string());
    }
    let (mut manifest, manifest_valid) = load_provenance_manifest(config);
    if !manifest_valid {
        manifest = KnowledgeProvenanceManifest::default();
    }
    let classify = |source: &str, id: &str, targeted: bool| {
        if !targeted {
            RecordDisposition::Keep
        } else if manifest
            .records
            .get(source)
            .is_some_and(|records| records.contains(id))
        {
            RecordDisposition::RemoveOwned
        } else {
            RecordDisposition::Quarantine
        }
    };
    let facts_removed = rewrite_knowledge_sources(config, &manifest, |source, id| {
        classify(source, id, keys.contains(source))
    })
    .map_err(|error| format!("targeted knowledge source retirement failed: {error}"))?;
    let log_entries_removed = rewrite_knowledge_logs(config, &manifest, |source, id| {
        classify(source, id, path_candidates.contains(source))
    })
    .map_err(|error| format!("targeted knowledge log retirement failed: {error}"))?;
    for key in &keys {
        manifest.sources.remove(key);
        manifest.records.remove(key);
    }
    manifest.managed_logs = managed_log_relative_path(config).into_iter().collect();
    manifest.schema = 4;
    save_provenance_manifest(config, &manifest)
        .map_err(|error| format!("targeted knowledge provenance commit failed: {error}"))?;
    Ok(KnowledgeReconcileResult {
        facts_removed,
        log_entries_removed,
    })
}

/// Remove every Minutes-generated knowledge derivative whose source is no
/// longer a strict, normal meeting in the configured corpus.
pub fn reconcile_knowledge_derivatives(
    config: &Config,
) -> Result<KnowledgeReconcileResult, Box<dyn std::error::Error>> {
    let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK)?;
    reconcile_knowledge_derivatives_locked(config)
}

fn reconcile_knowledge_derivatives_locked(
    config: &Config,
) -> Result<KnowledgeReconcileResult, Box<dyn std::error::Error>> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        return Ok(KnowledgeReconcileResult::default());
    }
    match fs::symlink_metadata(&config.knowledge.path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err("configured knowledge derivative is not a normal directory".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let mut authorized_keys = HashSet::new();
    let mut authorized_paths = HashSet::new();
    let (mut manifest, manifest_valid) = load_provenance_manifest(config);
    if !manifest_valid {
        manifest.sources.clear();
    }
    let mut live_revisions = BTreeMap::new();
    for entry in corpus_markdown_entries(config) {
        if let Ok(meeting) = authorized_meeting(entry.path(), config) {
            if let Some(exact) = exact_source_key(&meeting.path, config) {
                let revision = source_revision(&meeting.content);
                let unchanged = manifest.sources.get(&exact) == Some(&revision);
                live_revisions.insert(exact.clone(), revision);
                if unchanged {
                    authorized_paths.insert(exact.clone());
                    authorized_keys.insert(exact);
                }
            }
        }
    }
    let classify = |source: &str, id: &str, authorized: bool| {
        let owned = manifest
            .records
            .get(source)
            .is_some_and(|records| records.contains(id));
        if authorized && owned {
            RecordDisposition::Keep
        } else if owned {
            RecordDisposition::RemoveOwned
        } else {
            RecordDisposition::Quarantine
        }
    };
    let facts_removed = rewrite_knowledge_sources(config, &manifest, |source, id| {
        classify(source, id, authorized_keys.contains(source))
    })
    .map_err(|error| format!("knowledge source retirement failed: {error}"))?;
    let log_entries_removed = rewrite_knowledge_logs(config, &manifest, |source, id| {
        classify(source, id, authorized_paths.contains(source))
    })
    .map_err(|error| format!("knowledge log retirement failed: {error}"))?;
    manifest.schema = 4;
    manifest
        .sources
        .retain(|source, revision| live_revisions.get(source) == Some(revision));
    manifest
        .records
        .retain(|source, _| manifest.sources.contains_key(source));
    manifest.managed_logs = managed_log_relative_path(config).into_iter().collect();
    save_provenance_manifest(config, &manifest)
        .map_err(|error| format!("knowledge provenance commit failed: {error}"))?;
    Ok(KnowledgeReconcileResult {
        facts_removed,
        log_entries_removed,
    })
}

/// Main entry point: update the knowledge base from a processed meeting.
/// Called from pipeline.rs after vault sync. Non-fatal — errors are logged, never crash.
pub fn update_from_meeting(
    result: &crate::WriteResult,
    _frontmatter: &Frontmatter,
    _transcript: &str,
    config: &Config,
) -> Result<UpdateResult, Box<dyn std::error::Error>> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        return Ok(UpdateResult {
            facts_written: 0,
            facts_skipped: 0,
            people_updated: vec![],
        });
    }
    let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK)?;
    update_path_transaction_locked(&result.path, config, false, &mut |_| {})
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KnowledgeTxnPhase {
    BeforeFinalAuthorization,
    AfterDerivativeRetraction,
    AfterWrite,
    AfterManifestCommit,
}

fn same_authorized_snapshot(left: &AuthorizedMeeting, right: &AuthorizedMeeting) -> bool {
    left.path == right.path && left.content == right.content
}

fn zero_update() -> UpdateResult {
    UpdateResult {
        facts_written: 0,
        facts_skipped: 0,
        people_updated: vec![],
    }
}

fn policy_denied_update(
    path: &Path,
    config: &Config,
    deny_is_error: bool,
    reason: &str,
) -> Result<UpdateResult, Box<dyn std::error::Error>> {
    reconcile_knowledge_derivatives_locked(config)?;
    retract_meeting_derivatives_locked(path, config)?;
    if deny_is_error {
        Err(PolicyDeniedAfterConfirmedRetraction {
            source_scope: privacy_safe_source_scope(path),
            reason: reason.to_string(),
        }
        .into())
    } else {
        tracing::debug!(
            source_scope = %privacy_safe_source_scope(path),
            reason,
            "retracted and skipped policy-ineligible meeting during knowledge update"
        );
        Ok(zero_update())
    }
}

fn update_path_transaction_locked<F>(
    path: &Path,
    config: &Config,
    deny_is_error: bool,
    hook: &mut F,
) -> Result<UpdateResult, Box<dyn std::error::Error>>
where
    F: FnMut(KnowledgeTxnPhase),
{
    for _attempt in 0..3 {
        let initial = match authorized_meeting(path, config) {
            Ok(authorized) => authorized,
            Err(error) => {
                return policy_denied_update(
                    path,
                    config,
                    deny_is_error,
                    policy_reason(&error.to_string()),
                );
            }
        };
        reconcile_knowledge_derivatives_locked(config)?;
        hook(KnowledgeTxnPhase::BeforeFinalAuthorization);
        let final_authorized = match authorized_meeting(path, config) {
            Ok(authorized) => authorized,
            Err(error) => {
                return policy_denied_update(
                    path,
                    config,
                    deny_is_error,
                    policy_reason(&error.to_string()),
                );
            }
        };
        if !same_authorized_snapshot(&initial, &final_authorized) {
            continue;
        }

        // This final descriptor-bound authorization is immediately adjacent
        // to mutation. All Minutes writers share the same transaction lock.
        let commit_authorized = authorized_meeting(path, config)?;
        if !same_authorized_snapshot(&final_authorized, &commit_authorized) {
            continue;
        }
        retract_meeting_derivatives_locked(&commit_authorized.path, config)?;
        hook(KnowledgeTxnPhase::AfterDerivativeRetraction);
        let write = write_authorized_meeting(&commit_authorized, config);
        hook(KnowledgeTxnPhase::AfterWrite);
        let after = authorized_meeting(path, config);
        match (write, after) {
            (Ok(commit), Ok(after)) if same_authorized_snapshot(&commit_authorized, &after) => {
                if record_authorized_source(config, &after, commit.records).is_ok() {
                    hook(KnowledgeTxnPhase::AfterManifestCommit);
                    // Revalidate after the durable provenance commit as well.
                    // If the source changed in the commit window, the newly
                    // manifested derivatives are rolled back before retry.
                    if authorized_meeting(path, config)
                        .is_ok_and(|committed| same_authorized_snapshot(&after, &committed))
                    {
                        return Ok(commit.update);
                    }
                }
                retract_meeting_derivatives_locked(path, config)?;
                reconcile_knowledge_derivatives_locked(config)?;
            }
            _ => {
                // Roll back source-specific outputs, then globally reconcile
                // summaries/logs before retrying or denying the new state.
                retract_meeting_derivatives_locked(path, config)?;
                reconcile_knowledge_derivatives_locked(config)?;
            }
        }
    }
    policy_denied_update(path, config, deny_is_error, "unstable-source")
}

fn write_authorized_meeting(
    authorized: &AuthorizedMeeting,
    config: &Config,
) -> Result<KnowledgeWriteCommit, Box<dyn std::error::Error>> {
    let kc = &config.knowledge;
    let min_confidence = Confidence::parse(&kc.min_confidence);

    // Phase 1: Extract facts from structured frontmatter (zero hallucination risk)
    let mut person_facts = crate::knowledge_extract::extract_from_frontmatter(
        &authorized.frontmatter,
        &authorized.path.display().to_string(),
    );
    let source_key = preferred_source_key(&authorized.path, config)?;
    for person in &mut person_facts {
        for fact in &mut person.facts {
            fact.source_meeting.clone_from(&source_key);
        }
    }

    // Phase 2 (future): Optional LLM extraction from transcript body
    // Only runs when engine != "none". Not implemented yet — structured-first is safer.

    // Phase 3: Write facts through the adapter
    let adapter = make_adapter(kc)?;
    let mut total_written = 0usize;
    let mut total_skipped = 0usize;
    let mut people_updated = Vec::new();
    let mut write_errors: Vec<String> = Vec::new();

    for pf in &person_facts {
        match adapter.update_person_authorized(
            &pf.slug,
            &pf.name,
            &pf.facts,
            min_confidence,
            config,
            authorized,
        ) {
            Ok((written, skipped)) => {
                if written > 0 {
                    people_updated.push(pf.name.clone());
                }
                total_written += written;
                total_skipped += skipped;
            }
            Err(e) => {
                tracing::debug!(reason = %policy_reason(&e.to_string()), "knowledge adapter write failed");
                write_errors.push("adapter write failed".to_string());
            }
        }
    }

    // Always write the log entry, even on partial failure
    let log_entry = LogEntry {
        date: authorized.frontmatter.date,
        meeting_title: authorized.frontmatter.title.clone(),
        meeting_path: source_key,
        people_updated: people_updated.clone(),
        fact_count: total_written,
        skipped_count: total_skipped,
    };
    adapter.append_log(&log_entry, kc)?;

    if !write_errors.is_empty() {
        return Err(format!(
            "partial knowledge update ({} written, {} failed): {}",
            total_written,
            write_errors.len(),
            write_errors.join("; ")
        )
        .into());
    }

    let records = generated_record_ids(&person_facts, min_confidence, &kc.adapter, &log_entry)?;
    Ok(KnowledgeWriteCommit {
        update: UpdateResult {
            facts_written: total_written,
            facts_skipped: total_skipped,
            people_updated,
        },
        records,
    })
}

/// Process a single existing meeting file through knowledge extraction (for `minutes ingest`).
pub fn ingest_file(
    meeting_path: &Path,
    config: &Config,
) -> Result<UpdateResult, Box<dyn std::error::Error>> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        return Ok(zero_update());
    }
    let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK)?;
    update_path_transaction_locked(meeting_path, config, true, &mut |_| {})
}

/// Descriptor-bound, strict preview for `minutes ingest --dry-run`.
pub fn preview_ingest_file(
    meeting_path: &Path,
    config: &Config,
) -> Result<Vec<PersonFacts>, Box<dyn std::error::Error>> {
    let authorized = authorized_meeting(meeting_path, config)?;
    Ok(crate::knowledge_extract::extract_from_frontmatter(
        &authorized.frontmatter,
        &authorized.path.display().to_string(),
    ))
}

/// Product-level source mutation/deletion hook. Call this immediately after a
/// meeting edit or delete and before any derived-store query is enabled. A
/// normal edit replaces source-specific knowledge; a restricted/malformed or
/// deleted path retracts it. Persistent QMD state is confirmed disabled and
/// its Minutes-owned plaintext mirror is removed.
pub fn refresh_after_source_change(
    meeting_path: &Path,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    refresh_after_source_change_with(meeting_path, config, || {
        ensure_qmd_persistence_disabled(config)
    })
}

/// Revalidate every Minutes-managed persistent derivative immediately before
/// a consumer reads it. Knowledge is reconciled from the current live corpus;
/// any formerly configured QMD registration and plaintext mirror are confirmed
/// retracted. QMD cleanup still runs when knowledge reconciliation fails so one
/// store cannot prevent the other from failing closed.
pub fn revalidate_persistent_derivatives_before_read(
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    knowledge_status_snapshot_with(config, || ensure_qmd_persistence_disabled(config)).map(|_| ())
}

#[cfg(test)]
fn revalidate_persistent_derivatives_before_read_with<F>(
    config: &Config,
    qmd_refresh: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), String>,
{
    knowledge_status_snapshot_with_actions(config, qmd_refresh, || Ok(())).map(|_| ())
}

fn count_knowledge_snapshot_locked(
    config: &Config,
) -> Result<(usize, usize), Box<dyn std::error::Error>> {
    let para = config.knowledge.adapter.eq_ignore_ascii_case("para");
    let people = if para {
        config.knowledge.path.join("areas").join("people")
    } else {
        config.knowledge.path.join("people")
    };
    if para {
        let private_root = para_private_root(config)
            .map_err(|error| format!("PARA private-root binding failed: {error}"))?;
        prepare_para_private_root(&private_root)
            .map_err(|error| format!("PARA private-root attestation failed: {error}"))?;
        recover_para_transactions(&people, &private_root, Some(config))
            .map_err(|error| format!("PARA private transaction recovery failed: {error}"))?;
    }
    let people_count = match fs::symlink_metadata(&people) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            return Err("knowledge people store is not a normal directory".into());
        }
        Ok(_) if para => {
            let mut scanned = 0usize;
            let mut public = 0usize;
            for entry in fs::read_dir(&people)? {
                scanned = scanned
                    .checked_add(1)
                    .ok_or("PARA people entry count overflowed")?;
                if scanned > MAX_PARA_RECONCILIATION_ITEMS {
                    return Err("PARA people directory exceeded its bounded entry scan".into());
                }
                let entry = entry?;
                if entry.file_type().is_ok_and(|kind| kind.is_dir())
                    && para_public_person_directory(&entry.path(), &people)
                    && fs::symlink_metadata(entry.path().join("items.json")).is_ok_and(|metadata| {
                        metadata.is_file() && !metadata.file_type().is_symlink()
                    })
                {
                    public = public
                        .checked_add(1)
                        .ok_or("PARA public person count overflowed")?;
                }
            }
            public
        }
        Ok(_) => fs::read_dir(&people)?
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_file())
                    && entry
                        .path()
                        .extension()
                        .is_some_and(|extension| extension == "md")
            })
            .count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };

    let log_base = if para {
        config.knowledge.path.join("memory")
    } else {
        config.knowledge.path.clone()
    };
    let log_path = log_base.join(&config.knowledge.log_file);
    let log_entries = match fs::symlink_metadata(&log_path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err("knowledge log is not a normal file".into());
        }
        Ok(_) => fs::read_to_string(log_path)?.matches("\n## [").count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error.into()),
    };
    Ok((people_count, log_entries))
}

/// Reconcile persistent derivatives and return a privacy-safe snapshot without
/// reopening either config or the knowledge tree in a less-trusted consumer.
/// Knowledge reconciliation and counts share one exclusive lock. A configured
/// QMD target is always confirmed disabled even when knowledge handling fails.
pub fn knowledge_status_snapshot(
    config: &Config,
) -> Result<KnowledgeStatusSnapshot, Box<dyn std::error::Error>> {
    knowledge_status_snapshot_with(config, || ensure_qmd_persistence_disabled(config))
}

fn knowledge_status_snapshot_with<F>(
    config: &Config,
    qmd_refresh: F,
) -> Result<KnowledgeStatusSnapshot, Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), String>,
{
    knowledge_status_snapshot_with_actions(config, qmd_refresh, || disable_unconfigured_qmd(config))
}

fn knowledge_status_snapshot_with_actions<F, D>(
    config: &Config,
    qmd_refresh: F,
    qmd_disable: D,
) -> Result<KnowledgeStatusSnapshot, Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), String>,
    D: FnOnce() -> Result<(), String>,
{
    let configured = !config.knowledge.path.as_os_str().is_empty();
    let enabled = config.knowledge.enabled;
    let knowledge_result = if enabled && configured {
        match acquire_policy_lock(KNOWLEDGE_POLICY_LOCK) {
            Ok(_lock) => reconcile_knowledge_derivatives_locked(config)
                .and_then(|_| count_knowledge_snapshot_locked(config)),
            Err(error) => Err(error),
        }
    } else {
        Ok((0, 0))
    };
    let qmd_result = if config.search.qmd_collection.is_some() {
        qmd_refresh()
    } else {
        qmd_disable()
    };

    match (knowledge_result, qmd_result) {
        (Ok((people_count, log_entries)), Ok(())) => Ok(KnowledgeStatusSnapshot {
            enabled,
            configured,
            adapter: (enabled && configured).then(|| config.knowledge.adapter.clone()),
            engine: (enabled && configured).then(|| config.knowledge.engine.clone()),
            people_count,
            log_entries,
        }),
        (knowledge, qmd) => {
            if let Err(error) = knowledge {
                tracing::warn!(
                    reason = %policy_reason(&error.to_string()),
                    "pre-read knowledge derivative revalidation failed"
                );
            }
            let message = if qmd.is_err() {
                "persistent derivatives could not be safely revalidated or confirmed disabled before read"
            } else {
                "knowledge derivatives could not be safely reconciled before read"
            };
            Err(std::io::Error::other(message).into())
        }
    }
}

fn refresh_after_source_change_with<F>(
    meeting_path: &Path,
    config: &Config,
    qmd_refresh: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnOnce() -> Result<(), String>,
{
    let knowledge_result = if config.knowledge.enabled
        && !config.knowledge.path.as_os_str().is_empty()
    {
        match acquire_policy_lock(KNOWLEDGE_POLICY_LOCK) {
            Ok(_lock) => {
                update_path_transaction_locked(meeting_path, config, false, &mut |_| {}).map(|_| ())
            }
            Err(error) => Err(error),
        }
    } else {
        Ok(())
    };
    let qmd_result = if config.search.qmd_collection.is_some() {
        qmd_refresh()
    } else {
        Ok(())
    };

    match (knowledge_result, qmd_result) {
        (Ok(()), Ok(())) => Ok(()),
        (knowledge, qmd) => {
            if let Err(error) = knowledge {
                tracing::warn!(
                    source_scope = %privacy_safe_source_scope(meeting_path),
                    reason = %policy_reason(&error.to_string()),
                    "knowledge source-change reconciliation failed"
                );
            }
            let message = if qmd.is_err() {
                "source changed, but one or more derived query stores could not be safely refreshed or confirmed disabled"
            } else {
                "source changed, but knowledge derivatives could not be safely reconciled"
            };
            Err(std::io::Error::other(message).into())
        }
    }
}

// ── Adapter dispatch ────────────────────────────────────────────

fn make_adapter(
    kc: &KnowledgeConfig,
) -> Result<Box<dyn KnowledgeAdapter>, Box<dyn std::error::Error>> {
    match kc.adapter.to_lowercase().as_str() {
        "wiki" => Ok(Box::new(WikiAdapter)),
        "para" => Ok(Box::new(ParaAdapter)),
        "obsidian" => Ok(Box::new(ObsidianAdapter)),
        other => Err(format!(
            "unknown knowledge adapter '{}' — valid options: wiki, para, obsidian",
            other
        )
        .into()),
    }
}

// ── Adapter trait ───────────────────────────────────────────────

trait KnowledgeAdapter {
    /// Write facts about a person. Returns (written_count, skipped_count).
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>>;

    fn update_person_authorized(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &Config,
        _authorized: &AuthorizedMeeting,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        self.update_person(slug, name, facts, min_confidence, &config.knowledge)
    }

    /// Append an entry to the chronological log.
    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

// ── Wiki Adapter (Karpathy flat markdown) ───────────────────────

struct WikiAdapter;

impl KnowledgeAdapter for WikiAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let dir = config.path.join("people");
        create_private_dir_all(&config.path)?;
        create_private_dir_all(&dir)?;
        let file_path = dir.join(format!("{}.md", slug));

        let qualifying: Vec<&Fact> = facts
            .iter()
            .filter(|f| f.confidence.meets(min_confidence))
            .collect();
        let skipped = facts.len() - qualifying.len();
        if qualifying.is_empty() {
            return Ok((0, skipped));
        }

        let mut content = if file_path.exists() {
            fs::read_to_string(&file_path)?
        } else {
            format!("# {}\n\n", single_line(name))
        };

        // Deduplicate only within the same source. Collapsing identical text
        // across meetings loses provenance and makes later source retraction
        // remove a fact that is still supported by another normal meeting.
        let new_facts: Vec<&&Fact> = qualifying
            .iter()
            .filter(|fact| {
                let text = single_line(&fact.text);
                !content.lines().any(|line| {
                    generated_wiki_source(line) == Some(fact.source_meeting.as_str())
                        && line.contains(&text)
                })
            })
            .collect();
        if new_facts.is_empty() {
            return Ok((0, skipped));
        }

        // Group by category for structured sections
        let mut by_category: HashMap<&str, Vec<&&Fact>> = HashMap::new();
        for fact in &new_facts {
            by_category
                .entry(fact.category.as_str())
                .or_default()
                .push(fact);
        }

        for (category, cat_facts) in &by_category {
            let section_header = format!("## {}", capitalize(category));
            if !content.contains(&section_header) {
                if !content.ends_with("\n\n") {
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push('\n');
                }
                content.push_str(&section_header);
                content.push('\n');
                content.push('\n');
            }

            // Insert facts before the next section or at end
            let insert_pos = find_section_end(&content, &section_header);
            let mut block = String::new();
            for fact in cat_facts {
                block.push_str(&wiki_fact_record(fact));
                block.push('\n');
            }
            content.insert_str(insert_pos, &block);
        }

        fs::write(&file_path, &content)?;
        set_restrictive_permissions(&file_path)?;
        Ok((new_facts.len(), skipped))
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let log_path = config.path.join(&config.log_file);
        create_private_dir_all(&config.path)?;

        // Create with header if new, then true append (no full rewrite)
        let is_new = !log_path.exists();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        set_restrictive_permissions(&log_path)?;

        if is_new {
            write!(file, "# Knowledge Log\n\n")?;
        }

        write!(file, "{}", render_log_section(entry))?;

        Ok(())
    }
}

// ── PARA Adapter ────────────────────────────────────────────────

struct ParaAdapter;

impl KnowledgeAdapter for ParaAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let areas = config.path.join("areas");
        let people = areas.join("people");
        let dir = people.join(slug);
        create_private_dir_all(&config.path)?;
        create_private_dir_all(&areas)?;
        create_private_dir_all(&people)?;
        let private_root = unscoped_para_private_root(&config.path)?;
        prepare_para_private_root(&private_root)?;
        recover_para_transactions(&people, &private_root, None)?;
        let qualifying: Vec<&Fact> = facts
            .iter()
            .filter(|f| f.confidence.meets(min_confidence))
            .collect();
        let skipped = facts.len() - qualifying.len();
        if qualifying.is_empty() {
            return Ok((0, skipped));
        }

        // Load existing items.json or create empty array.
        // If the file exists but is malformed, back it up and fail rather than
        // silently discarding all accumulated facts.
        let snapshot = match fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("PARA person path is not a normal directory".into());
            }
            Ok(_) => Some(inspect_para_person(&dir)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let mut items: Vec<serde_json::Value> = if let Some(snapshot) = snapshot.as_ref() {
            let raw = std::str::from_utf8(&snapshot.items.bytes)
                .map_err(|_| "PARA items.json is not UTF-8")?;
            match serde_json::from_str(raw) {
                Ok(parsed) => parsed,
                Err(e) => {
                    return Err(format!(
                        "items.json for {slug} is malformed; the exact generation was retained ({e})"
                    )
                    .into());
                }
            }
        } else {
            vec![]
        };
        validate_para_items(&items)?;

        items
            .try_reserve(qualifying.len())
            .map_err(|_| "PARA item allocation failed")?;

        let mut written = 0usize;
        for fact in &qualifying {
            if items.iter().any(|item| {
                item.get("fact").and_then(|value| value.as_str()) == Some(fact.text.as_str())
                    && item.get("source").and_then(|value| value.as_str())
                        == Some(fact.source_meeting.as_str())
            }) {
                continue;
            }
            items.push(para_fact_record(slug, fact));
            written += 1;
        }

        if written > 0 {
            validate_para_items(&items)?;
            let successor_items =
                bounded_json_to_vec_pretty(&items, MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
            let successor_summary = render_para_summary(name, items.iter())?.into_bytes();
            if let Some(snapshot) = snapshot.as_ref() {
                replace_para_person_generation_with_hook(
                    &dir,
                    &private_root,
                    snapshot,
                    Some(successor_items),
                    Some(successor_summary),
                    |_| {},
                )?;
            } else {
                publish_new_para_person_generation(
                    &dir,
                    &private_root,
                    successor_items,
                    successor_summary,
                )?;
            }
        }

        Ok((written, skipped))
    }

    fn update_person_authorized(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &Config,
        authorized: &AuthorizedMeeting,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let knowledge = &config.knowledge;
        let areas = knowledge.path.join("areas");
        let people = areas.join("people");
        let dir = people.join(slug);
        create_private_dir_all(&knowledge.path)?;
        create_private_dir_all(&areas)?;
        create_private_dir_all(&people)?;
        let private_root = para_private_root(config)?;
        prepare_para_private_root(&private_root)?;
        recover_para_transactions(&people, &private_root, Some(config))?;
        let qualifying: Vec<&Fact> = facts
            .iter()
            .filter(|fact| fact.confidence.meets(min_confidence))
            .collect();
        let skipped = facts.len() - qualifying.len();
        if qualifying.is_empty() {
            return Ok((0, skipped));
        }
        let snapshot = match fs::symlink_metadata(&dir) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                return Err("PARA person path is not a normal directory".into());
            }
            Ok(_) => Some(inspect_para_person(&dir)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let mut items: Vec<serde_json::Value> = if let Some(snapshot) = snapshot.as_ref() {
            serde_json::from_slice(&snapshot.items.bytes)
                .map_err(|_| "PARA items.json is malformed; the exact generation was retained")?
        } else {
            Vec::new()
        };
        validate_para_items(&items)?;
        if let Some(snapshot) = snapshot.as_ref() {
            let existing_refs = items.iter().collect::<Vec<_>>();
            if let Err(error) = revalidate_para_public_items(config, &existing_refs) {
                let retained = move_para_directory_to_unique_claim(
                    &dir,
                    &private_root,
                    PARA_PERSON_FAILED_PREFIX,
                    &snapshot.directory,
                )?
                .ok_or("unproven PARA person changed before private quarantine")?;
                sync_para_directory(&people)?;
                sync_para_directory(&private_root)?;
                return Err(format!(
                    "unproven PARA person was removed from the public derivative and retained at {}: {error}",
                    retained.display()
                )
                .into());
            }
        }
        let pending_records = qualifying
            .iter()
            .map(|fact| record_id_for_json("para", &para_fact_record(slug, fact)))
            .collect::<Result<BTreeSet<_>, _>>()?;
        items
            .try_reserve(qualifying.len())
            .map_err(|_| "PARA item allocation failed")?;
        let mut written = 0usize;
        for fact in &qualifying {
            if items.iter().any(|item| {
                item.get("fact").and_then(|value| value.as_str()) == Some(fact.text.as_str())
                    && item.get("source").and_then(|value| value.as_str())
                        == Some(fact.source_meeting.as_str())
            }) {
                continue;
            }
            items.push(para_fact_record(slug, fact));
            written += 1;
        }
        if written == 0 {
            return Ok((0, skipped));
        }
        validate_para_items(&items)?;
        let item_refs = items.iter().collect::<Vec<_>>();
        let successor_items =
            bounded_json_to_vec_pretty(&items, MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES)?;
        let successor_summary = render_para_summary(name, items.iter())?.into_bytes();
        let authorize = || {
            revalidate_para_public_items_with_pending(
                config,
                &item_refs,
                Some((authorized, &pending_records)),
            )
        };
        if let Some(snapshot) = snapshot.as_ref() {
            replace_para_person_generation_with_authorization(
                &dir,
                &private_root,
                snapshot,
                Some(successor_items),
                Some(successor_summary),
                |_| {},
                |_| {},
                (|_| {}, |_| {}),
                authorize,
            )?;
        } else {
            publish_new_para_person_generation_with_authorization(
                &dir,
                &private_root,
                successor_items,
                successor_summary,
                authorize,
            )?;
        }
        Ok((written, skipped))
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // PARA stores log in memory/ directory
        let log_path = config.path.join("memory").join(&config.log_file);
        if let Some(parent) = log_path.parent() {
            create_private_dir_all(&config.path)?;
            create_private_dir_all(parent)?;
        }
        // Reuse wiki log format
        WikiAdapter.append_log(
            entry,
            &KnowledgeConfig {
                path: log_path.parent().unwrap_or(&config.path).to_path_buf(),
                log_file: log_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                ..config.clone()
            },
        )
    }
}

// ── Obsidian Adapter (wiki + [[wikilinks]]) ─────────────────────

struct ObsidianAdapter;

impl KnowledgeAdapter for ObsidianAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        // Same as wiki adapter but adds [[wikilinks]] to cross-references
        WikiAdapter.update_person(slug, name, facts, min_confidence, config)
        // TODO: post-process to add [[name]] links for any person slugs mentioned in fact text
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        WikiAdapter.append_log(entry, config)
    }
}

// ── Helpers ─────────────────────────────────────────────────────

/// Windows does not have a Unix creation mode. Preservation objects therefore
/// receive a protected, current-user-only DACL in the same native create call
/// that makes them visible. There is no create-then-`icacls` interval in which
/// private bytes can inherit a broader parent ACL.
#[cfg(windows)]
mod windows_private {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
        GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        GetSecurityInfo, SetEntriesInAclW, SetSecurityInfo, EXPLICIT_ACCESS_W, SET_ACCESS,
        SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        GetTokenInformation, InitializeSecurityDescriptor, IsValidSid,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
        TokenUser, ACL, ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        NO_INHERITANCE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FileAttributeTagInfo, GetFileInformationByHandle,
        GetFileInformationByHandleEx, BY_HANDLE_FILE_INFORMATION, CREATE_NEW, FILE_ADD_FILE,
        FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE, WRITE_DAC,
        WRITE_OWNER,
    };
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    struct OwnerOnlySecurity {
        // TOKEN_USER contains an in-buffer SID pointer. Word storage guarantees
        // the alignment required before casting the buffer to TOKEN_USER.
        token_user: Vec<usize>,
        acl: *mut ACL,
        descriptor: Box<SECURITY_DESCRIPTOR>,
        directory: bool,
    }

    impl Drop for OwnerOnlySecurity {
        fn drop(&mut self) {
            if !self.acl.is_null() {
                unsafe {
                    LocalFree(self.acl.cast());
                }
            }
        }
    }

    impl OwnerOnlySecurity {
        fn new(directory: bool) -> io::Result<Self> {
            let mut token: HANDLE = null_mut();
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = TokenHandle(token);

            let mut required = 0u32;
            let first =
                unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required) };
            if first != 0 || required == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            {
                return Err(io::Error::last_os_error());
            }
            let words = (required as usize).div_ceil(size_of::<usize>());
            let mut token_user = vec![0usize; words];
            if unsafe {
                GetTokenInformation(
                    token.0,
                    TokenUser,
                    token_user.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
            if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
                return Err(io::Error::other(
                    "current Windows token has no valid user SID",
                ));
            }

            let inheritance = if directory {
                OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
            } else {
                NO_INHERITANCE
            };
            let access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: inheritance,
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER,
                    ptstrName: sid.cast(),
                },
            };
            let mut acl: *mut ACL = null_mut();
            let acl_status = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
            if acl_status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(acl_status as i32));
            }

            let mut descriptor = Box::new(unsafe { zeroed::<SECURITY_DESCRIPTOR>() });
            let descriptor_ptr = descriptor.as_mut() as *mut SECURITY_DESCRIPTOR as *mut c_void;
            let initialized = unsafe {
                InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) != 0
                    && SetSecurityDescriptorOwner(descriptor_ptr, sid, 0) != 0
                    && SetSecurityDescriptorDacl(descriptor_ptr, 1, acl, 0) != 0
                    && SetSecurityDescriptorControl(
                        descriptor_ptr,
                        SE_DACL_PROTECTED,
                        SE_DACL_PROTECTED,
                    ) != 0
            };
            if !initialized {
                unsafe {
                    LocalFree(acl.cast());
                }
                return Err(io::Error::last_os_error());
            }

            Ok(Self {
                token_user,
                acl,
                descriptor,
                directory,
            })
        }

        fn sid(&self) -> PSID {
            unsafe { (*(self.token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid }
        }

        fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor.as_mut() as *mut SECURITY_DESCRIPTOR
                    as *mut c_void,
                bInheritHandle: 0,
            }
        }

        fn tighten_and_verify(&self, file: &File) -> io::Result<()> {
            let handle = file.as_raw_handle() as HANDLE;
            let status = unsafe {
                SetSecurityInfo(
                    handle,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | PROTECTED_DACL_SECURITY_INFORMATION,
                    self.sid(),
                    null_mut(),
                    self.acl,
                    null(),
                )
            };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            self.verify(file)
        }

        fn verify(&self, file: &File) -> io::Result<()> {
            let mut owner: PSID = null_mut();
            let mut dacl: *mut ACL = null_mut();
            let mut descriptor = null_mut();
            let status = unsafe {
                GetSecurityInfo(
                    file.as_raw_handle() as HANDLE,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            struct SecurityDescriptorGuard(*mut c_void);
            impl Drop for SecurityDescriptorGuard {
                fn drop(&mut self) {
                    if !self.0.is_null() {
                        unsafe {
                            LocalFree(self.0);
                        }
                    }
                }
            }
            let _descriptor = SecurityDescriptorGuard(descriptor);
            if owner.is_null() || unsafe { EqualSid(owner, self.sid()) } == 0 || dacl.is_null() {
                return Err(io::Error::other(
                    "private Windows object is not owned by the current user",
                ));
            }

            let mut control = 0u16;
            let mut revision = 0u32;
            if unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
                || control & SE_DACL_PROTECTED == 0
            {
                return Err(io::Error::other(
                    "private Windows object does not have a protected DACL",
                ));
            }

            let mut acl_info = unsafe { zeroed::<ACL_SIZE_INFORMATION>() };
            if unsafe {
                GetAclInformation(
                    dacl,
                    (&mut acl_info as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            } == 0
                || acl_info.AceCount != 1
            {
                return Err(io::Error::other(
                    "private Windows object DACL is not owner-only",
                ));
            }

            let mut ace_ptr: *mut c_void = null_mut();
            if unsafe { GetAce(dacl, 0, &mut ace_ptr) } == 0 || ace_ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ace =
                unsafe { &*(ace_ptr.cast::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>()) };
            let required_flags = if self.directory {
                (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8
            } else {
                NO_INHERITANCE as u8
            };
            let ace_sid = (&ace.SidStart as *const u32).cast_mut().cast();
            if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
                || ace.Header.AceFlags != required_flags
                || ace.Mask != FILE_ALL_ACCESS
                || unsafe { EqualSid(ace_sid, self.sid()) } == 0
            {
                return Err(io::Error::other(
                    "private Windows object grants access beyond its owner",
                ));
            }
            Ok(())
        }
    }

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Windows path contains NUL",
            ));
        }
        path.push(0);
        Ok(path)
    }

    fn validate_attributes(file: &File, expect_directory: bool) -> io::Result<()> {
        let mut attributes = unsafe { zeroed::<FILE_ATTRIBUTE_TAG_INFO>() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as HANDLE,
                FileAttributeTagInfo,
                (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let is_directory = attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0;
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || is_directory != expect_directory
        {
            return Err(io::Error::other(
                "private Windows path is a reparse point or has the wrong type",
            ));
        }
        Ok(())
    }

    fn open_existing(
        path: &Path,
        access: u32,
        directory: bool,
        allow_delete_share: bool,
    ) -> io::Result<File> {
        let path = wide(path)?;
        let flags = FILE_FLAG_OPEN_REPARSE_POINT
            | if directory {
                FILE_FLAG_BACKUP_SEMANTICS
            } else {
                FILE_ATTRIBUTE_NORMAL
            };
        let share = FILE_SHARE_READ
            | FILE_SHARE_WRITE
            | if allow_delete_share {
                FILE_SHARE_DELETE
            } else {
                0
            };
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                share,
                null(),
                OPEN_EXISTING,
                flags,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle as _) };
        validate_attributes(&file, directory)?;
        Ok(file)
    }

    pub(super) fn inspect_directory(path: &Path) -> io::Result<File> {
        open_existing(path, 0, true, true)
    }

    pub(super) fn create_directory(path: &Path) -> io::Result<()> {
        let path = wide(path)?;
        let mut security = OwnerOnlySecurity::new(true)?;
        let attributes = security.attributes();
        if unsafe { CreateDirectoryW(path.as_ptr(), &attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn open_private_directory(path: &Path) -> io::Result<File> {
        // The returned descriptor is retained across child creation/writes.
        // Omitting FILE_SHARE_DELETE makes Windows reject a rename/delete of
        // this exact directory until the descriptor is dropped.
        let file = open_existing(
            path,
            FILE_ADD_FILE
                | FILE_LIST_DIRECTORY
                | FILE_TRAVERSE
                | READ_CONTROL
                | WRITE_DAC
                | WRITE_OWNER,
            true,
            false,
        )?;
        OwnerOnlySecurity::new(true)?.tighten_and_verify(&file)?;
        Ok(file)
    }

    pub(super) fn create_private_file_at(
        directory: &File,
        directory_path: &Path,
        name: &std::ffi::OsStr,
    ) -> io::Result<File> {
        let name_wide: Vec<u16> = name.encode_wide().collect();
        if name_wide.is_empty()
            || name_wide == [b'.' as u16]
            || name_wide == [b'.' as u16, b'.' as u16]
            || name_wide
                .iter()
                .any(|unit| matches!(*unit, 0 | 47 | 58 | 92))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "private Windows filename is not one safe relative component",
            ));
        }
        if !same_directory_identity(directory, directory_path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private Windows directory changed before file creation",
            ));
        }

        // The retained directory handle denies delete sharing, so its visible
        // path cannot be renamed or replaced during this CreateFileW call.
        // SECURITY_ATTRIBUTES applies the protected owner-only DACL atomically
        // at CREATE_NEW time; no private bytes are written before verification.
        let path = wide(&directory_path.join(name))?;
        let mut security = OwnerOnlySecurity::new(false)?;
        let attributes = security.attributes();
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER | SYNCHRONIZE,
                0,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle as _) };
        validate_attributes(&file, false)?;
        security.tighten_and_verify(&file)?;
        if !same_directory_identity(directory, directory_path) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private Windows directory changed during file creation",
            ));
        }
        Ok(file)
    }

    pub(super) fn same_directory_identity(file: &File, path: &Path) -> bool {
        let Ok(path_file) = inspect_directory(path) else {
            return false;
        };
        let mut left = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        let mut right = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut left) } == 0
            || unsafe {
                GetFileInformationByHandle(path_file.as_raw_handle() as HANDLE, &mut right)
            } == 0
        {
            return false;
        }
        left.dwVolumeSerialNumber == right.dwVolumeSerialNumber
            && left.nFileIndexHigh == right.nFileIndexHigh
            && left.nFileIndexLow == right.nFileIndexLow
    }

    #[cfg(test)]
    pub(super) fn verify_private_file(file: &File) -> io::Result<()> {
        OwnerOnlySecurity::new(false)?.verify(file)
    }

    #[cfg(test)]
    pub(super) fn verify_private_directory(file: &File) -> io::Result<()> {
        OwnerOnlySecurity::new(true)?.verify(file)
    }
}

pub(crate) fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Set 0600 permissions on sensitive knowledge files (person profiles, logs).
/// Matches the rest of Minutes' security posture for meeting data.
#[cfg(unix)]
fn set_restrictive_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        return create_private_dir_all_no_follow(path);
    }
    #[cfg(not(windows))]
    fs::create_dir_all(path)?;
    #[cfg(not(windows))]
    {
        set_restrictive_directory_permissions(path)
    }
}

fn create_private_dir_all_no_follow(path: &Path) -> std::io::Result<()> {
    open_private_dir_no_follow(path).map(|_| ())
}

#[cfg(unix)]
fn open_private_dir_no_follow(path: &Path) -> std::io::Result<File> {
    open_private_dir_no_follow_with(path, |_, _| {})
}

#[cfg(unix)]
fn private_directory_platform_path(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        for (short, private) in [
            (Path::new("/var"), Path::new("/private/var")),
            (Path::new("/tmp"), Path::new("/private/tmp")),
            (Path::new("/etc"), Path::new("/private/etc")),
        ] {
            if let Ok(relative) = path.strip_prefix(short) {
                return private.join(relative);
            }
        }
    }
    path.to_path_buf()
}

#[cfg(unix)]
fn open_private_dir_no_follow_with<F>(path: &Path, mut after_component: F) -> std::io::Result<File>
where
    F: FnMut(&Path, &File),
{
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let path = private_directory_platform_path(path);
    let start = if path.is_absolute() { "/" } else { "." };
    let start = CString::new(start).expect("static path has no NUL");
    let descriptor = unsafe {
        libc::open(
            start.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut directory = unsafe { File::from_raw_fd(descriptor) };
    let mut traversed = if path.is_absolute() {
        PathBuf::from("/")
    } else {
        PathBuf::new()
    };
    for component in path.components() {
        let Component::Normal(name) = component else {
            if matches!(component, Component::RootDir | Component::CurDir) {
                continue;
            }
            return Err(std::io::Error::other(
                "private directory contains an unsafe component",
            ));
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| std::io::Error::other("private directory contains NUL"))?;
        let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
        let mut child = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        if child < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
            let created = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if created < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error);
                }
            }
            child = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        }
        if child < 0 {
            return Err(std::io::Error::last_os_error());
        }
        directory = unsafe { File::from_raw_fd(child) };
        traversed.push(component.as_os_str());
        after_component(&traversed, &directory);
    }
    let result = unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) };
    if result < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(directory)
}

#[cfg(windows)]
fn open_private_dir_no_follow(path: &Path) -> std::io::Result<File> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut current = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => current.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                return Err(std::io::Error::other(
                    "private directory contains an unsafe parent component",
                ));
            }
            Component::Normal(name) => {
                current.push(name);
                match windows_private::inspect_directory(&current) {
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        windows_private::create_directory(&current)?;
                        windows_private::inspect_directory(&current)?;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }
    windows_private::open_private_directory(&path)
}

#[cfg(all(not(unix), not(windows)))]
fn open_private_dir_no_follow(_path: &Path) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private directory creation is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn create_new_private_file_at(
    directory: &File,
    _directory_path: &Path,
    name: &std::ffi::OsStr,
) -> std::io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let name = CString::new(name.as_bytes())
        .map_err(|_| std::io::Error::other("private filename contains NUL"))?;
    let descriptor = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            0o600,
        )
    };
    if descriptor < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

#[cfg(unix)]
fn open_private_child_dir_at(
    parent: &File,
    _parent_path: &Path,
    name: &str,
) -> std::io::Result<File> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd};
    let name = CString::new(name)
        .map_err(|_| std::io::Error::other("private directory name contains NUL"))?;
    let flags = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW;
    let mut descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if descriptor < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
        let created = unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), 0o700) };
        if created < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::AlreadyExists {
                return Err(error);
            }
        }
        descriptor = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    }
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let directory = unsafe { File::from_raw_fd(descriptor) };
    if unsafe { libc::fchmod(directory.as_raw_fd(), 0o700) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(directory)
}

#[cfg(windows)]
fn create_new_private_file_at(
    directory: &File,
    directory_path: &Path,
    name: &std::ffi::OsStr,
) -> std::io::Result<File> {
    windows_private::create_private_file_at(directory, directory_path, name)
}

#[cfg(windows)]
fn open_private_child_dir_at(
    _parent: &File,
    parent_path: &Path,
    name: &str,
) -> std::io::Result<File> {
    let path = parent_path.join(name);
    create_private_dir_all_no_follow(&path)?;
    windows_private::open_private_directory(&path)
}

#[cfg(all(not(unix), not(windows)))]
fn create_new_private_file_at(
    _directory: &File,
    _directory_path: &Path,
    _name: &std::ffi::OsStr,
) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private file creation is unsupported on this platform",
    ))
}

#[cfg(all(not(unix), not(windows)))]
fn open_private_child_dir_at(
    _parent: &File,
    _parent_path: &Path,
    _name: &str,
) -> std::io::Result<File> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "private directory creation is unsupported on this platform",
    ))
}

#[cfg(unix)]
fn set_restrictive_directory_permissions(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_restrictive_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(unix))]
fn set_restrictive_directory_permissions(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_restrictive_permissions_file(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_restrictive_permissions_file(_file: &File) -> std::io::Result<()> {
    Ok(())
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn find_section_end(content: &str, section_header: &str) -> usize {
    if let Some(start) = content.find(section_header) {
        let after_header = start + section_header.len();
        // Find next ## or end of file
        if let Some(next_section) = content[after_header..].find("\n## ") {
            after_header + next_section
        } else {
            content.len()
        }
    } else {
        content.len()
    }
}

/// Deterministic FNV-1a hash — stable across Rust toolchain versions.
/// DefaultHasher is NOT stable across versions, which would silently
/// change fact IDs in items.json on compiler upgrades.
fn hash_fact(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in text.to_lowercase().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Condvar, Mutex};
    use tempfile::TempDir;

    #[test]
    fn confidence_ordering() {
        assert!(Confidence::Explicit > Confidence::Strong);
        assert!(Confidence::Strong > Confidence::Inferred);
        assert!(Confidence::Inferred > Confidence::Tentative);
        assert!(Confidence::Strong.meets(Confidence::Strong));
        assert!(Confidence::Explicit.meets(Confidence::Strong));
        assert!(!Confidence::Inferred.meets(Confidence::Strong));
    }

    #[test]
    fn slugify_names() {
        assert_eq!(slugify("Taylor Rivera"), "taylor-rivera");
        assert_eq!(slugify("  Casey  "), "casey");
        assert_eq!(slugify("Sam O'Brien"), "sam-o-brien");
    }

    #[test]
    fn update_from_meeting_skips_restricted_meetings() {
        let knowledge = TempDir::new().unwrap();
        let meetings = TempDir::new().unwrap();
        let path = meetings.path().join("2026-06-10-board.md");
        let raw = meeting_markdown("Board Sync", Some("restricted"), "Draft board memo");
        fs::write(&path, &raw).unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.path().to_path_buf(),
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let (frontmatter, _) = crate::markdown::split_frontmatter(&raw);
        let frontmatter: Frontmatter = serde_yaml::from_str(frontmatter).unwrap();
        let result = crate::WriteResult {
            path,
            title: "Board Sync".into(),
            word_count: 10,
            content_type: crate::ContentType::Meeting,
        };

        let update = update_from_meeting(&result, &frontmatter, "notes", &config).unwrap();
        assert_eq!(update.facts_written, 0);
        assert_eq!(update.facts_skipped, 0);
        assert!(update.people_updated.is_empty());
        assert!(!knowledge.path().join("people").exists());
        assert!(!knowledge.path().join("log.md").exists());
        let manifest: KnowledgeProvenanceManifest =
            serde_json::from_slice(&fs::read(provenance_manifest_path(&config).unwrap()).unwrap())
                .unwrap();
        assert!(manifest.sources.is_empty());
    }

    #[test]
    fn ingest_file_refuses_restricted_meetings() {
        let kb = TempDir::new().unwrap();
        let meetings = TempDir::new().unwrap();
        let meeting_path = meetings.path().join("2026-06-10-board.md");
        fs::write(
            &meeting_path,
            "---\ntitle: Board Sync\ntype: meeting\ndate: 2026-06-10T12:00:00-07:00\nduration: 30m\nsensitivity: restricted\nattendees: [Alex Kim]\naction_items: []\ndecisions: []\nintents: []\n---\n\nnotes\n",
        )
        .unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: kb.path().to_path_buf(),
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let error = ingest_file(&meeting_path, &config).unwrap_err();
        assert!(error.to_string().contains("restricted"));
        assert!(!kb.path().join("people").exists());
        assert!(!kb.path().join("log.md").exists());
    }

    fn meeting_markdown(title: &str, sensitivity: Option<&str>, task: &str) -> String {
        let sensitivity = sensitivity
            .map(|value| format!("sensitivity: {value}\n"))
            .unwrap_or_default();
        format!(
            "---\ntitle: {title}\ntype: meeting\ndate: 2026-06-10T12:00:00-07:00\nduration: 30m\n{sensitivity}attendees: [Alex Kim]\naction_items:\n  - assignee: Alex Kim\n    task: {task}\n    status: open\ndecisions: []\nintents: []\n---\n\nnotes\n"
        )
    }

    #[test]
    fn authorized_meeting_rejects_in_place_mutation_between_descriptor_reads() {
        let meetings = TempDir::new().unwrap();
        let path = meetings.path().join("mutable.md");
        fs::write(&path, meeting_markdown("Mutable", None, "NORMAL-CANARY")).unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            ..Config::default()
        };
        let replacement =
            meeting_markdown("Mutable", Some("restricted"), "RESTRICTED-IN-PLACE-CANARY");
        let error = authorized_meeting_with_hook(&path, &config, || {
            // fs::write truncates and rewrites the existing inode, exercising
            // the descriptor rather than a pathname replacement.
            fs::write(&path, &replacement).unwrap();
        })
        .unwrap_err();
        assert!(error.to_string().contains("changed during read"));
    }

    #[test]
    fn qmd_policy_mirror_retracts_restricted_malformed_and_linked_sources() {
        let meetings = TempDir::new().unwrap();
        let mirror_parent = TempDir::new().unwrap();
        let mirror = mirror_parent.path().canonicalize().unwrap().join("mirror");
        let normal = meetings.path().join("normal.md");
        let malformed = meetings.path().join("malformed.md");
        let restricted = meetings.path().join("restricted.md");
        fs::write(&normal, meeting_markdown("Normal", None, "NORMAL-CANARY")).unwrap();
        fs::write(&malformed, "---\ntitle: [broken\n---\nMALFORMED-CANARY").unwrap();
        fs::write(
            &restricted,
            meeting_markdown("Secret", Some("restricted"), "RESTRICTED-CANARY"),
        )
        .unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let outside = mirror_parent.path().join("outside.md");
            fs::write(&outside, meeting_markdown("Outside", None, "LINK-CANARY")).unwrap();
            symlink(&outside, meetings.path().join("linked.md")).unwrap();
        }

        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            ..Config::default()
        };
        let first = rebuild_qmd_policy_mirror_at(&config, &mirror).unwrap();
        assert_eq!(first.files, 1);
        let mirrored = fs::read_to_string(mirror.join("normal.md")).unwrap();
        assert!(mirrored.contains("NORMAL-CANARY"));
        let all_mirror_bytes = WalkDir::new(&mirror)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .flat_map(|entry| fs::read(entry.path()).unwrap())
            .collect::<Vec<_>>();
        let all_mirror_text = String::from_utf8_lossy(&all_mirror_bytes);
        assert!(!all_mirror_text.contains("MALFORMED-CANARY"));
        assert!(!all_mirror_text.contains("RESTRICTED-CANARY"));
        assert!(!all_mirror_text.contains("LINK-CANARY"));

        fs::write(
            &normal,
            meeting_markdown("Normal", Some("restricted"), "NORMAL-CANARY"),
        )
        .unwrap();
        let second = rebuild_qmd_policy_mirror_at(&config, &mirror).unwrap();
        assert_eq!(second.files, 0);
        assert!(!mirror.join("normal.md").exists());
    }

    #[derive(Debug, Default)]
    struct FakeQmdState {
        collections: BTreeMap<String, PathBuf>,
        show_spawn: HashSet<String>,
        show_nonzero: HashSet<String>,
        show_malformed: HashSet<String>,
        show_output_override: BTreeMap<String, String>,
        remove_fail: HashSet<String>,
        list_spawn: bool,
        list_io_error: Option<std::io::ErrorKind>,
        list_nonzero: bool,
        list_output_override: Option<String>,
        list_calls: usize,
        source_flip_on_list_call: Option<(usize, PathBuf, String)>,
        update_fail: bool,
        retarget_on_update: Option<(String, PathBuf)>,
        alias_on_update: Option<(String, PathBuf)>,
        source_flip_on_update: Option<(PathBuf, String)>,
        update_gate: Option<Arc<(Mutex<FakeQmdGate>, Condvar)>>,
        command_count: usize,
    }

    #[derive(Debug, Default)]
    struct FakeQmdGate {
        entered: bool,
        released: bool,
    }

    #[derive(Debug, Clone, Default)]
    struct FakeQmdRunner {
        state: Arc<Mutex<FakeQmdState>>,
    }

    impl FakeQmdRunner {
        fn with_collection(name: &str, path: &Path) -> Self {
            let runner = Self::default();
            runner
                .state
                .lock()
                .unwrap()
                .collections
                .insert(name.to_string(), path.to_path_buf());
            runner
        }

        fn contains(&self, name: &str) -> bool {
            self.state.lock().unwrap().collections.contains_key(name)
        }

        fn command_count(&self) -> usize {
            self.state.lock().unwrap().command_count
        }
    }

    impl QmdRunner for FakeQmdRunner {
        fn run_until(
            &self,
            args: &[&str],
            _deadline: Instant,
        ) -> Result<QmdCommandResult, QmdRunError> {
            let mut state = self.state.lock().unwrap();
            state.command_count += 1;
            match args {
                ["collection", "list"] => {
                    state.list_calls += 1;
                    if state
                        .source_flip_on_list_call
                        .as_ref()
                        .is_some_and(|(call, _, _)| *call == state.list_calls)
                    {
                        let (_, path, content) =
                            state.source_flip_on_list_call.take().expect("flip exists");
                        fs::write(path, content)
                            .map_err(|_| "source flip during list failed".to_string())?;
                    }
                    if state.list_spawn {
                        return Err("spawn failed".into());
                    }
                    if let Some(kind) = state.list_io_error {
                        return Err(QmdRunError::Io {
                            kind,
                            message: "synthetic qmd spawn failure".into(),
                        });
                    }
                    if state.list_nonzero {
                        return Ok(QmdCommandResult {
                            success: false,
                            stdout: String::new(),
                        });
                    }
                    if let Some(stdout) = state.list_output_override.clone() {
                        return Ok(QmdCommandResult {
                            success: true,
                            stdout,
                        });
                    }
                    let stdout = state
                        .collections
                        .keys()
                        .map(|name| format!("{name} (qmd://{name})\n"))
                        .collect::<String>();
                    let stdout =
                        format!("Collections ({}):\n\n{}", state.collections.len(), stdout);
                    Ok(QmdCommandResult {
                        success: true,
                        stdout,
                    })
                }
                ["collection", "show", name] => {
                    let Some(path) = state.collections.get(*name) else {
                        return Ok(QmdCommandResult {
                            success: false,
                            stdout: String::new(),
                        });
                    };
                    if state.show_spawn.contains(*name) {
                        return Err("spawn failed".into());
                    }
                    if state.show_nonzero.contains(*name) {
                        return Ok(QmdCommandResult {
                            success: false,
                            stdout: String::new(),
                        });
                    }
                    let stdout = if let Some(stdout) = state.show_output_override.get(*name) {
                        stdout.clone()
                    } else if state.show_malformed.contains(*name) {
                        "Name: malformed-without-path\n".to_string()
                    } else {
                        format!("Name: {name}\nPath: {}\n", path.display())
                    };
                    Ok(QmdCommandResult {
                        success: true,
                        stdout,
                    })
                }
                ["collection", "remove", name] => {
                    let success = !state.remove_fail.contains(*name);
                    if success {
                        state.collections.remove(*name);
                    }
                    Ok(QmdCommandResult {
                        success,
                        stdout: String::new(),
                    })
                }
                ["collection", "add", path, "--name", name] => {
                    state
                        .collections
                        .insert((*name).to_string(), PathBuf::from(path));
                    Ok(QmdCommandResult {
                        success: true,
                        stdout: String::new(),
                    })
                }
                ["update", "-c", _name] => {
                    let retarget = state.retarget_on_update.take();
                    let alias = state.alias_on_update.take();
                    let source_flip = state.source_flip_on_update.take();
                    let gate = state.update_gate.take();
                    let update_fail = state.update_fail;
                    drop(state);
                    if let Some(gate) = gate {
                        let (mutex, condition) = &*gate;
                        let mut status = mutex.lock().unwrap();
                        status.entered = true;
                        condition.notify_all();
                        while !status.released {
                            status = condition.wait(status).unwrap();
                        }
                    }
                    if let Some((path, content)) = source_flip {
                        fs::write(path, content).map_err(|_| "source flip failed".to_string())?;
                    }
                    let mut state = self.state.lock().unwrap();
                    if let Some((name, path)) = retarget {
                        state.collections.insert(name, path);
                    }
                    if let Some((name, path)) = alias {
                        state.collections.insert(name, path);
                    }
                    Ok(QmdCommandResult {
                        success: !update_fail,
                        stdout: String::new(),
                    })
                }
                _ => Err(format!("unexpected fake qmd command: {args:?}").into()),
            }
        }
    }

    #[cfg(unix)]
    struct ShellQmdRunner {
        script: String,
    }

    #[cfg(unix)]
    impl QmdRunner for ShellQmdRunner {
        fn run_until(
            &self,
            args: &[&str],
            deadline: Instant,
        ) -> Result<QmdCommandResult, QmdRunError> {
            let mut command = crate::bounded_child::BoundedCommand::new("sh");
            command.args(["-c", self.script.as_str(), "minutes-qmd-test"]);
            command.args(args);
            run_bounded_qmd_command(&mut command, deadline)
        }
    }

    fn qmd_fixture() -> (TempDir, TempDir, TempDir, Config, PathBuf) {
        let meetings = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let mirror_parent = TempDir::new().unwrap();
        crate::policy_fs::ensure_owner_only_directory(state.path()).unwrap();
        fs::write(
            meetings.path().join("normal.md"),
            meeting_markdown("Normal", None, "QMD-NORMAL"),
        )
        .unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            ..Config::default()
        };
        let mirror = mirror_parent.path().canonicalize().unwrap().join("mirror");
        (meetings, state, mirror_parent, config, mirror)
    }

    fn write_qmd_test_ownership_marker(directory: &Path) {
        let mut sources = BTreeMap::new();
        for entry in WalkDir::new(directory).follow_links(false) {
            let entry = entry.unwrap();
            if entry.depth() == 0 || !entry.file_type().is_file() {
                continue;
            }
            let relative = entry.path().strip_prefix(directory).unwrap();
            if relative == Path::new(QMD_MIRROR_MARKER) {
                continue;
            }
            sources.insert(
                qmd_relative_source_key(relative).unwrap(),
                source_revision(&fs::read_to_string(entry.path()).unwrap()),
            );
        }
        fs::write(
            directory.join(QMD_MIRROR_MARKER),
            serde_json::to_vec(&QmdMirrorMarker {
                schema: 2,
                source: "configured-meeting-corpus".into(),
                policy: "normal-only-strict-frontmatter-no-links".into(),
                sources,
            })
            .unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn qmd_parsers_reject_count_mismatch_and_ambiguous_show_paths() {
        let mirror = std::env::temp_dir().join("minutes-qmd-parser-mirror");
        let raw = std::env::temp_dir().join("minutes-qmd-parser-raw");
        let mirror_line = format!("Path: {}\n", mirror.display());
        let ambiguous = format!("Path: {}\nPath: {}\n", mirror.display(), raw.display());
        assert!(parse_qmd_collection_names("Collections (1):\n").is_err());
        assert!(
            parse_qmd_collection_names("Collections (2):\nminutes (qmd://minutes/)\n").is_err()
        );
        assert!(parse_qmd_collection_path("Path:\n").is_none());
        assert!(parse_qmd_collection_path(&ambiguous).is_none());
        assert_eq!(parse_qmd_collection_path(&mirror_line), Some(mirror));
        assert_eq!(
            parse_qmd_collection_names(
                "No collections found.\nRun 'qmd collection add .' to create one.\n"
            )
            .unwrap(),
            Vec::<String>::new()
        );
        assert!(parse_qmd_collection_names("No collections found.\n").is_err());
        assert!(parse_qmd_collection_names("Run 'qmd collection add .' to create one.\n").is_err());
        assert_eq!(
            parse_qmd_collection_names(
                "Collections (1):\n\nminutes (qmd://minutes/)\n  Pattern: **/*.md\n  Ignore: archive/**\n  [excluded]\n  Files: 4\n"
            )
            .unwrap(),
            vec!["minutes".to_string()]
        );
    }

    #[test]
    fn qmd_registry_parser_rejects_excessive_collection_counts_before_allocation() {
        let output = format!("Collections ({}):\n\n", MAX_QMD_REGISTRY_COLLECTIONS + 1);
        let error = parse_qmd_collection_names(&output).unwrap_err();
        assert!(error.contains("collection budget"));
    }

    #[test]
    fn qmd_operation_preserves_io_error_kind_and_caps_process_count() {
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);
        let operation = QmdOperationRunner::new(&runner);
        let error = operation.run(&["collection", "list"]).unwrap_err();
        assert!(error.contains("NotFound"));

        let runner = FakeQmdRunner::default();
        let operation = QmdOperationRunner::new(&runner);
        for _ in 0..MAX_QMD_OPERATION_COMMANDS {
            operation.run(&["collection", "list"]).unwrap();
        }
        let error = operation.run(&["collection", "list"]).unwrap_err();
        assert!(error.contains("command budget"));
        assert_eq!(runner.command_count(), MAX_QMD_OPERATION_COMMANDS);
    }

    #[test]
    fn qmd_policy_lock_wait_is_bounded_by_the_operation_deadline() {
        let directory = TempDir::new().unwrap();
        crate::policy_fs::ensure_owner_only_directory(directory.path()).unwrap();
        let _held = acquire_policy_lock_at(directory.path(), QMD_POLICY_LOCK).unwrap();
        let started = Instant::now();

        let error = acquire_policy_lock_at_until(
            directory.path(),
            QMD_POLICY_LOCK,
            Instant::now() + Duration::from_millis(100),
        )
        .err()
        .expect("a held policy lease must exceed the bounded deadline");

        assert!(error.to_string().contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn qmd_hung_child_is_killed_at_the_shared_operation_deadline() {
        let runner = ShellQmdRunner {
            script: "sleep 30".into(),
        };
        let operation = QmdOperationRunner::with_timeout(&runner, Duration::from_millis(150));
        let started = Instant::now();

        let error = operation.run(&["collection", "list"]).unwrap_err();

        assert!(error.contains("deadline"));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn qmd_stdout_and_stderr_are_rejected_at_per_command_limits() {
        let stdout_runner = ShellQmdRunner {
            script: "head -c 1048576 /dev/zero".into(),
        };
        let operation = QmdOperationRunner::with_timeout(&stdout_runner, Duration::from_secs(5));
        let stdout_error = operation.run(&["collection", "list"]).unwrap_err();
        assert!(stdout_error.contains("stdout resource budget"));

        let stderr_runner = ShellQmdRunner {
            script: "head -c 131072 /dev/zero >&2; printf 'Collections (0):\\n\\n'".into(),
        };
        let operation = QmdOperationRunner::with_timeout(&stderr_runner, Duration::from_secs(5));
        let stderr_error = operation.run(&["collection", "list"]).unwrap_err();
        assert!(stderr_error.contains("stderr resource budget"));
    }

    #[test]
    fn qmd_rebuild_reauthorizes_the_whole_source_set_before_swap() {
        let (meetings, _state, _mirror_parent, config, mirror) = qmd_fixture();
        rebuild_qmd_policy_mirror_at(&config, &mirror).unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "NEW-NORMAL-CANARY"),
        )
        .unwrap();
        let result = rebuild_qmd_policy_mirror_at_with_hook(&config, &mirror, |copied, _| {
            if copied == 1 {
                fs::write(
                    &source,
                    meeting_markdown(
                        "Normal",
                        Some("restricted"),
                        "RESTRICTED-DURING-COPY-CANARY",
                    ),
                )
                .unwrap();
            }
        });
        assert!(result.is_err());
        let visible = all_knowledge_text(&mirror);
        assert!(!visible.contains("NEW-NORMAL-CANARY"));
        assert!(!visible.contains("RESTRICTED-DURING-COPY-CANARY"));
    }

    #[test]
    fn qmd_refresh_failure_matrix_attempts_target_removal_and_never_reports_success() {
        for failure in [
            "list-spawn",
            "list-nonzero",
            "show-spawn",
            "show-nonzero",
            "show-malformed",
            "show-empty-path",
            "show-duplicate-path",
            "update",
        ] {
            let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
            let runner = FakeQmdRunner::with_collection("minutes", &mirror);
            {
                let mut fake = runner.state.lock().unwrap();
                match failure {
                    "list-spawn" => fake.list_spawn = true,
                    "list-nonzero" => fake.list_nonzero = true,
                    "show-spawn" => {
                        fake.show_spawn.insert("minutes".into());
                    }
                    "show-nonzero" => {
                        fake.show_nonzero.insert("minutes".into());
                    }
                    "show-malformed" => {
                        fake.show_malformed.insert("minutes".into());
                    }
                    "show-empty-path" => {
                        fake.show_output_override
                            .insert("minutes".into(), "Path:\n".into());
                    }
                    "show-duplicate-path" => {
                        fake.show_output_override.insert(
                            "minutes".into(),
                            format!(
                                "Path: {}\nPath: {}\n",
                                mirror.display(),
                                config.output_dir.display()
                            ),
                        );
                    }
                    "update" => fake.update_fail = true,
                    _ => unreachable!(),
                }
            }
            let result =
                refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
            assert!(result.is_err(), "failure={failure}");
            assert!(!runner.contains("minutes"), "failure={failure}");
        }
    }

    #[test]
    fn qmd_unrelated_uninspectable_collection_is_preserved_while_target_fails_closed() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let unrelated = mirror_parent.path().join("third-party-data");
        fs::create_dir_all(&unrelated).unwrap();
        let runner = FakeQmdRunner::with_collection("minutes", &mirror);
        {
            let mut fake = runner.state.lock().unwrap();
            fake.collections
                .insert("offline-third-party".into(), unrelated);
            fake.show_spawn.insert("offline-third-party".into());
        }

        let result =
            refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
        assert!(result.is_err());
        assert!(!runner.contains("minutes"));
        assert!(runner.contains("offline-third-party"));
    }

    #[test]
    fn qmd_removal_requires_independent_show_and_list_confirmation() {
        let parent = TempDir::new().unwrap();
        let path = parent.path().join("registered");
        fs::create_dir_all(&path).unwrap();
        let runner = FakeQmdRunner::with_collection("minutes", &path);
        runner.state.lock().unwrap().list_nonzero = true;
        let operation = QmdOperationRunner::new(&runner);
        let error = remove_and_confirm(&operation, &["minutes".to_string()]).unwrap_err();
        assert!(error.contains("could not be confirmed removed"));
        assert!(!runner.contains("minutes"));
    }

    #[test]
    fn qmd_removal_rejects_transient_show_absence_plus_malformed_successful_list() {
        for (target, list_output) in [
            ("minutes", "minutes\n"),
            ("minutes", " minutes (qmd://minutes/)\n"),
            ("Files: private", "Files: private (qmd://files-private/)\n"),
        ] {
            let parent = TempDir::new().unwrap();
            let path = parent.path().join("registered");
            fs::create_dir_all(&path).unwrap();
            let runner = FakeQmdRunner::with_collection(target, &path);
            {
                let mut state = runner.state.lock().unwrap();
                state.remove_fail.insert(target.into());
                state.show_nonzero.insert(target.into());
                state.list_output_override = Some(list_output.into());
            }
            let operation = QmdOperationRunner::new(&runner);
            let error = remove_and_confirm(&operation, &[target.to_string()]).unwrap_err();
            assert!(error.contains("could not be confirmed removed"));
            assert!(runner.contains(target));
        }
    }

    #[test]
    fn qmd_registration_replaces_raw_alias_with_exact_mirror_and_disables_on_update_failure() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("raw-alias", &config.output_dir);
        let registered =
            register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
                .unwrap();
        assert!(!runner.contains("raw-alias"));
        assert!(runner.contains("minutes"));
        assert_eq!(
            runner
                .state
                .lock()
                .unwrap()
                .collections
                .get("minutes")
                .unwrap(),
            &registered.path
        );

        runner.state.lock().unwrap().update_fail = true;
        let failed =
            register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
        assert!(failed.is_err());
        assert!(!runner.contains("minutes"));
    }

    #[test]
    fn pre_read_bridge_disables_persisted_qmd_target_after_configuration_removal() {
        let (meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
            .unwrap();
        assert!(runner.contains("minutes"));
        assert!(state.path().join(QMD_OWNED_TARGET).exists());
        fs::write(
            meetings.path().join("normal.md"),
            meeting_markdown("Normal", Some("restricted"), "QMD-NORMAL"),
        )
        .unwrap();

        let snapshot = knowledge_status_snapshot_with_actions(
            &config,
            || Err("configured refresh must not run".into()),
            || disable_unconfigured_qmd_with_at(&config, &runner, &mirror, state.path()),
        )
        .unwrap();
        assert!(!snapshot.enabled);
        assert!(!runner.contains("minutes"));
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());
    }

    #[test]
    fn invalid_configured_qmd_target_first_disables_persisted_valid_target() {
        for invalid in ["bad/name", ""] {
            let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
            let runner = FakeQmdRunner::default();
            register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
                .unwrap();
            assert!(runner.contains("minutes"));
            assert!(state.path().join(QMD_OWNED_TARGET).exists());

            let error =
                refresh_qmd_collection_with_at(&config, invalid, &runner, &mirror, state.path())
                    .unwrap_err();
            assert!(error.contains("invalid"));
            assert!(!runner.contains("minutes"));
            assert!(!state.path().join(QMD_OWNED_TARGET).exists());
        }
    }

    #[test]
    fn qmd_source_flip_during_update_disables_target_and_clears_ownership() {
        let (meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
            .unwrap();
        runner.state.lock().unwrap().source_flip_on_update = Some((
            meetings.path().join("normal.md"),
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "RESTRICTED-DURING-UPDATE-CANARY",
            ),
        ));

        let result =
            refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
        assert!(result.is_err());
        assert!(!runner.contains("minutes"));
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());
    }

    #[test]
    fn qmd_source_flip_during_post_registry_audit_fails_final_authorization() {
        let (meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("minutes", &mirror);
        runner.state.lock().unwrap().source_flip_on_list_call = Some((
            3,
            meetings.path().join("normal.md"),
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "RESTRICTED-DURING-POST-AUDIT-CANARY",
            ),
        ));

        let result =
            refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
        assert!(result.is_err());
        assert!(!runner.contains("minutes"));
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());
    }

    #[test]
    fn qmd_persistent_disable_retracts_registry_plaintext_and_ownership_before_idle_flip() {
        let (meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
            .unwrap();
        let stale_staging = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.staging-stale"));
        fs::create_dir(&stale_staging).unwrap();
        fs::write(stale_staging.join("leaked.md"), "QMD-STAGING-CANARY").unwrap();
        write_qmd_test_ownership_marker(&stale_staging);

        let error =
            reject_persistent_qmd_with_at(&config, "minutes", &runner, &mirror, state.path())
                .unwrap_err();

        assert!(error.contains("global index cannot guarantee revocation"));
        assert!(!runner.contains("minutes"));
        assert!(!mirror.exists());
        assert!(!stale_staging.exists());
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());

        fs::write(
            meetings.path().join("normal.md"),
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "RESTRICTED-WHILE-MINUTES-IDLE-CANARY",
            ),
        )
        .unwrap();
        assert!(!runner.contains("minutes"));
        assert!(!mirror.exists());
    }

    #[test]
    fn qmd_persistent_disable_removes_configured_target_without_prior_marker() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let unrelated_path = mirror_parent.path().join("previous-target");
        fs::create_dir(&unrelated_path).unwrap();
        let runner = FakeQmdRunner::with_collection("minutes", &unrelated_path);

        disable_qmd_persistence_with_at(&config, &runner, &mirror, state.path(), Some("minutes"))
            .unwrap();

        assert!(!runner.contains("minutes"));
        assert!(!mirror.exists());
    }

    #[test]
    fn qmd_cleanup_attestation_removes_orphan_transaction_plaintext_without_registry_state() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let orphan_staging = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.staging-orphan"));
        let orphan_previous = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.previous-orphan"));
        fs::create_dir(&orphan_staging).unwrap();
        fs::create_dir(&orphan_previous).unwrap();
        fs::write(orphan_staging.join("leaked.md"), "ORPHAN-STAGING-CANARY").unwrap();
        fs::write(orphan_previous.join("leaked.md"), "ORPHAN-PREVIOUS-CANARY").unwrap();
        write_qmd_test_ownership_marker(&orphan_staging);
        write_qmd_test_ownership_marker(&orphan_previous);

        let runner = FakeQmdRunner::default();
        disable_unconfigured_qmd_with_at(&config, &runner, &mirror, state.path()).unwrap();

        assert!(!orphan_staging.exists());
        assert!(!orphan_previous.exists());
        assert!(runner.command_count() >= 2);
    }

    #[test]
    fn qmd_plaintext_cleanup_preserves_a_replacement_at_the_atomic_claim_boundary() {
        let (_meetings, _state, _mirror_parent, _config, mirror) = qmd_fixture();
        let displaced = mirror.with_extension("bound-original");
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("legacy.md"), "BOUND-LEGACY-QMD-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);

        let result = purge_qmd_policy_plaintext_at_with_hook(&mirror, |candidate| {
            if candidate == mirror {
                fs::rename(&mirror, &displaced).unwrap();
                fs::create_dir(&mirror).unwrap();
                fs::write(
                    mirror.join("unrelated.txt"),
                    "UNRELATED-REPLACEMENT-MUST-SURVIVE",
                )
                .unwrap();
            }
        });

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(displaced.join("legacy.md")).unwrap(),
            "BOUND-LEGACY-QMD-CANARY"
        );
        let preserved_replacement = fs::read_dir(_mirror_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| path.join("unrelated.txt").exists())
            .unwrap();
        assert_eq!(
            fs::read_to_string(preserved_replacement.join("unrelated.txt")).unwrap(),
            "UNRELATED-REPLACEMENT-MUST-SURVIVE"
        );
    }

    #[test]
    fn qmd_plaintext_cleanup_preserves_prefix_only_unowned_directory() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        let lookalike = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.staging-unowned"));
        fs::create_dir(&lookalike).unwrap();
        fs::write(
            lookalike.join("unrelated.txt"),
            "UNOWNED-PREFIX-LOOKALIKE-MUST-SURVIVE",
        )
        .unwrap();

        let result = purge_qmd_policy_plaintext_at(&mirror);

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(lookalike.join("unrelated.txt")).unwrap(),
            "UNOWNED-PREFIX-LOOKALIKE-MUST-SURVIVE"
        );
    }

    #[test]
    fn qmd_plaintext_cleanup_preserves_child_replacement_after_attestation() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("legacy.md"), "BOUND-CHILD-QMD-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);
        let displaced = mirror_parent.path().join("displaced-bound-child.md");
        let observed_claim = std::cell::RefCell::new(None::<PathBuf>);

        let result = purge_qmd_policy_plaintext_at_with_hooks(
            &mirror,
            |_| {},
            |claimed| {
                observed_claim.replace(Some(claimed.to_path_buf()));
                fs::rename(claimed.join("legacy.md"), &displaced).unwrap();
                fs::write(
                    claimed.join("legacy.md"),
                    "UNRELATED-CHILD-REPLACEMENT-MUST-SURVIVE",
                )
                .unwrap();
            },
        );

        assert!(result.is_err());
        let expected_claim = observed_claim.into_inner().unwrap();
        assert_eq!(
            fs::read_to_string(&displaced).unwrap(),
            "BOUND-CHILD-QMD-CANARY"
        );
        assert_eq!(
            fs::read_to_string(expected_claim.join("legacy.md")).unwrap(),
            "UNRELATED-CHILD-REPLACEMENT-MUST-SURVIVE"
        );
        assert!(!expected_claim.join(QMD_RETIREMENT_RECEIPT).exists());
    }

    #[test]
    fn qmd_plaintext_cleanup_scales_past_low_mac_fd_limits() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        for index in 0..350 {
            fs::write(
                mirror.join(format!("meeting-{index:03}.md")),
                format!("bounded-qmd-file-{index}"),
            )
            .unwrap();
        }
        write_qmd_test_ownership_marker(&mirror);

        purge_qmd_policy_plaintext_at(&mirror).unwrap();

        assert!(!mirror.exists());
        let residues = fs::read_dir(mirror_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!(".{QMD_MIRROR_DIR}.retired-")))
            })
            .collect::<Vec<_>>();
        assert_eq!(residues.len(), 1);
        assert!(residues[0].join(QMD_RETIREMENT_RECEIPT).exists());
        for index in 0..350 {
            assert_eq!(
                fs::read_to_string(residues[0].join(format!("meeting-{index:03}.md"))).unwrap(),
                format!("bounded-qmd-file-{index}")
            );
        }
    }

    #[test]
    fn qmd_plaintext_cleanup_retries_random_claim_collisions() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("legacy.md"), "COLLISION-RETRY-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);
        let collision = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.retired-existing"));
        fs::create_dir(&collision).unwrap();
        write_qmd_test_ownership_marker(&collision);
        fs::write(
            collision.join(QMD_RETIREMENT_RECEIPT),
            QMD_RETIREMENT_RECEIPT_BYTES,
        )
        .unwrap();
        let claimed = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.retired-after-collision"));
        let attempts = std::cell::Cell::new(0usize);

        purge_qmd_policy_plaintext_at_with_claim_paths(&mirror, |_| {
            let attempt = attempts.get();
            attempts.set(attempt + 1);
            Ok(if attempt == 0 {
                collision.clone()
            } else {
                claimed.clone()
            })
        })
        .unwrap();

        assert_eq!(attempts.get(), 2);
        assert!(collision.join(QMD_RETIREMENT_RECEIPT).exists());
        assert!(claimed.join(QMD_RETIREMENT_RECEIPT).exists());
        assert_eq!(
            fs::read_to_string(claimed.join("legacy.md")).unwrap(),
            "COLLISION-RETRY-CANARY"
        );
    }

    #[test]
    fn qmd_plaintext_cleanup_preserves_a_late_replacement_after_an_earlier_proof() {
        let (_meetings, _state, _mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("a.md"), "FIRST-FILE-CANARY").unwrap();
        fs::write(mirror.join("b.md"), "SECOND-FILE-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);
        let displaced = mirror.with_extension("displaced-second");
        let observed_claim = std::cell::RefCell::new(None::<PathBuf>);
        let replaced = std::cell::Cell::new(false);

        let result =
            purge_qmd_policy_plaintext_at_with_retirement_hook(&mirror, |claimed, relative| {
                observed_claim.replace(Some(claimed.to_path_buf()));
                if relative == Path::new("a.md") && !replaced.replace(true) {
                    fs::rename(claimed.join("b.md"), &displaced).unwrap();
                    fs::write(claimed.join("b.md"), "LATE-REPLACEMENT-MUST-SURVIVE").unwrap();
                }
            });

        assert!(result.is_err());
        let claimed = observed_claim.into_inner().unwrap();
        assert_eq!(
            fs::read_to_string(claimed.join("a.md")).unwrap(),
            "FIRST-FILE-CANARY"
        );
        assert_eq!(
            fs::read_to_string(claimed.join("b.md")).unwrap(),
            "LATE-REPLACEMENT-MUST-SURVIVE"
        );
        assert_eq!(
            fs::read_to_string(&displaced).unwrap(),
            "SECOND-FILE-CANARY"
        );
        assert!(!claimed.join(QMD_RETIREMENT_RECEIPT).exists());
    }

    #[test]
    fn qmd_plaintext_cleanup_preserves_an_incomplete_marker_tree() {
        let (_meetings, _state, _mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("legacy.md"), "INCOMPLETE-MARKER-CANARY").unwrap();
        fs::write(
            mirror.join(QMD_MIRROR_MARKER),
            serde_json::to_vec(&QmdMirrorMarker {
                schema: 2,
                source: "configured-meeting-corpus".into(),
                policy: "normal-only-strict-frontmatter-no-links".into(),
                sources: BTreeMap::new(),
            })
            .unwrap(),
        )
        .unwrap();
        let result = purge_qmd_policy_plaintext_at(&mirror);

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(mirror.join("legacy.md")).unwrap(),
            "INCOMPLETE-MARKER-CANARY"
        );
    }

    #[test]
    fn qmd_plaintext_cleanup_never_truncates_unknown_receipted_residue_entries() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        let residue = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.retired-unsanitized"));
        fs::create_dir(&residue).unwrap();
        fs::write(
            residue.join(QMD_RETIREMENT_RECEIPT),
            QMD_RETIREMENT_RECEIPT_BYTES,
        )
        .unwrap();
        fs::write(residue.join("unknown.md"), "UNKNOWN-RESIDUE-CANARY").unwrap();

        let result = purge_qmd_policy_plaintext_at(&mirror);

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(residue.join("unknown.md")).unwrap(),
            "UNKNOWN-RESIDUE-CANARY"
        );
    }

    #[test]
    fn qmd_retirement_receipt_recheck_preserves_late_residue_replacement() {
        let (_meetings, _state, mirror_parent, _config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("legacy.md"), "RETIRED-CHILD-QMD-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);
        purge_qmd_policy_plaintext_at(&mirror).unwrap();
        let residue = fs::read_dir(mirror_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(&format!(".{QMD_MIRROR_DIR}.retired-")))
            })
            .unwrap();
        assert!(residue.join(QMD_RETIREMENT_RECEIPT).exists());
        let displaced = mirror_parent.path().join("displaced-retired-child.md");
        let hook_ran = std::cell::Cell::new(false);

        let result = purge_qmd_policy_plaintext_at_with_hooks(
            &mirror,
            |_| {},
            |candidate| {
                if candidate.file_name() == residue.file_name() {
                    hook_ran.set(true);
                    fs::rename(candidate.join("legacy.md"), &displaced).unwrap();
                    fs::write(
                        candidate.join("legacy.md"),
                        "UNRELATED-RETIRED-REPLACEMENT-MUST-SURVIVE",
                    )
                    .unwrap();
                }
            },
        );

        assert!(hook_ran.get());
        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(residue.join("legacy.md")).unwrap(),
            "UNRELATED-RETIRED-REPLACEMENT-MUST-SURVIVE"
        );
        assert_eq!(
            fs::read_to_string(displaced).unwrap(),
            "RETIRED-CHILD-QMD-CANARY"
        );
    }

    #[test]
    fn qmd_cleanup_attestation_removes_unmarked_raw_alias() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("legacy-raw", &config.output_dir);

        disable_unconfigured_qmd_with_at(&config, &runner, &mirror, state.path()).unwrap();

        assert!(!runner.contains("legacy-raw"));
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());
        assert!(!mirror.exists());
    }

    #[test]
    fn qmd_cleanup_attestation_fails_closed_when_registry_is_unavailable_after_plaintext_purge() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        fs::create_dir(&mirror).unwrap();
        fs::write(mirror.join("leaked.md"), "UNATTESTED-QMD-CANARY").unwrap();
        write_qmd_test_ownership_marker(&mirror);
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::PermissionDenied);

        let error =
            disable_unconfigured_qmd_with_at(&config, &runner, &mirror, state.path()).unwrap_err();

        assert!(error.contains("PermissionDenied"));
        assert!(!mirror.exists());
    }

    #[test]
    fn agent_startup_blocks_unusable_qmd_only_when_a_registration_may_exist() {
        // This case used to block on any spawn failure, including on machines
        // that had never registered anything. That was deliberate, on the
        // reasoning that Minutes cannot prove a negative from its own state.
        //
        // It was changed after #788, where a user who never opted into QMD was
        // locked out of every content-bearing tool with no route back: the
        // remediation Minutes prints runs this same audit and fails the same
        // way. The audit also fails closed on a QMD that is present but broken,
        // so this was reachable well beyond "never installed".
        //
        // The refined rule keeps the property the old one was protecting. A
        // registration Minutes made leaves a marker, a mirror, or a config
        // reference behind, and any of those still blocks. What no longer
        // blocks is a machine showing none of them, where there is no copy to
        // revoke and so nothing for the block to protect.
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::Interrupted,
        ] {
            let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
            let runner = FakeQmdRunner::default();
            runner.state.lock().unwrap().list_io_error = Some(kind);

            let readiness =
                evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

            assert!(
                readiness.ready,
                "spawn kind {kind:?} must not block a machine with no registration evidence"
            );
            // Still pending, so a later working QMD re-audits rather than
            // inheriting this pass.
            assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
        }

        // The same failures with an ownership marker present must still block:
        // that marker is Minutes saying it registered something.
        for kind in [
            std::io::ErrorKind::NotFound,
            std::io::ErrorKind::Interrupted,
        ] {
            let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
            save_qmd_owned_target(state.path(), "minutes").unwrap();
            let runner = FakeQmdRunner::default();
            runner.state.lock().unwrap().list_io_error = Some(kind);

            let readiness =
                evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

            assert!(
                !readiness.ready,
                "spawn kind {kind:?} must block when a registration marker exists"
            );
            assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
            assert!(readiness.remediation.is_some());
        }
    }

    /// Answers one `collection list` and then behaves as though the binary is
    /// gone, the way a qmd removed or replaced mid-run would.
    struct VanishingQmdRunner {
        lists: std::sync::atomic::AtomicUsize,
    }

    impl QmdRunner for VanishingQmdRunner {
        fn run_until(
            &self,
            args: &[&str],
            _deadline: Instant,
        ) -> Result<QmdCommandResult, QmdRunError> {
            if args == ["collection", "list"]
                && self.lists.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0
            {
                return Ok(QmdCommandResult {
                    success: true,
                    stdout: "Collections (0):\n".into(),
                });
            }
            Err(QmdRunError::Io {
                kind: std::io::ErrorKind::NotFound,
                message: "qmd vanished mid-run".into(),
            })
        }
    }

    #[test]
    fn residue_evidence_answers_yes_when_it_cannot_finish_looking() {
        // Every uncertain answer from the residue scan has to be "there is
        // residue". The scan used to read a bounded prefix of the directory and
        // treat an unreadable directory as clean, so on a machine with enough
        // entries the answer depended on readdir order, and on one we could not
        // open it was simply wrong in the unsafe direction.
        let parent = TempDir::new().unwrap();
        let mirror = parent.path().join("mirror");
        assert!(
            !qmd_mirror_residue_evidence(&mirror),
            "an empty parent has no residue"
        );

        fs::create_dir(parent.path().join(format!(".{QMD_MIRROR_DIR}.retired-old"))).unwrap();
        assert!(
            qmd_mirror_residue_evidence(&mirror),
            "a retired mirror directory is residue"
        );

        // More entries than the scan is willing to walk, and none of them
        // residue. Answering "no" here would be answering a question we
        // stopped reading before the end of, so it has to answer "yes".
        //
        // Asserted through the injected bound rather than by materializing the
        // real one. A test that writes 8193 files to prove this puts that load
        // on every other test running beside it, and asserting it on a sampled
        // prefix instead would make the result depend on readdir order, which
        // is the defect being tested for.
        let crowded = TempDir::new().unwrap();
        let crowded_mirror = crowded.path().join("mirror");
        for index in 0..4 {
            fs::write(crowded.path().join(format!("unrelated-{index}")), b"").unwrap();
        }
        assert!(
            qmd_mirror_residue_evidence_within(&crowded_mirror, 3),
            "a scan that gave up early must not report clean"
        );
        assert!(
            !qmd_mirror_residue_evidence_within(&crowded_mirror, 4),
            "a scan that read every entry and found nothing reports clean"
        );

        // A missing parent is the one honest "no": there is nowhere for a
        // mirror to have been.
        let absent = TempDir::new().unwrap();
        let absent_mirror = absent.path().join("gone").join("mirror");
        assert!(!qmd_mirror_residue_evidence(&absent_mirror));
    }

    #[test]
    #[cfg(unix)]
    fn an_unreadable_mirror_parent_counts_as_residue() {
        use std::os::unix::fs::PermissionsExt;
        let parent = TempDir::new().unwrap();
        let sealed = parent.path().join("sealed");
        fs::create_dir(&sealed).unwrap();
        let mirror = sealed.join("mirror");
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o000)).unwrap();
        let evidence = qmd_mirror_residue_evidence(&mirror);
        // Restore before asserting so a failure cannot leave the tree sealed.
        fs::set_permissions(&sealed, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            evidence,
            "a directory we cannot open must not be reported clean"
        );
    }

    #[test]
    fn a_retraction_reports_the_evidence_it_saw_before_it_started() {
        // The CLI decides whether to tell the user their cleanup is confirmed,
        // and it must decide from the same snapshot readiness uses. When it
        // computed its own weaker version, `minutes qmd cleanup` reported
        // success on a machine carrying an ownership marker while readiness
        // went on blocking on that same marker.
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        save_qmd_owned_target(state.path(), "minutes").unwrap();
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);

        let outcome = disable_qmd_persistence_reporting_at(
            &config,
            &runner,
            &mirror,
            state.path(),
            config.search.qmd_collection.as_deref(),
        );

        assert!(outcome.result.is_err());
        assert!(outcome.registry_never_answered);
        assert!(
            outcome.registration_evidence,
            "the marker present at entry must be reported, even though the retraction ran after it"
        );
    }

    #[test]
    fn a_registry_that_answered_once_never_counts_as_unreachable() {
        // The #788 pass requires that qmd told us nothing at all. Deciding that
        // from the *last* failure instead of the whole run is wrong: the
        // retraction re-audits after its removals, so a registry that listed
        // fine and then failed would look identical to one that was never
        // there. On a machine whose local marker had been deleted, that would
        // report ready while a real Minutes-owned collection still held the
        // meeting text.
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = VanishingQmdRunner {
            lists: std::sync::atomic::AtomicUsize::new(0),
        };

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(
            !readiness.ready,
            "a registry that answered once must keep failing closed"
        );
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
    }

    #[test]
    fn a_mirror_or_configured_collection_also_counts_as_registration_evidence() {
        // The marker is not the only trace. A leftover plaintext mirror, or a
        // config that still names a QMD collection, each mean a registration
        // may exist, and each must keep failing closed when QMD cannot answer.
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        fs::create_dir_all(&mirror).unwrap();
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);
        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());
        assert!(!readiness.ready, "a leftover mirror must still block");

        let (_meetings2, state2, _parent2, mut config2, mirror2) = qmd_fixture();
        config2.search.qmd_collection = Some("minutes".to_string());
        let runner2 = FakeQmdRunner::default();
        runner2.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);
        let readiness2 =
            evaluate_agent_trust_readiness_with_at(&config2, &runner2, &mirror2, state2.path());
        assert!(
            !readiness2.ready,
            "a configured QMD collection must still block"
        );
    }

    #[test]
    fn agent_readiness_revalidation_does_not_reuse_a_prior_ready_result() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        let first = evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());
        assert!(first.ready);

        // Use a registry that answers with garbage rather than one that will
        // not spawn. Both used to block, but an unreachable registry on a
        // machine with no registration evidence is now allowed through (#788),
        // which would make this test pass for the wrong reason. What is under
        // test here is that readiness is recomputed, not the retirement policy.
        runner.state.lock().unwrap().list_output_override =
            Some("unparseable QMD registry output".into());
        let second =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(!second.ready);
        assert_eq!(second.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
    }

    #[test]
    fn stale_local_retirement_receipt_never_attests_an_unavailable_external_registry() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let stale = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.retired-stale-attestation"));
        fs::create_dir(&stale).unwrap();
        fs::write(
            stale.join(QMD_RETIREMENT_RECEIPT),
            QMD_RETIREMENT_RECEIPT_BYTES,
        )
        .unwrap();
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(!readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
    }

    #[test]
    fn agent_startup_blocks_unavailable_qmd_when_legacy_plaintext_evidence_existed() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let stale_staging = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.staging-startup"));
        let stale_previous = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.previous-startup"));
        for directory in [&mirror, &stale_staging, &stale_previous] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("legacy.md"), "LEGACY-QMD-STARTUP-CANARY").unwrap();
            write_qmd_test_ownership_marker(directory);
        }
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_io_error = Some(std::io::ErrorKind::NotFound);

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(!readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert!(readiness
            .remediation
            .as_deref()
            .is_some_and(|message| message.contains("minutes qmd cleanup")));
        assert!(!mirror.exists());
        assert!(!stale_staging.exists());
        assert!(!stale_previous.exists());
        assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
    }

    #[test]
    fn agent_startup_blocks_malformed_qmd_registry_and_keeps_retry_evidence() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        runner.state.lock().unwrap().list_output_override =
            Some("unparseable QMD registry output".into());

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(!readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
    }

    #[test]
    fn agent_startup_blocks_an_uninspectable_registry_collection() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let unrelated = mirror_parent.path().join("unrelated-qmd-root");
        fs::create_dir(&unrelated).unwrap();
        let runner = FakeQmdRunner::with_collection("uninspectable", &unrelated);
        runner
            .state
            .lock()
            .unwrap()
            .show_malformed
            .insert("uninspectable".into());

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(!readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert!(state.path().join(QMD_RETIREMENT_PENDING).exists());
    }

    #[test]
    fn agent_startup_retires_raw_alias_and_all_legacy_mirror_transaction_names() {
        let (_meetings, state, mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("legacy-raw", &config.output_dir);
        let stale_staging = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.staging-startup"));
        let stale_previous = mirror_parent
            .path()
            .join(format!(".{QMD_MIRROR_DIR}.previous-startup"));
        for directory in [&mirror, &stale_staging, &stale_previous] {
            fs::create_dir(directory).unwrap();
            fs::write(directory.join("legacy.md"), "LEGACY-QMD-STARTUP-CANARY").unwrap();
            write_qmd_test_ownership_marker(directory);
        }

        let readiness =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());

        assert!(readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::ReadyClean);
        assert!(!runner.contains("legacy-raw"));
        assert!(!mirror.exists());
        assert!(!stale_staging.exists());
        assert!(!stale_previous.exists());
        assert!(!state.path().join(QMD_OWNED_TARGET).exists());
        assert!(!state.path().join(QMD_RETIREMENT_PENDING).exists());

        let retired_before = fs::read_dir(mirror_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| {
                name.to_string_lossy()
                    .starts_with(&format!(".{QMD_MIRROR_DIR}.retired-"))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(retired_before.len(), 3);
        let second =
            evaluate_agent_trust_readiness_with_at(&config, &runner, &mirror, state.path());
        assert!(second.ready);
        assert_eq!(second.qmd_retirement, QmdRetirementReadiness::ReadyClean);
        let retired_after = fs::read_dir(mirror_parent.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| {
                name.to_string_lossy()
                    .starts_with(&format!(".{QMD_MIRROR_DIR}.retired-"))
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(retired_after, retired_before);
    }

    #[test]
    fn malformed_strict_config_blocks_without_running_qmd_against_defaults() {
        let (_meetings, state, _mirror_parent, _config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        let readiness = evaluate_agent_trust_readiness_from_strict_config_with_at(
            Err("PRIVATE-CONFIG-CANARY".into()),
            &runner,
            &mirror,
            state.path(),
        );

        assert!(!readiness.ready);
        assert_eq!(readiness.qmd_retirement, QmdRetirementReadiness::Blocked);
        assert_eq!(runner.state.lock().unwrap().list_calls, 0);
        let error = readiness.require_ready().unwrap_err();
        assert!(error.contains("minutes qmd cleanup"));
        assert!(!error.contains("PRIVATE-CONFIG-CANARY"));
    }

    #[test]
    fn qmd_persistent_disable_reports_unconfirmed_retraction_and_keeps_retry_marker() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::default();
        register_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
            .unwrap();
        runner
            .state
            .lock()
            .unwrap()
            .remove_fail
            .insert("minutes".into());

        let error = disable_qmd_persistence_with_at(
            &config,
            &runner,
            &mirror,
            state.path(),
            Some("minutes"),
        )
        .unwrap_err();

        assert!(error.contains("could not be confirmed removed"));
        assert!(runner.contains("minutes"));
        assert!(state.path().join(QMD_OWNED_TARGET).exists());
        assert!(!mirror.exists());
    }

    #[test]
    fn qmd_refresh_disables_target_on_empty_rebuild_update_retarget_or_concurrent_alias() {
        for failure in ["empty", "rebuild", "retarget", "alias"] {
            let (meetings, state, _mirror_parent, mut config, mirror) = qmd_fixture();
            let runner = FakeQmdRunner::with_collection("minutes", &mirror);
            match failure {
                "empty" => fs::remove_file(meetings.path().join("normal.md")).unwrap(),
                "rebuild" => config.output_dir = meetings.path().join("missing-root"),
                "retarget" => {
                    runner.state.lock().unwrap().retarget_on_update =
                        Some(("minutes".into(), meetings.path().to_path_buf()));
                }
                "alias" => {
                    runner.state.lock().unwrap().alias_on_update =
                        Some(("minutes-copy".into(), mirror.clone()));
                }
                _ => unreachable!(),
            }
            let result =
                refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path());
            assert!(result.is_err(), "failure={failure}");
            assert!(!runner.contains("minutes"), "failure={failure}");
            assert!(!runner.contains("minutes-copy"), "failure={failure}");
        }
    }

    #[test]
    fn qmd_removal_failure_is_reported_and_target_is_still_disabled() {
        let (_meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("minutes", &mirror);
        {
            let mut fake = runner.state.lock().unwrap();
            fake.collections
                .insert("raw-alias".into(), config.output_dir.clone());
            fake.remove_fail.insert("raw-alias".into());
        }
        let error =
            refresh_qmd_collection_with_at(&config, "minutes", &runner, &mirror, state.path())
                .unwrap_err();
        assert!(error.contains("could not be confirmed removed"));
        assert!(!runner.contains("minutes"));
        assert!(runner.contains("raw-alias"));
    }

    #[test]
    fn qmd_two_worker_transaction_serializes_rebuild_attestation_and_update() {
        use std::sync::mpsc;
        use std::time::Duration;

        let (meetings, state, _mirror_parent, config, mirror) = qmd_fixture();
        let runner = FakeQmdRunner::with_collection("minutes", &mirror);
        let gate = Arc::new((Mutex::new(FakeQmdGate::default()), Condvar::new()));
        runner.state.lock().unwrap().update_gate = Some(gate.clone());

        let first_config = config.clone();
        let first_runner = runner.clone();
        let first_mirror = mirror.clone();
        let first_lock = state.path().to_path_buf();
        let first = std::thread::spawn(move || {
            refresh_qmd_collection_with_at(
                &first_config,
                "minutes",
                &first_runner,
                &first_mirror,
                &first_lock,
            )
        });

        let (gate_mutex, gate_condition) = &*gate;
        let mut gate_status = gate_mutex.lock().unwrap();
        while !gate_status.entered {
            let (status, timeout) = gate_condition
                .wait_timeout(gate_status, Duration::from_secs(10))
                .unwrap();
            gate_status = status;
            assert!(
                !timeout.timed_out(),
                "first QMD worker did not reach the update gate within 10 seconds; finished={}",
                first.is_finished()
            );
        }
        drop(gate_status);
        let count_while_first_holds_lock = runner.command_count();

        // The next worker sees the source's newer restricted state, but must
        // not enter any registry/rebuild step until the first worker releases
        // the single collection transaction lock.
        fs::write(
            meetings.path().join("normal.md"),
            meeting_markdown("Normal", Some("restricted"), "QMD-NORMAL"),
        )
        .unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let second_config = config.clone();
        let second_runner = runner.clone();
        let second_mirror = mirror.clone();
        let second_lock = state.path().to_path_buf();
        let second = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            refresh_qmd_collection_with_at(
                &second_config,
                "minutes",
                &second_runner,
                &second_mirror,
                &second_lock,
            )
        });
        started_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(runner.command_count(), count_while_first_holds_lock);

        let mut gate_status = gate_mutex.lock().unwrap();
        gate_status.released = true;
        gate_condition.notify_all();
        drop(gate_status);
        // The first worker's mirror was authorized before the source flipped,
        // but its post-update attestation observes the restricted revision. It
        // must therefore fail closed and disable the collection; serialization
        // does not grant a stale transaction permission to publish.
        assert!(first.join().unwrap().is_err());
        assert!(second.join().unwrap().is_err());
        assert!(!runner.contains("minutes"));
        assert!(!mirror.join("normal.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn exact_qmd_registration_rejects_symlink_components() {
        use std::os::unix::fs::symlink;

        let parent = TempDir::new().unwrap();
        let canonical_parent = parent.path().canonicalize().unwrap();
        let real = canonical_parent.join("real/mirror");
        fs::create_dir_all(&real).unwrap();
        symlink(
            canonical_parent.join("real"),
            canonical_parent.join("linked"),
        )
        .unwrap();
        let through_link = canonical_parent.join("linked/mirror");
        assert!(!exact_stable_path(&through_link, &through_link).unwrap());
        assert!(!exact_stable_path(&through_link, &real).unwrap());
        assert!(exact_stable_path(&real, &real).unwrap());
    }

    #[test]
    fn live_corpus_policy_excludes_every_inactive_directory_for_direct_and_mirror_reads() {
        let meetings = TempDir::new().unwrap();
        let mirror_parent = TempDir::new().unwrap();
        let live = meetings.path().join("teams/live.md");
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        fs::write(&live, meeting_markdown("Live", None, "LIVE-CANARY")).unwrap();
        let mut excluded = Vec::new();
        for (index, name) in crate::markdown::INACTIVE_CORPUS_DIRS.iter().enumerate() {
            let spelling = if index % 2 == 0 {
                name.to_ascii_uppercase()
            } else {
                name.to_string()
            };
            let path = meetings.path().join(&spelling).join("nested/hidden.md");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                meeting_markdown(name, None, &format!("{name}-CANARY")),
            )
            .unwrap();
            excluded.push((spelling, path));
        }
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            ..Config::default()
        };

        let candidates = live_corpus_markdown_paths(&config);
        assert_eq!(candidates, vec![live.clone()]);
        assert!(authorized_meeting(&live, &config).is_ok());
        for (_, path) in &excluded {
            assert!(authorized_meeting(path, &config).is_err());
        }

        let mirror = mirror_parent.path().join("mirror");
        let rebuilt = rebuild_qmd_policy_mirror_at(&config, &mirror).unwrap();
        assert_eq!(rebuilt.files, 1);
        assert!(mirror.join("teams/live.md").exists());
        for (name, _) in &excluded {
            assert!(!mirror.join(name).exists());
        }
    }

    fn all_knowledge_text(root: &Path) -> String {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                let name = entry.file_name().to_string_lossy();
                name != ".minutes-preserved-knowledge"
                    && name != ".minutes-private-reconcile"
                    && !name.starts_with(".minutes-person-")
            })
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| fs::read(entry.path()).ok())
            .flat_map(|bytes| bytes.into_iter().chain(std::iter::once(b'\n')))
            .map(char::from)
            .collect()
    }

    fn all_text_unfiltered(root: &Path) -> String {
        WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_type().is_file())
            .filter_map(|entry| fs::read(entry.path()).ok())
            .flat_map(|bytes| bytes.into_iter().chain(std::iter::once(b'\n')))
            .map(char::from)
            .collect()
    }

    fn knowledge_config(meetings: &TempDir, knowledge: &TempDir, adapter: &str) -> Config {
        let knowledge_path = knowledge.path().join("visible-kb");
        fs::create_dir_all(&knowledge_path).unwrap();
        Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge_path,
                adapter: adapter.into(),
                ..Default::default()
            },
            ..Config::default()
        }
    }

    fn all_preserved_text(config: &Config) -> String {
        preservation_namespace(config)
            .map(|namespace| all_knowledge_text(&namespace))
            .unwrap_or_default()
    }

    #[test]
    fn preservation_root_stays_outside_nested_knowledge_and_source_corpus() {
        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let knowledge = output.join("wiki");
        fs::create_dir_all(&knowledge).unwrap();
        let config = Config {
            output_dir: output.clone(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.clone(),
                ..Default::default()
            },
            ..Config::default()
        };
        let root = preservation_root(&config).unwrap();
        let canonical = root.canonicalize().unwrap();
        assert!(!canonical.starts_with(output.canonicalize().unwrap()));
        assert!(!canonical.starts_with(knowledge.canonicalize().unwrap()));
    }

    #[test]
    fn para_private_root_does_not_fall_back_when_knowledge_parent_is_public() {
        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let knowledge = output.join("wiki");
        fs::create_dir_all(knowledge.join("nested")).unwrap();
        let config = Config {
            output_dir: output.clone(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.join("nested/.."),
                adapter: "para".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let error = para_private_root(&config)
            .expect_err("an unsafe deterministic first candidate must fail without fallback");
        assert!(error.to_string().contains("overlaps"));
    }

    #[cfg(unix)]
    #[test]
    fn para_private_root_rejects_symlink_alias_containment_without_fallback() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let real_knowledge = output.join("real-wiki");
        let alias = workspace.path().join("wiki-alias");
        fs::create_dir_all(&real_knowledge).unwrap();
        symlink(&real_knowledge, &alias).unwrap();
        let config = Config {
            output_dir: output.clone(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: alias,
                adapter: "para".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let error = para_private_root(&config)
            .expect_err("a canonical public parent must never select an alternate root");
        assert!(error.to_string().contains("overlaps"));
    }

    #[test]
    fn para_private_root_is_the_deterministic_knowledge_parent_sibling() {
        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let knowledge_parent = workspace.path().join("knowledge-parent");
        let knowledge = knowledge_parent.join("wiki");
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(&knowledge).unwrap();
        let config = Config {
            output_dir: output,
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.clone(),
                adapter: "para".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let private = para_private_root(&config).unwrap();

        let canonical_knowledge_parent = knowledge_parent.canonicalize().unwrap();
        let canonical_knowledge = knowledge.canonicalize().unwrap();
        assert_eq!(private.parent(), Some(canonical_knowledge_parent.as_path()));
        assert!(!private.starts_with(&canonical_knowledge));
    }

    #[test]
    fn para_private_root_rejects_injected_cross_scope_identity() {
        let error = attest_para_directory_scopes_match(
            QmdObjectIdentity {
                scope: 11,
                object: 1,
            },
            QmdObjectIdentity {
                scope: 12,
                object: 2,
            },
        )
        .expect_err("cross-filesystem private storage must fail closed");
        assert!(error.to_string().contains("another filesystem or volume"));
    }

    #[test]
    fn para_private_root_rejects_existing_wrong_shape_people_path() {
        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let knowledge = workspace.path().join("knowledge/wiki");
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(knowledge.join("areas")).unwrap();
        fs::write(knowledge.join("areas/people"), b"not-a-directory").unwrap();
        let config = Config {
            output_dir: output,
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge,
                adapter: "para".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let error = para_private_root(&config).expect_err("wrong-shape people must fail closed");
        assert!(error.to_string().contains("not a real directory"));
    }

    #[cfg(unix)]
    #[test]
    fn para_private_root_rejects_symlink_people_path() {
        use std::os::unix::fs::symlink;

        let workspace = TempDir::new().unwrap();
        let output = workspace.path().join("meetings");
        let knowledge = workspace.path().join("knowledge/wiki");
        let elsewhere = workspace.path().join("elsewhere-people");
        fs::create_dir_all(&output).unwrap();
        fs::create_dir_all(knowledge.join("areas")).unwrap();
        fs::create_dir_all(&elsewhere).unwrap();
        symlink(&elsewhere, knowledge.join("areas/people")).unwrap();
        let config = Config {
            output_dir: output,
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge,
                adapter: "para".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let error = para_private_root(&config).expect_err("symlink people must fail closed");
        assert!(error.to_string().contains("not a real directory"));
    }

    #[test]
    fn windows_provenance_state_model_covers_every_exact_handle_phase() {
        let proof = WindowsProvenanceFileProof {
            identity: QmdObjectIdentity {
                scope: 7,
                object: 7,
            },
            len: 7,
            sha256: [7; 32],
        };
        use WindowsProvenanceActiveLayout as Layout;
        use WindowsProvenanceObservedFile as FileState;

        assert_eq!(
            classify_windows_provenance_active_layout(
                Some(&proof),
                FileState::Previous,
                FileState::Intended,
                FileState::Absent,
            ),
            Layout::PreMutation
        );
        assert_eq!(
            classify_windows_provenance_active_layout(
                Some(&proof),
                FileState::Absent,
                FileState::Intended,
                FileState::Previous,
            ),
            Layout::PreviousParked
        );
        assert_eq!(
            classify_windows_provenance_active_layout(
                Some(&proof),
                FileState::Intended,
                FileState::Absent,
                FileState::Previous,
            ),
            Layout::PublishedWithPrevious
        );
        assert_eq!(
            classify_windows_provenance_active_layout(
                Some(&proof),
                FileState::Intended,
                FileState::Absent,
                FileState::Empty,
            ),
            Layout::PublishedRetired
        );
        assert_eq!(
            classify_windows_provenance_active_layout(
                None,
                FileState::Absent,
                FileState::Intended,
                FileState::Absent,
            ),
            Layout::PreMutation
        );
        assert_eq!(
            classify_windows_provenance_active_layout(
                None,
                FileState::Intended,
                FileState::Absent,
                FileState::Empty,
            ),
            Layout::PublishedRetired
        );
        for hostile in [
            (FileState::Other, FileState::Intended, FileState::Absent),
            (FileState::Previous, FileState::Other, FileState::Absent),
            (FileState::Previous, FileState::Intended, FileState::Other),
            (
                FileState::Intended,
                FileState::Intended,
                FileState::Previous,
            ),
            (FileState::Absent, FileState::Absent, FileState::Previous),
        ] {
            assert_eq!(
                classify_windows_provenance_active_layout(
                    Some(&proof),
                    hostile.0,
                    hostile.1,
                    hostile.2,
                ),
                Layout::Unknown,
                "a winner, swap, extension, or missing exact source must fail closed"
            );
        }
        assert_eq!(
            PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
                .iter()
                .collect::<BTreeSet<_>>()
                .len(),
            3
        );
        const { assert!(MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES < 64 * 1024) };
    }

    #[test]
    fn windows_provenance_torn_active_aborts_only_unjournaled_temp() {
        use WindowsProvenanceObservedFile as FileState;
        let published = WindowsProvenanceFileProof {
            identity: QmdObjectIdentity {
                scope: 11,
                object: 11,
            },
            len: 11,
            sha256: [11; 32],
        };

        for temp in [FileState::Intended, FileState::Previous, FileState::Other] {
            assert!(windows_provenance_terminal_can_abort_unjournaled_temp(
                Some(&published),
                FileState::Intended,
                FileState::Absent,
            ));
            assert!(!windows_provenance_terminal_layout_is_exact(
                Some(&published),
                FileState::Intended,
                temp,
                FileState::Absent,
            ));
        }
        assert!(windows_provenance_terminal_can_abort_unjournaled_temp(
            None,
            FileState::Absent,
            FileState::Empty,
        ));
        assert!(
            !windows_provenance_terminal_can_abort_unjournaled_temp(
                Some(&published),
                FileState::Other,
                FileState::Absent,
            ),
            "a target winner cannot be scrubbed as an unjournaled temp"
        );
        assert!(
            !windows_provenance_terminal_can_abort_unjournaled_temp(
                Some(&published),
                FileState::Intended,
                FileState::Previous,
            ),
            "a parked previous generation proves mutation may have begun"
        );
    }

    #[test]
    fn windows_provenance_three_slot_journal_preserves_authority_across_torn_completion() {
        let previous = WindowsProvenanceFileProof {
            identity: QmdObjectIdentity {
                scope: 5,
                object: 5,
            },
            len: 5,
            sha256: [5; 32],
        };
        let intended = WindowsProvenanceFileProof {
            identity: QmdObjectIdentity {
                scope: 9,
                object: 9,
            },
            len: 9,
            sha256: [9; 32],
        };
        let terminal = WindowsProvenanceJournalRecord {
            schema: 1,
            sequence: 4,
            state: WindowsProvenanceJournalState::Completed,
            prior_sequence: None,
            previous: None,
            intended: Some(previous.clone()),
        };
        let active = WindowsProvenanceJournalRecord {
            schema: 1,
            sequence: 5,
            state: WindowsProvenanceJournalState::Active,
            prior_sequence: Some(4),
            previous: Some(previous),
            intended: Some(intended.clone()),
        };
        let completed = WindowsProvenanceJournalRecord {
            schema: 1,
            sequence: 6,
            state: WindowsProvenanceJournalState::Completed,
            prior_sequence: None,
            previous: None,
            intended: Some(intended),
        };

        let torn_completion = vec![(0, terminal.clone()), (1, active.clone())];
        assert_eq!(
            select_latest_windows_provenance_record(&torn_completion)
                .unwrap()
                .unwrap(),
            (1, active.clone()),
            "a malformed/empty third slot leaves the durable Active record authoritative"
        );
        let completion_before_resets = vec![(0, terminal), (1, active), (2, completed.clone())];
        assert_eq!(
            select_latest_windows_provenance_record(&completion_before_resets)
                .unwrap()
                .unwrap(),
            (2, completed.clone()),
            "a durable completion outranks still-present prior and Active slots"
        );
        assert!(
            select_latest_windows_provenance_record(&[(0, completed.clone()), (1, completed)])
                .is_err()
        );
    }

    fn write_private_control_fixture(
        directory: &crate::policy_fs::BoundRecoveryDirectory,
        name: &str,
        bytes: &[u8],
    ) {
        let mut file = directory.create_new_exact_file(OsStr::new(name)).unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    #[test]
    fn windows_provenance_action_boundaries_preserve_swapped_winners() {
        for source_name in [
            PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST,
            PRIVATE_KNOWLEDGE_PROVENANCE_TEMP,
        ] {
            let root = TempDir::new().unwrap();
            let namespace = root.path().join("private");
            let directory =
                crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace)
                    .unwrap();
            write_private_control_fixture(&directory, source_name, b"OBSERVED-EXACT");
            let observed = directory.bind_exact_file(OsStr::new(source_name)).unwrap();
            let proof =
                windows_provenance_file_proof(&observed, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)
                    .unwrap();
            drop(observed);
            let displaced = namespace.join(format!("{source_name}.displaced"));
            let error = rename_windows_provenance_exact_no_replace_with_hook(
                &directory,
                OsStr::new(source_name),
                OsStr::new("published"),
                &proof,
                || {
                    fs::rename(namespace.join(source_name), &displaced).unwrap();
                    write_private_control_fixture(&directory, source_name, b"HOSTILE-WINNER");
                },
            )
            .expect_err("a source-name winner must not be moved");
            assert!(error.to_string().contains("exact-rename"));
            assert_eq!(
                fs::read(namespace.join(source_name)).unwrap(),
                b"HOSTILE-WINNER"
            );
            assert_eq!(fs::read(displaced).unwrap(), b"OBSERVED-EXACT");
            assert!(!namespace.join("published").exists());
        }

        for source_name in [
            PRIVATE_KNOWLEDGE_PROVENANCE_TEMP,
            PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP,
        ] {
            let root = TempDir::new().unwrap();
            let namespace = root.path().join("private");
            let directory =
                crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace)
                    .unwrap();
            write_private_control_fixture(&directory, source_name, b"OBSERVED-EXACT");
            let observed = directory.bind_exact_file(OsStr::new(source_name)).unwrap();
            let proof =
                windows_provenance_file_proof(&observed, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)
                    .unwrap();
            drop(observed);
            let displaced = namespace.join(format!("{source_name}.displaced"));
            let error = zero_observed_windows_provenance_residue_with_hook(
                &directory,
                OsStr::new(source_name),
                &proof,
                MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES,
                || {
                    fs::rename(namespace.join(source_name), &displaced).unwrap();
                    write_private_control_fixture(&directory, source_name, b"HOSTILE-WINNER");
                },
            )
            .expect_err("a residue-name winner must not be zeroed");
            assert!(error.to_string().contains("exact-zero"));
            assert_eq!(
                fs::read(namespace.join(source_name)).unwrap(),
                b"HOSTILE-WINNER"
            );
            assert_eq!(fs::read(displaced).unwrap(), b"OBSERVED-EXACT");
        }

        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        let backup_name = OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP);
        write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP, b"");
        let observed = directory.bind_exact_file(backup_name).unwrap();
        let proof =
            windows_provenance_file_proof(&observed, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)
                .unwrap();
        drop(observed);
        let displaced_empty = namespace.join("empty-backup.displaced");
        let error = remove_observed_empty_windows_provenance_residue_with_hook(
            &directory,
            backup_name,
            &proof,
            || {
                fs::rename(namespace.join(backup_name), &displaced_empty).unwrap();
                write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP, b"");
            },
        )
        .expect_err("even a byte-identical empty winner must not be removed");
        assert!(error.to_string().contains("removal boundary"));
        assert!(namespace.join(backup_name).exists());
        assert!(displaced_empty.exists());

        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        let journal_name = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[0];
        write_private_control_fixture(&directory, journal_name, b"OBSERVED-JOURNAL");
        let observed = directory.bind_exact_file(OsStr::new(journal_name)).unwrap();
        let proof =
            windows_provenance_file_proof(&observed, MAX_KNOWLEDGE_PROVENANCE_JOURNAL_BYTES)
                .unwrap();
        drop(observed);
        let displaced = namespace.join("journal.displaced");
        let error = match reset_observed_windows_provenance_slot_with_hook(
            &directory,
            journal_name,
            &proof,
            || {
                fs::rename(namespace.join(journal_name), &displaced).unwrap();
                write_private_control_fixture(&directory, journal_name, b"HOSTILE-JOURNAL");
            },
        ) {
            Ok(_) => panic!("a journal-name winner must not be reset"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("exact-reset"));
        assert_eq!(
            fs::read(namespace.join(journal_name)).unwrap(),
            b"HOSTILE-JOURNAL"
        );
        assert_eq!(fs::read(displaced).unwrap(), b"OBSERVED-JOURNAL");
    }

    #[test]
    fn windows_provenance_equal_content_branch_preserves_a_swapped_temp_winner() {
        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        let bytes = b"EQUAL-CONTENT";
        write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_MANIFEST, bytes);
        write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_TEMP, bytes);
        let current = windows_current_provenance_proof(&directory)
            .unwrap()
            .unwrap();
        let temporary = directory
            .bind_exact_file(OsStr::new(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP))
            .unwrap();
        let intended =
            windows_provenance_file_proof(&temporary, MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES)
                .unwrap();
        drop(temporary);
        let displaced = namespace.join("equal-temp.displaced");

        let error = retire_equal_content_windows_provenance_temp_with_hook(
            &directory,
            Some(&current),
            &intended,
            bytes,
            || {
                fs::rename(
                    namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP),
                    &displaced,
                )
                .unwrap();
                write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_TEMP, bytes);
            },
        )
        .expect_err("an equal-byte replacement must not inherit retirement authority");

        assert!(error.to_string().contains("exact-zero"));
        assert_eq!(
            fs::read(namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP)).unwrap(),
            bytes
        );
        assert_eq!(fs::read(displaced).unwrap(), bytes);
    }

    #[test]
    fn windows_provenance_initializer_preserves_a_post_observation_backup_winner() {
        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        ensure_windows_provenance_journal_slots(&directory).unwrap();
        write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP, b"");
        let displaced = namespace.join("initializer-backup.displaced");

        let error = initialize_windows_provenance_journal_with_hook(&directory, |boundary| {
            if boundary == WindowsProvenanceInitializationBoundary::BackupRetirement {
                fs::rename(
                    namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP),
                    &displaced,
                )
                .unwrap();
                write_private_control_fixture(
                    &directory,
                    PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP,
                    b"BACKUP-WINNER",
                );
            }
        })
        .expect_err("a post-observation backup winner must fail initialization closed");

        assert!(error.to_string().contains("exact-zero"));
        assert_eq!(
            fs::read(namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP)).unwrap(),
            b"BACKUP-WINNER"
        );
        assert_eq!(fs::read(displaced).unwrap(), b"");
    }

    #[test]
    fn windows_provenance_initializer_writes_through_the_retained_baseline_capability() {
        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        ensure_windows_provenance_journal_slots(&directory).unwrap();
        let baseline_name = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[0];
        let displaced = namespace.join("initializer-baseline.displaced");

        let error = initialize_windows_provenance_journal_with_hook(&directory, |boundary| {
            if boundary == WindowsProvenanceInitializationBoundary::BaselineWrite {
                fs::rename(namespace.join(baseline_name), &displaced).unwrap();
                write_private_control_fixture(&directory, baseline_name, b"");
            }
        })
        .expect_err("a post-reset journal winner must not receive the baseline write");

        assert_eq!(fs::read(namespace.join(baseline_name)).unwrap(), b"");
        assert!(displaced.exists());
        assert!(!error.to_string().is_empty());
    }

    #[test]
    fn windows_provenance_initializer_preserves_a_swapped_journal_before_reset() {
        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        ensure_windows_provenance_journal_slots(&directory).unwrap();
        let journal_index = 1;
        let journal_name = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS[journal_index];
        let existing = directory
            .bind_owner_private_exact_file(OsStr::new(journal_name))
            .unwrap();
        let existing = existing.zero_exact_for_retirement().unwrap();
        let mut exact = existing.try_clone_exact_file().unwrap();
        exact.write_all(b"OBSERVED-JOURNAL").unwrap();
        exact.sync_all().unwrap();
        drop(exact);
        drop(existing);
        let displaced = namespace.join("initializer-journal.displaced");

        let error = initialize_windows_provenance_journal_with_hook(&directory, |boundary| {
            if boundary == WindowsProvenanceInitializationBoundary::JournalReset(journal_index) {
                fs::rename(namespace.join(journal_name), &displaced).unwrap();
                write_private_control_fixture(&directory, journal_name, b"JOURNAL-WINNER");
            }
        })
        .expect_err("a post-observation journal winner must not inherit reset authority");

        assert!(error.to_string().contains("exact-reset"));
        assert_eq!(
            fs::read(namespace.join(journal_name)).unwrap(),
            b"JOURNAL-WINNER"
        );
        assert_eq!(fs::read(displaced).unwrap(), b"OBSERVED-JOURNAL");
    }

    #[test]
    fn windows_provenance_initializer_retains_one_exact_baseline_capability() {
        let root = TempDir::new().unwrap();
        let namespace = root.path().join("private");
        let directory =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&namespace).unwrap();
        ensure_windows_provenance_journal_slots(&directory).unwrap();
        write_private_control_fixture(
            &directory,
            PRIVATE_KNOWLEDGE_PROVENANCE_TEMP,
            b"UNJOURNALED-TEMP",
        );
        write_private_control_fixture(&directory, PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP, b"");

        initialize_windows_provenance_journal_with_hook(&directory, |_| {}).unwrap();

        let reads = PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS
            .iter()
            .map(|name| read_windows_provenance_journal(&directory, name))
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(matches!(
            &reads[0],
            WindowsProvenanceJournalRead::Valid(record, _)
                if record.state == WindowsProvenanceJournalState::Baseline
                    && record.sequence == 0
        ));
        assert!(reads[1..]
            .iter()
            .all(|read| matches!(read, WindowsProvenanceJournalRead::Empty(_))));
        assert_eq!(
            fs::read(namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP)).unwrap(),
            b""
        );
        assert_eq!(
            fs::read(namespace.join(PRIVATE_KNOWLEDGE_PROVENANCE_BACKUP)).unwrap(),
            b""
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_preservation_objects_are_current_user_only_from_creation() {
        let workspace = TempDir::new().unwrap();
        let root = workspace.path().join("preserved");
        let directory = open_private_dir_no_follow(&root).unwrap();
        windows_private::verify_private_directory(&directory).unwrap();

        let file =
            create_new_private_file_at(&directory, &root, std::ffi::OsStr::new("private.bin"))
                .unwrap();
        windows_private::verify_private_file(&file).unwrap();
        assert!(file_identity_matches_path(&directory, &root));
    }

    #[cfg(windows)]
    #[test]
    fn windows_retained_root_and_namespace_block_redirect_before_private_write() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let source = config.knowledge.path.join("manual.md");
        fs::write(&source, b"WINDOWS-PRIVATE-PRESERVATION-CANARY").unwrap();
        let redirected_root = meetings.path().join("agent-visible-root-redirect");
        let redirected_namespace = meetings.path().join("agent-visible-namespace-redirect");

        preserve_file_before_retraction_with_hook(&config, &source, |namespace| {
            // Both the root and namespace descriptors deny delete-sharing, so
            // Windows must reject either name swap before CreateFileW runs.
            assert!(fs::rename(namespace.parent().unwrap(), &redirected_root).is_err());
            assert!(fs::rename(namespace, &redirected_namespace).is_err());
        })
        .unwrap();

        assert!(!redirected_root.exists());
        assert!(!redirected_namespace.exists());
        assert!(all_preserved_text(&config).contains("WINDOWS-PRIVATE-PRESERVATION-CANARY"));
    }

    #[test]
    #[cfg(unix)]
    fn private_directory_walk_is_bound_to_descriptors_during_component_swap() {
        use std::os::unix::fs::symlink;
        let root = TempDir::new().unwrap();
        let original = private_directory_platform_path(&root.path().join("a"));
        let moved = root.path().join("a-original");
        let outside = root.path().join("outside");
        fs::create_dir_all(&original).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = original.join("b");
        let mut swapped = false;
        open_private_dir_no_follow_with(&target, |traversed, _descriptor| {
            if !swapped && traversed == original {
                fs::rename(&original, &moved).unwrap();
                symlink(&outside, &original).unwrap();
                swapped = true;
            }
        })
        .unwrap();
        assert!(moved.join("b").is_dir());
        assert!(!outside.join("b").exists());
    }

    // Windows retains a non-delete-sharing preservation capability, so the
    // rename used to create this POSIX race is refused before the boundary.
    // The adjacent Windows retained-root/namespace regression covers that
    // stronger native outcome.
    #[cfg(unix)]
    #[test]
    fn source_swap_after_backup_is_not_overwritten_or_unlinked() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("manual.md");
        fs::write(&path, b"ORIGINAL-USER-BYTES").unwrap();
        let identity = preserve_file_before_retraction(&config, &path).unwrap();
        let displaced = config.knowledge.path.join("original-displaced.md");
        fs::rename(&path, &displaced).unwrap();
        fs::write(&path, b"NEW-SWAPPED-USER-BYTES").unwrap();

        let error = remove_preserved_file(&config, &path, &identity).unwrap_err();
        assert!(error.to_string().contains("changed after preservation"));
        assert_eq!(fs::read(&path).unwrap(), b"NEW-SWAPPED-USER-BYTES");
        assert_eq!(fs::read(&displaced).unwrap(), b"ORIGINAL-USER-BYTES");
    }

    #[test]
    fn oversized_knowledge_source_fails_before_preservation_or_mutation() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("oversized.md");
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)
            .unwrap();
        file.set_len(MAX_RETAINED_RECONCILIATION_BYTES + 1).unwrap();

        let error = preserve_file_before_retraction(&config, &path)
            .expect_err("an oversized source must fail before allocating or moving it");

        assert!(error.to_string().contains("bounded preservation size"));
        assert_eq!(
            fs::metadata(&path).unwrap().len(),
            MAX_RETAINED_RECONCILIATION_BYTES + 1
        );
        assert!(!config
            .knowledge
            .path
            .join(".minutes-private-reconcile")
            .exists());
    }

    #[test]
    fn replacement_rewrite_after_publication_retains_exact_old_capture() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("rewrite-boundary.md");
        let original = b"EXACT-OLD-CAPTURE";
        let intended = b"INTENDED-REWRITE";
        let rewritten = b"ATTACKER-REWRITE";
        assert_eq!(intended.len(), rewritten.len());
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error = replace_preserved_file_with_hook(
            &config,
            &path,
            &identity,
            Some(intended),
            |capture_path| {
                capture = Some(capture_path.to_path_buf());
                fs::write(&path, rewritten).unwrap();
            },
        )
        .expect_err("a same-inode replacement rewrite must fail before old retirement");

        assert!(error.to_string().contains("publication content changed"));
        assert_eq!(fs::read(&path).unwrap(), rewritten);
        assert_eq!(fs::read(capture.unwrap()).unwrap(), original);
    }

    #[test]
    fn replacement_late_rewrite_after_initial_successor_proof_preserves_exact_old() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("late-successor.md");
        fs::write(&path, b"EXACT-OLD").unwrap();
        let identity = preserved_source_identity(&path).unwrap();

        let error = replace_preserved_file_with_hooks(
            &config,
            &path,
            &identity,
            Some(b"INTENDED-NEW"),
            |_| {},
            |successor| fs::write(successor, b"LATE-REWRITE").unwrap(),
        )
        .expect_err("a successor rewritten after its initial proof must fail closed");

        assert!(error.to_string().contains("publication content changed"));
        assert_eq!(fs::read(&path).unwrap(), b"LATE-REWRITE");
        let captures = path.parent().unwrap().join(".minutes-private-reconcile");
        assert!(fs::read_dir(captures).unwrap().flatten().any(|entry| {
            fs::read(entry.path()).ok().as_deref() == Some(b"EXACT-OLD".as_slice())
        }));
    }

    #[test]
    fn deletion_late_winner_after_initial_absence_proof_preserves_exact_old() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("late-delete.md");
        fs::write(&path, b"EXACT-OLD").unwrap();
        let identity = preserved_source_identity(&path).unwrap();

        let error = replace_preserved_file_with_hooks(
            &config,
            &path,
            &identity,
            None,
            |_| {},
            |successor| fs::write(successor, b"LATE-WINNER").unwrap(),
        )
        .expect_err("a deletion winner after the initial absence proof must fail closed");

        assert!(error.to_string().contains("repopulated"));
        assert_eq!(fs::read(&path).unwrap(), b"LATE-WINNER");
        let captures = path.parent().unwrap().join(".minutes-private-reconcile");
        assert!(fs::read_dir(captures).unwrap().flatten().any(|entry| {
            fs::read(entry.path()).ok().as_deref() == Some(b"EXACT-OLD".as_slice())
        }));
    }

    #[test]
    fn replacement_hardlink_after_publication_retains_exact_old_capture() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("hardlink-boundary.md");
        let alias = config.knowledge.path.join("hardlink-boundary-alias.md");
        let original = b"EXACT-OLD-CAPTURE";
        let intended = b"INTENDED-REWRITE";
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error = replace_preserved_file_with_hook(
            &config,
            &path,
            &identity,
            Some(intended),
            |capture_path| {
                capture = Some(capture_path.to_path_buf());
                fs::hard_link(&path, &alias).unwrap();
            },
        )
        .expect_err("a linked replacement must fail before old retirement");

        assert!(error.to_string().contains("name or links changed"));
        assert_eq!(fs::read(&path).unwrap(), intended);
        assert_eq!(fs::read(&alias).unwrap(), intended);
        assert_eq!(fs::read(capture.unwrap()).unwrap(), original);
    }

    #[test]
    fn replacement_name_winner_after_publication_retains_exact_old_capture() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("winner-boundary.md");
        let displaced = config.knowledge.path.join("winner-boundary-displaced.md");
        let original = b"EXACT-OLD-CAPTURE";
        let intended = b"INTENDED-REWRITE";
        let winner = b"REPLACEMENT-WINNER";
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error = replace_preserved_file_with_hook(
            &config,
            &path,
            &identity,
            Some(intended),
            |capture_path| {
                capture = Some(capture_path.to_path_buf());
                fs::rename(&path, &displaced).unwrap();
                fs::write(&path, winner).unwrap();
            },
        )
        .expect_err("a replacement-name winner must fail before old retirement");

        assert!(error.to_string().contains("name or links changed"));
        assert_eq!(fs::read(&path).unwrap(), winner);
        assert_eq!(fs::read(&displaced).unwrap(), intended);
        assert_eq!(fs::read(capture.unwrap()).unwrap(), original);
    }

    #[test]
    fn deletion_name_winner_retains_exact_old_capture() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("delete-winner.md");
        let original = b"EXACT-OLD-CAPTURE";
        let winner = b"DELETE-NAME-WINNER";
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error =
            replace_preserved_file_with_hook(&config, &path, &identity, None, |capture_path| {
                capture = Some(capture_path.to_path_buf());
                fs::write(&path, winner).unwrap();
            })
            .expect_err("a deletion-name winner must fail before old retirement");

        assert!(error.to_string().contains("repopulated"));
        assert_eq!(fs::read(&path).unwrap(), winner);
        assert_eq!(fs::read(capture.unwrap()).unwrap(), original);
    }

    #[test]
    fn final_capture_slot_replacement_is_retained_not_unlinked() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("manual.md");
        fs::write(&path, b"BOUND-ORIGINAL").unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let replacement = b"PUBLISHED-REWRITE";
        let capture_winner = b"FINAL-CAPTURE-SLOT-WINNER";
        let mut capture_path = None;
        let mut displaced_capture = None;

        let error = replace_preserved_file_with_hook(
            &config,
            &path,
            &identity,
            Some(replacement),
            |capture| {
                let displaced = capture.with_extension("bound-original");
                fs::rename(capture, &displaced).unwrap();
                fs::write(capture, capture_winner).unwrap();
                capture_path = Some(capture.to_path_buf());
                displaced_capture = Some(displaced);
            },
        )
        .expect_err("a detached exact capture must be retained rather than sanitized");

        assert!(error.to_string().contains("capture"));
        assert_eq!(fs::read(&path).unwrap(), replacement);
        assert_eq!(fs::read(capture_path.unwrap()).unwrap(), capture_winner);
        assert_eq!(
            fs::read(displaced_capture.unwrap()).unwrap(),
            b"BOUND-ORIGINAL"
        );
    }

    #[test]
    fn final_capture_retirement_preserves_same_inode_rewrite_bytes() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-rewrite");
        let original = b"BOUND-CAPTURE-ORIGINAL";
        let rewritten = b"SAME-INODE-USER-REWRITE";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();
        let mut retained_writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&capture)
            .unwrap();

        let error = attest_retained_capture_with_hook(
            &capture,
            capture_file.as_mut(),
            &identity,
            move |_| {
                retained_writer.set_len(0)?;
                retained_writer.seek(SeekFrom::Start(0))?;
                retained_writer.write_all(rewritten)?;
                retained_writer.sync_all()
            },
        )
        .expect_err("a same-inode rewrite after the first proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert_eq!(fs::read(&capture).unwrap(), rewritten);
    }

    #[test]
    fn final_capture_retirement_preserves_deleted_retained_writer_bytes() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-delete");
        let original = b"BOUND-CAPTURE-ORIGINAL";
        let rewritten = b"DELETED-NAME-USER-REWRITE";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();
        let mut retained_writer = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&capture)
            .unwrap();
        let mut retained_reader = retained_writer.try_clone().unwrap();

        let error = attest_retained_capture_with_hook(
            &capture,
            capture_file.as_mut(),
            &identity,
            move |path| {
                retained_writer.set_len(0)?;
                retained_writer.seek(SeekFrom::Start(0))?;
                retained_writer.write_all(rewritten)?;
                retained_writer.sync_all()?;
                fs::remove_file(path)
            },
        )
        .expect_err("a deleted capture name after the first proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert!(!capture.exists());
        retained_reader.seek(SeekFrom::Start(0)).unwrap();
        let mut retained_bytes = Vec::new();
        retained_reader.read_to_end(&mut retained_bytes).unwrap();
        assert_eq!(retained_bytes, rewritten);
    }

    #[test]
    fn final_capture_retirement_preserves_same_inode_rewrite_after_last_content_proof() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-final-rewrite");
        let original = b"FINAL-CONTENT-PROOF-ORIGINAL";
        let rewritten = vec![b'R'; original.len()];
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        let error = attest_retained_capture_with_hooks(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |path| fs::write(path, &rewritten),
        )
        .expect_err("a same-inode rewrite after the content proof must block truncation");

        assert!(error.to_string().contains("content boundary"));
        assert_eq!(fs::read(&capture).unwrap(), rewritten);
    }

    #[test]
    fn final_capture_retirement_reopens_name_after_last_content_proof() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-final-name");
        let alias = root.path().join("capture-final-name-alias");
        let original = b"FINAL-CONTENT-PROOF-ORIGINAL";
        let winner = b"FINAL-CAPTURE-NAME-WINNER";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        let error = attest_retained_capture_with_hooks(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |path| {
                fs::rename(path, &alias)?;
                fs::write(path, winner)
            },
        )
        .expect_err("a name replacement after the content proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert_eq!(fs::read(&alias).unwrap(), original);
        assert_eq!(fs::read(&capture).unwrap(), winner);
    }

    #[test]
    fn final_capture_retirement_rechecks_links_after_last_content_proof() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-final-link");
        let alias = root.path().join("capture-final-link-alias");
        let original = b"FINAL-LINK-PROOF-ORIGINAL";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        let error = attest_retained_capture_with_hooks(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |path| fs::hard_link(path, &alias),
        )
        .expect_err("a hard link after the content proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert_eq!(fs::read(&capture).unwrap(), original);
        assert_eq!(fs::read(&alias).unwrap(), original);
    }

    #[test]
    fn final_capture_retirement_rechecks_content_after_successor_hook() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-successor-rewrite");
        let original = b"SUCCESSOR-BOUNDARY-ORIGINAL";
        let rewritten = b"SUCCESSOR-BOUNDARY-REWRITTEN";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        let error = attest_retained_capture_with_successor_proof(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |_| Ok(()),
            || {
                fs::write(&capture, rewritten)?;
                Ok(())
            },
        )
        .expect_err("a rewrite inside successor proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert_eq!(fs::read(&capture).unwrap(), rewritten);
    }

    #[test]
    fn final_capture_retirement_rechecks_links_after_successor_hook() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-successor-link");
        let alias = root.path().join("capture-successor-link-alias");
        let original = b"SUCCESSOR-LINK-ORIGINAL";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        let error = attest_retained_capture_with_successor_proof(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |_| Ok(()),
            || {
                fs::hard_link(&capture, &alias)?;
                Ok(())
            },
        )
        .expect_err("a hard link inside successor proof must block truncation");

        assert!(error.to_string().contains("retirement boundary"));
        assert_eq!(fs::read(&capture).unwrap(), original);
        assert_eq!(fs::read(&alias).unwrap(), original);
    }

    #[test]
    fn successful_capture_retirement_retains_exact_old_bytes() {
        let root = TempDir::new().unwrap();
        let capture = root.path().join("capture-retained-old");
        let original = b"RETAINED-OLD-GENERATION";
        fs::write(&capture, original).unwrap();
        let identity = preserved_source_identity(&capture).unwrap();
        let mut capture_file = open_exact_capture_for_retirement(&capture, &identity).unwrap();

        attest_retained_capture_with_successor_proof(
            &capture,
            capture_file.as_mut(),
            &identity,
            |_| Ok(()),
            |_| Ok(()),
            || Ok(()),
        )
        .unwrap();

        assert_eq!(fs::read(&capture).unwrap(), original);
    }

    #[test]
    fn zero_tombstone_exemption_reopens_the_final_visible_name() {
        let root = TempDir::new().unwrap();
        let tombstone = root.path().join("capture-zero");
        let displaced = root.path().join("capture-zero-displaced");
        let winner = b"BYTE-BEARING-TOMBSTONE-WINNER";
        fs::write(&tombstone, b"").unwrap();

        let result = exact_reconciliation_zero_tombstone_with_hook(&tombstone, |path| {
            fs::rename(path, &displaced)?;
            fs::write(path, winner)
        });

        assert!(result.is_none());
        assert_eq!(fs::read(&displaced).unwrap(), b"");
        assert_eq!(fs::read(&tombstone).unwrap(), winner);
    }

    #[test]
    fn reconciliation_rewrite_preserves_late_hardlink_alias_bytes() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("rewrite-hardlink.md");
        let alias = config.knowledge.path.join("rewrite-hardlink-alias.md");
        let original = b"REWRITE-LATE-HARDLINK-ORIGINAL";
        let replacement = b"REWRITE-LATE-HARDLINK-PUBLISHED";
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error = replace_preserved_file_with_hook(
            &config,
            &path,
            &identity,
            Some(replacement),
            |capture_path| {
                fs::hard_link(capture_path, &alias).unwrap();
                fs::remove_file(capture_path).unwrap();
                capture = Some(capture_path.to_path_buf());
            },
        )
        .expect_err("a late hard-link must block exact capture truncation");

        assert!(error.to_string().contains("capture"));
        assert_eq!(fs::read(&path).unwrap(), replacement);
        assert!(!capture.unwrap().exists());
        assert_eq!(fs::read(&alias).unwrap(), original);
    }

    #[test]
    fn reconciliation_delete_preserves_late_hardlink_alias_bytes() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let path = config.knowledge.path.join("delete-hardlink.md");
        let alias = config.knowledge.path.join("delete-hardlink-alias.md");
        let original = b"DELETE-LATE-HARDLINK-ORIGINAL";
        fs::write(&path, original).unwrap();
        let identity = preserved_source_identity(&path).unwrap();
        let mut capture = None;

        let error =
            replace_preserved_file_with_hook(&config, &path, &identity, None, |capture_path| {
                fs::hard_link(capture_path, &alias).unwrap();
                fs::remove_file(capture_path).unwrap();
                capture = Some(capture_path.to_path_buf());
            })
            .expect_err("a late hard-link must block exact capture truncation");

        assert!(error.to_string().contains("capture"));
        assert!(!path.exists());
        assert!(!capture.unwrap().exists());
        assert_eq!(fs::read(&alias).unwrap(), original);
    }

    #[test]
    #[cfg(unix)]
    fn capture_budget_counts_complete_namespace_and_only_exempts_exact_zero_files() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let namespace = |name: &str| {
            let directory = root.path().join(name);
            fs::create_dir(&directory).unwrap();
            directory
        };

        let inert = namespace("inert");
        fs::write(inert.join("renamed-exact-zero"), b"").unwrap();
        require_retained_slot_budget(&inert, "capture-", 0, 1, 1).unwrap();

        let renamed = namespace("renamed-nonzero");
        fs::write(renamed.join("unrelated-name"), b"RENAMED-NONZERO").unwrap();
        assert!(require_retained_slot_budget(&renamed, "capture-", 0, 1, u64::MAX).is_err());
        assert!(require_retained_slot_budget(&renamed, "capture-", 0, usize::MAX, 1).is_err());

        let linked = namespace("hardlinked-zero");
        let linked_source = linked.join("zero-source");
        fs::write(&linked_source, b"").unwrap();
        fs::hard_link(&linked_source, linked.join("zero-alias")).unwrap();
        assert!(require_retained_slot_budget(&linked, "capture-", 0, 1, u64::MAX).is_err());

        let directory = namespace("directory");
        let descendant_root = directory.join("non-prefix-directory");
        fs::create_dir(&descendant_root).unwrap();
        fs::write(
            descendant_root.join("oversized-private-descendant"),
            vec![b'X'; 4096],
        )
        .unwrap();
        assert!(require_retained_slot_budget(&directory, "capture-", 0, 1, u64::MAX).is_err());
        assert!(
            require_retained_slot_budget(&directory, "capture-", 0, usize::MAX, 1).is_err(),
            "a directory must exhaust the byte budget without recursing into descendants"
        );

        let linked_entry = namespace("symlink");
        symlink("target", linked_entry.join("non-prefix-symlink")).unwrap();
        assert!(require_retained_slot_budget(&linked_entry, "capture-", 0, 1, u64::MAX).is_err());

        let special = namespace("special");
        let fifo = special.join("non-prefix-fifo");
        let fifo_name = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_name.as_ptr(), 0o600) }, 0);
        assert!(require_retained_slot_budget(&special, "capture-", 0, 1, u64::MAX).is_err());
    }

    #[test]
    fn byte_bearing_captures_fail_closed_at_the_sequential_rewrite_bound() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let people = config.knowledge.path.join("people");
        fs::create_dir_all(&people).unwrap();
        let profile = people.join("repeated.md");
        fs::write(&profile, b"INITIAL").unwrap();

        for revision in 0..MAX_RETAINED_RECONCILIATION_CAPTURES {
            let identity = preserved_source_identity(&profile).unwrap();
            let replacement = format!("REVISION-{revision}");
            replace_preserved_file(&config, &profile, &identity, Some(replacement.as_bytes()))
                .unwrap();
        }

        let last = format!("REVISION-{}", MAX_RETAINED_RECONCILIATION_CAPTURES - 1);
        assert_eq!(fs::read_to_string(&profile).unwrap(), last);
        let identity = preserved_source_identity(&profile).unwrap();
        let error = replace_preserved_file(&config, &profile, &identity, Some(b"OVER-BOUND"))
            .expect_err("retained old bytes must exhaust the hard capture bound");
        assert!(error.to_string().contains("safety budget is exhausted"));
        let staging = people.join(".minutes-private-reconcile");
        let captures = fs::read_dir(&staging)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry.file_name().to_string_lossy().starts_with("capture-")
                    && entry.metadata().is_ok_and(|metadata| metadata.len() > 0)
            })
            .count();
        assert_eq!(captures, MAX_RETAINED_RECONCILIATION_CAPTURES);
    }

    #[test]
    fn byte_bearing_captures_bound_large_profile_retraction_and_followup() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let people = config.knowledge.path.join("people");
        fs::create_dir_all(&people).unwrap();
        let profile_count = MAX_RETAINED_RECONCILIATION_CAPTURES + 8;
        for index in 0..profile_count {
            fs::write(
                people.join(format!("person-{index}.md")),
                format!(
                    "# Person {index}\n\n## Context\n\n- FACT-{index} *(strong; 2026-06-10 — source-{index})*\n"
                ),
            )
            .unwrap();
        }

        let error = rewrite_wiki_people(&config, |_, _| RecordDisposition::RemoveOwned)
            .expect_err("large retirement must stop at the retained capture bound");
        assert!(error.to_string().contains("safety budget is exhausted"));
        assert!(fs::read_dir(&people)
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.path().extension().is_some_and(|ext| ext == "md")));

        let followup = people.join("policy-denied-followup.md");
        fs::write(
            &followup,
            "# Followup\n\n## Context\n\n- DENIED *(strong; 2026-06-10 — denied-source)*\n",
        )
        .unwrap();
        let error = rewrite_wiki_people(&config, |_, _| RecordDisposition::RemoveOwned)
            .expect_err("follow-up retirement must remain fail-closed at the bound");
        assert!(error.to_string().contains("safety budget is exhausted"));
        assert!(followup.exists());
    }

    #[test]
    fn inert_capture_tombstones_are_ignored_by_all_adapter_and_corpus_walkers() {
        for adapter in ["wiki", "obsidian", "para"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let mut config = knowledge_config(&meetings, &knowledge, adapter);
            if adapter == "para" {
                let directory = config.knowledge.path.join("areas/people/retired");
                fs::create_dir_all(&directory).unwrap();
                let items = directory.join("items.json");
                let summary = directory.join("summary.md");
                fs::write(
                    &items,
                    serde_json::to_vec_pretty(&serde_json::json!([
                        {"id": "retired", "fact": "RETIRED", "source": "retired-source", "status": "active"}
                    ]))
                    .unwrap(),
                )
                .unwrap();
                let parsed =
                    serde_json::from_slice::<Vec<serde_json::Value>>(&fs::read(&items).unwrap())
                        .unwrap();
                fs::write(
                    &summary,
                    render_para_summary("Retired", parsed.iter()).unwrap(),
                )
                .unwrap();
                rewrite_para_people(&config, |_, _| RecordDisposition::RemoveOwned).unwrap();
            } else {
                let people = config.knowledge.path.join("people");
                fs::create_dir_all(&people).unwrap();
                fs::write(
                    people.join("retired.md"),
                    "# Retired\n\n## Context\n\n- RETIRED *(strong; 2026-06-10 — retired-source)*\n",
                )
                .unwrap();
                rewrite_wiki_people(&config, |_, _| RecordDisposition::RemoveOwned).unwrap();
            }

            config.output_dir = config.knowledge.path.clone();
            assert!(live_corpus_markdown_paths(&config).iter().all(|path| !path
                .components()
                .any(|component| component.as_os_str() == ".minutes-private-reconcile")));
            assert_eq!(count_knowledge_snapshot_locked(&config).unwrap().0, 0);
        }
    }

    #[test]
    fn byte_bearing_capture_winners_still_exhaust_the_fail_closed_budget() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let people = config.knowledge.path.join("people");
        fs::create_dir_all(&people).unwrap();

        // Each adversarial final-slot swap retains two byte-bearing entries:
        // the displaced exact capture and the winner installed at its former
        // name. Both count in the complete private namespace, and the
        // operation now correctly fails instead of sanitizing either one.
        for index in 0..(MAX_RETAINED_RECONCILIATION_CAPTURES / 2) {
            let profile = people.join(format!("winner-{index}.md"));
            fs::write(&profile, b"ORIGINAL").unwrap();
            let identity = preserved_source_identity(&profile).unwrap();
            let error = replace_preserved_file_with_hook(
                &config,
                &profile,
                &identity,
                Some(b"REWRITTEN"),
                |capture| {
                    let displaced = capture.with_extension("sanitized-original");
                    fs::rename(capture, displaced).unwrap();
                    fs::write(capture, b"BYTE-BEARING-FINAL-WINNER").unwrap();
                },
            )
            .expect_err("a displaced exact capture must fail closed");
            assert!(error.to_string().contains("capture"));
            assert_eq!(fs::read(&profile).unwrap(), b"REWRITTEN");
        }

        let blocked = people.join("blocked.md");
        fs::write(&blocked, b"MUST-REMAIN").unwrap();
        let identity = preserved_source_identity(&blocked).unwrap();
        let error = replace_preserved_file(&config, &blocked, &identity, Some(b"NOPE"))
            .expect_err("byte-bearing winners must retain the fail-closed entry budget");
        assert!(error.to_string().contains("safety budget is exhausted"));
        assert_eq!(fs::read(&blocked).unwrap(), b"MUST-REMAIN");
    }

    #[test]
    fn final_publication_temp_replacement_is_retained_not_unlinked() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let intended = b"INTENDED-SUMMARY";
        let temp_winner = b"FINAL-TEMP-SLOT-WINNER";
        let mut temporary_path = None;
        let mut displaced_temporary = None;

        let error =
            publish_private_file_no_replace_with_hook(&destination, intended, |temporary| {
                let displaced = temporary.with_extension("bound-original");
                fs::rename(temporary, &displaced).unwrap();
                fs::write(temporary, temp_winner).unwrap();
                temporary_path = Some(temporary.to_path_buf());
                displaced_temporary = Some(displaced);
            })
            .unwrap_err();

        assert!(error
            .to_string()
            .contains("knowledge publication proof failed"));
        assert!(!destination.exists());
        assert_eq!(fs::read(temporary_path.unwrap()).unwrap(), temp_winner);
        assert_eq!(fs::read(displaced_temporary.unwrap()).unwrap(), intended);
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-publish-")
                    && fs::read(entry.path()).ok().as_deref() == Some(intended.as_slice())
            }));
    }

    #[test]
    fn private_publication_same_inode_rewrite_fails_and_retains_intended_bytes() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let intended = b"INTENDED-SUMMARY";
        let rewritten = b"ATTACKER-SUMMARY";
        assert_eq!(intended.len(), rewritten.len());

        let error =
            publish_private_file_no_replace_with_hook(&destination, intended, |temporary| {
                fs::write(temporary, rewritten).unwrap()
            })
            .expect_err("a same-inode temporary rewrite must fail closed");

        assert!(error.to_string().contains("publication content changed"));
        assert!(!destination.exists());
        let retained = fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .filter_map(|entry| fs::read(entry.path()).ok())
            .collect::<Vec<_>>();
        assert!(retained.iter().any(|bytes| bytes == rewritten));
        assert!(retained.iter().any(|bytes| bytes == intended));
    }

    #[test]
    fn private_publication_hardlink_fails_and_retains_intended_bytes() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let alias = directory.path().join("summary-alias.md");
        let intended = b"INTENDED-SUMMARY";

        let error =
            publish_private_file_no_replace_with_hook(&destination, intended, |temporary| {
                fs::hard_link(temporary, &alias).unwrap()
            })
            .expect_err("a hard-linked temporary must fail closed");

        assert!(error.to_string().contains("name or links changed"));
        assert!(!destination.exists());
        assert_eq!(fs::read(&alias).unwrap(), intended);
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-publish-")
                    && fs::read(entry.path()).ok().as_deref() == Some(intended.as_slice())
            }));
    }

    #[test]
    fn private_publication_post_move_rewrite_is_claimed_out_of_public_name() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let intended = b"INTENDED-SUMMARY";
        let rewritten = b"ATTACKER-SUMMARY";
        assert_eq!(intended.len(), rewritten.len());

        let error = publish_private_file_no_replace_with_hooks(
            &destination,
            intended,
            |_| {},
            |published| fs::write(published, rewritten).unwrap(),
        )
        .expect_err("a post-move rewrite must be claimed out of the public name");

        assert!(error
            .to_string()
            .contains("suspect public bytes were claimed"));
        assert!(!destination.exists());
        let retained = fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .filter_map(|entry| fs::read(entry.path()).ok())
            .collect::<Vec<_>>();
        assert!(retained.iter().any(|bytes| bytes == rewritten));
        assert!(retained.iter().any(|bytes| bytes == intended));
    }

    #[test]
    fn private_publication_post_move_hardlink_preserves_alias_and_claims_name() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let alias = directory.path().join("summary-alias.md");
        let intended = b"INTENDED-SUMMARY";

        let error = publish_private_file_no_replace_with_hooks(
            &destination,
            intended,
            |_| {},
            |published| fs::hard_link(published, &alias).unwrap(),
        )
        .expect_err("a post-move hardlink must fail closed");

        assert!(error
            .to_string()
            .contains("suspect public bytes were claimed"));
        assert!(!destination.exists());
        assert_eq!(fs::read(&alias).unwrap(), intended);
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-publish-")
                    && fs::read(entry.path()).ok().as_deref() == Some(intended.as_slice())
            }));
    }

    #[test]
    fn private_publication_post_move_name_winner_is_preserved() {
        let directory = TempDir::new().unwrap();
        let destination = directory.path().join("summary.md");
        let displaced = directory.path().join("displaced-summary.md");
        let intended = b"INTENDED-SUMMARY";
        let winner = b"PATHNAME-WINNER";

        let error = publish_private_file_no_replace_with_hooks(
            &destination,
            intended,
            |_| {},
            |published| {
                fs::rename(published, &displaced).unwrap();
                fs::write(published, winner).unwrap();
            },
        )
        .expect_err("a post-move pathname winner must fail closed");

        assert!(error.to_string().contains("pathname winner was preserved"));
        assert_eq!(fs::read(&destination).unwrap(), winner);
        assert_eq!(fs::read(&displaced).unwrap(), intended);
        assert!(fs::read_dir(directory.path())
            .unwrap()
            .flatten()
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-publish-")
                    && fs::read(entry.path()).ok().as_deref() == Some(intended.as_slice())
            }));
    }

    #[test]
    fn qmd_inventory_rejects_entry_overflow_while_retaining_only_the_bound() {
        let root = TempDir::new().unwrap();
        for index in 0..=MAX_QMD_RETIREMENT_DESCENDANTS {
            fs::write(root.path().join(format!("entry-{index:04}")), []).unwrap();
        }
        let bound = qmd_open_directory_no_follow(root.path()).unwrap();

        let error = qmd_build_plaintext_inventory(&bound, Instant::now() + Duration::from_secs(5))
            .expect_err("inventory must stop at its entry bound");

        assert!(error.contains("entry budget"));
        assert_eq!(
            fs::read_dir(root.path()).unwrap().count(),
            MAX_QMD_RETIREMENT_DESCENDANTS + 1
        );
    }

    // Windows' retained exact handles prevent this same-path rename race from
    // being constructed. Windows provenance action-boundary tests exercise its
    // stronger handle-based publication contract instead.
    #[cfg(unix)]
    #[test]
    fn reconciliation_rewrite_preserves_same_path_replacement_at_final_boundary() {
        for adapter in ["wiki", "para"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let config = knowledge_config(&meetings, &knowledge, adapter);
            let (target, original) = if adapter == "para" {
                let directory = config.knowledge.path.join("areas/people/alex-kim");
                fs::create_dir_all(&directory).unwrap();
                let target = directory.join("items.json");
                let items = serde_json::json!([
                    {"id": "first", "fact": "FIRST", "source": "source-a", "status": "active"},
                    {"id": "second", "fact": "SECOND", "source": "source-b", "status": "active"}
                ]);
                let original = serde_json::to_vec_pretty(&items).unwrap();
                fs::write(&target, &original).unwrap();
                (target, original)
            } else {
                let directory = config.knowledge.path.join("people");
                fs::create_dir_all(&directory).unwrap();
                let target = directory.join("alex-kim.md");
                let original = b"# Alex Kim\n\n## Commitment\n\n- FIRST *(strong; 2026-06-10 -- source-a)*\n- SECOND *(strong; 2026-06-10 -- source-b)*\n".to_vec();
                // Use the exact generated separator expected by the parser.
                let original = String::from_utf8(original)
                    .unwrap()
                    .replace(" -- ", " — ")
                    .into_bytes();
                fs::write(&target, &original).unwrap();
                (target, original)
            };
            let displaced = target.with_extension("parsed-original");
            let replacement = format!("FINAL-REWRITE-REPLACEMENT-{adapter}").into_bytes();
            let mut classifications = 0usize;
            let mut swapped = false;
            let result = if adapter == "para" {
                rewrite_para_people_with_hook(
                    &config,
                    |_, _| {
                        classifications += 1;
                        if classifications == 1 {
                            RecordDisposition::RemoveOwned
                        } else {
                            RecordDisposition::Keep
                        }
                    },
                    |path| {
                        if path == target && !swapped {
                            fs::rename(&target, &displaced).unwrap();
                            fs::write(&target, &replacement).unwrap();
                            swapped = true;
                        }
                    },
                )
            } else {
                rewrite_wiki_people_with_hook(
                    &config,
                    |_, _| {
                        classifications += 1;
                        if classifications == 1 {
                            RecordDisposition::RemoveOwned
                        } else {
                            RecordDisposition::Keep
                        }
                    },
                    |path| {
                        if path == target && !swapped {
                            fs::rename(&target, &displaced).unwrap();
                            fs::write(&target, &replacement).unwrap();
                            swapped = true;
                        }
                    },
                )
            };
            assert!(result.is_err(), "adapter={adapter}");
            assert!(swapped, "adapter={adapter}");
            assert_eq!(fs::read(&target).unwrap(), replacement, "adapter={adapter}");
            assert_eq!(fs::read(&displaced).unwrap(), original, "adapter={adapter}");
        }
    }

    // See the rewrite variant above for why this adversarial rename is Unix
    // only and which Windows exact-handle regressions cover the native path.
    #[cfg(unix)]
    #[test]
    fn reconciliation_delete_preserves_same_path_replacement_at_final_boundary() {
        for adapter in ["wiki", "para"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let config = knowledge_config(&meetings, &knowledge, adapter);
            let (target, original) = if adapter == "para" {
                let directory = config.knowledge.path.join("areas/people/alex-kim");
                fs::create_dir_all(&directory).unwrap();
                let target = directory.join("items.json");
                let original = serde_json::to_vec_pretty(&serde_json::json!([
                    {"id": "only", "fact": "ONLY", "source": "source-a", "status": "active"}
                ]))
                .unwrap();
                fs::write(&target, &original).unwrap();
                (target, original)
            } else {
                let directory = config.knowledge.path.join("people");
                fs::create_dir_all(&directory).unwrap();
                let target = directory.join("alex-kim.md");
                let original =
                    "# Alex Kim\n\n## Commitment\n\n- ONLY *(strong; 2026-06-10 — source-a)*\n"
                        .as_bytes()
                        .to_vec();
                fs::write(&target, &original).unwrap();
                (target, original)
            };
            let displaced = target.with_extension("parsed-original");
            let replacement = format!("FINAL-DELETE-REPLACEMENT-{adapter}").into_bytes();
            let mut swapped = false;
            let result = if adapter == "para" {
                rewrite_para_people_with_hook(
                    &config,
                    |_, _| RecordDisposition::RemoveOwned,
                    |path| {
                        if path == target && !swapped {
                            fs::rename(&target, &displaced).unwrap();
                            fs::write(&target, &replacement).unwrap();
                            swapped = true;
                        }
                    },
                )
            } else {
                rewrite_wiki_people_with_hook(
                    &config,
                    |_, _| RecordDisposition::RemoveOwned,
                    |path| {
                        if path == target && !swapped {
                            fs::rename(&target, &displaced).unwrap();
                            fs::write(&target, &replacement).unwrap();
                            swapped = true;
                        }
                    },
                )
            };
            assert!(result.is_err(), "adapter={adapter}");
            assert!(swapped, "adapter={adapter}");
            assert_eq!(fs::read(&target).unwrap(), replacement, "adapter={adapter}");
            assert_eq!(fs::read(&displaced).unwrap(), original, "adapter={adapter}");
        }
    }

    #[test]
    fn para_summary_publication_never_replaces_a_final_boundary_winner() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        let directory = config.knowledge.path.join("areas/people/alex-kim");
        fs::create_dir_all(&directory).unwrap();
        let items = directory.join("items.json");
        let summary = directory.join("summary.md");
        fs::write(
            &items,
            serde_json::to_vec_pretty(&serde_json::json!([
                {"id": "kept", "fact": "KEPT", "source": "source-a", "status": "active"}
            ]))
            .unwrap(),
        )
        .unwrap();
        let replacement = b"FINAL-SUMMARY-PUBLICATION-WINNER";
        let mut installed = false;
        let result = rewrite_para_people_with_hook(
            &config,
            |_, _| RecordDisposition::Keep,
            |path| {
                if path == summary && !installed {
                    fs::write(&summary, replacement).unwrap();
                    installed = true;
                }
            },
        );
        assert!(result.is_err());
        assert!(installed);
        assert_eq!(fs::read(&summary).unwrap(), replacement);
        let retained_generations: Vec<PathBuf> = fs::read_dir(para_private_root(&config).unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PARA_PERSON_STAGE_PREFIX)
            })
            .map(|entry| entry.path())
            .collect();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert!(retained_generations.is_empty());
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            assert_eq!(retained_generations.len(), 1);
            assert!(all_text_unfiltered(&retained_generations[0]).contains("KEPT"));
            assert!(!all_text_unfiltered(&retained_generations[0])
                .contains("FINAL-SUMMARY-PUBLICATION-WINNER"));
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_revocation_failed_summary_proof_never_exposes_a_mixed_generation() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        let people = config.knowledge.path.join("areas/people");
        let private_root = para_private_root(&config).unwrap();
        assert!(!private_root.starts_with(&config.knowledge.path));
        let directory = people.join("alex-kim");
        fs::create_dir_all(&directory).unwrap();
        let items = vec![
            serde_json::json!({"id":"revoked","fact":"REVOKED-CANARY","source":"source-a","status":"active"}),
            serde_json::json!({"id":"kept","fact":"KEPT-CANARY","source":"source-b","status":"active"}),
        ];
        let original_items = serde_json::to_vec_pretty(&items).unwrap();
        let original_summary = render_para_summary("Alex Kim", items.iter()).unwrap();
        fs::write(directory.join("items.json"), &original_items).unwrap();
        fs::write(directory.join("summary.md"), &original_summary).unwrap();

        let result = rewrite_para_people_with_hooks(
            &config,
            |source, _| {
                if source == "source-a" {
                    RecordDisposition::RemoveOwned
                } else {
                    RecordDisposition::Keep
                }
            },
            |_| {},
            |published| {
                fs::write(
                    published.join("summary.md"),
                    "# Corrupted\n\nREVOKED-CANARY\n",
                )
                .unwrap();
            },
        );

        assert!(result.is_err());
        assert!(
            !directory.exists(),
            "the canonical person name must fail closed instead of exposing an old or mixed pair"
        );
        let count = count_knowledge_snapshot_locked(&config).unwrap().0;
        assert_eq!(
            count, 0,
            "hidden recovery generations are not reader-visible people"
        );
        assert!(!all_text_unfiltered(&config.knowledge.path).contains("REVOKED-CANARY"));
        let captures = fs::read_dir(&private_root)
            .unwrap()
            .flatten()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(PARA_PERSON_CAPTURE_PREFIX)
            })
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert!(
            captures.is_empty(),
            "normal POSIX retirement uses the fixed slot"
        );
        let (slot, _) = recyclable_para_generation_paths(&private_root, &directory).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(!all_text_unfiltered(&private_root).contains("REVOKED-CANARY"));
    }

    #[test]
    fn reconciliation_semantic_caps_fail_before_any_mutation() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        let people = config.knowledge.path.join("areas/people");
        let directory = people.join("bounded-person");
        fs::create_dir_all(&directory).unwrap();
        let items = (0..=MAX_PARA_RECONCILIATION_ITEMS)
            .map(|index| {
                serde_json::json!({
                    "id": index,
                    "fact": "bounded",
                    "source": "source-a",
                    "status": "active"
                })
            })
            .collect::<Vec<_>>();
        let original = serde_json::to_vec(&items).unwrap();
        assert!(original.len() as u64 <= MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES);
        fs::write(directory.join("items.json"), &original).unwrap();

        let error = rewrite_para_people(&config, |_, _| RecordDisposition::Keep)
            .expect_err("an over-count person must fail before publication");
        assert!(error.to_string().contains("item bound"));
        assert_eq!(fs::read(directory.join("items.json")).unwrap(), original);
        assert!(!fs::read_dir(&people).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".minutes-person-")
        }));

        let wiki_people = config.knowledge.path.join("people");
        fs::create_dir_all(&wiki_people).unwrap();
        let oversized = wiki_people.join("oversized.md");
        let oversized_bytes =
            vec![b'x'; usize::try_from(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES).unwrap() + 1];
        fs::write(&oversized, &oversized_bytes).unwrap();
        let mut wiki_config = config.clone();
        wiki_config.knowledge.adapter = "wiki".into();
        let error = rewrite_wiki_people(&wiki_config, |_, _| RecordDisposition::Keep)
            .expect_err("an over-byte profile must fail before mutation");
        assert!(error.to_string().contains("bounded preservation size"));
        assert_eq!(fs::read(&oversized).unwrap(), oversized_bytes);
        fs::remove_file(&oversized).unwrap();

        let near = wiki_people.join("near.md");
        let suffix = " *(strong; 2026-07-17 — source-a)*\n";
        let prefix = "# Near\n\n## Context\n\n- ";
        let padding = usize::try_from(MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES).unwrap()
            - prefix.len()
            - suffix.len();
        let near_content = format!("{prefix}{}{suffix}", "x".repeat(padding));
        assert_eq!(
            near_content.len() as u64,
            MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES
        );
        fs::write(&near, &near_content).unwrap();
        assert_eq!(
            rewrite_wiki_people(&wiki_config, |_, _| RecordDisposition::Keep).unwrap(),
            0
        );
        assert_eq!(fs::read_to_string(&near).unwrap(), near_content);
    }

    #[test]
    fn para_depth_and_output_caps_fail_before_generation_claim() {
        for mode in ["depth", "output"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let config = knowledge_config(&meetings, &knowledge, "para");
            let people = config.knowledge.path.join("areas/people");
            let directory = people.join("bounded-person");
            fs::create_dir_all(&directory).unwrap();
            let values = if mode == "depth" {
                let mut nested = serde_json::Value::String("leaf".into());
                for _ in 0..=MAX_PARA_RECONCILIATION_VALUE_DEPTH {
                    nested = serde_json::Value::Array(vec![nested]);
                }
                vec![serde_json::json!({
                    "id": "deep",
                    "fact": "bounded",
                    "source": "source-a",
                    "status": "active",
                    "nested": nested
                })]
            } else {
                (0..MAX_PARA_RECONCILIATION_ITEMS)
                    .map(|index| {
                        serde_json::json!({
                            "id": index,
                            "fact": "x".repeat(430),
                            "source": "source-a",
                            "status": "active"
                        })
                    })
                    .collect()
            };
            let original = serde_json::to_vec(&values).unwrap();
            assert!(original.len() as u64 <= MAX_KNOWLEDGE_RECONCILIATION_FILE_BYTES);
            fs::write(directory.join("items.json"), &original).unwrap();

            let error = rewrite_para_people(&config, |_, _| RecordDisposition::Keep)
                .expect_err("bounded malformed generation must fail before its claim");
            if mode == "depth" {
                assert!(error.to_string().contains("depth bound"));
            } else {
                assert!(error.to_string().contains("byte bound"));
            }
            assert_eq!(fs::read(directory.join("items.json")).unwrap(), original);
            assert!(!fs::read_dir(&people).unwrap().flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-person-")
            }));
        }
    }

    // Windows denies this directory rename while the retained private
    // capability is live; `windows_retained_root_and_namespace_block_redirect`
    // asserts that stronger result directly.
    #[cfg(unix)]
    #[test]
    fn preservation_root_real_directory_replacement_is_detected_between_operations() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let mut replacement = None;
        let result = open_preservation_namespace_with(&config, |root| {
            let moved = root.with_extension("original-root");
            fs::rename(root, &moved).unwrap();
            fs::create_dir(root).unwrap();
            replacement = Some((root.to_path_buf(), moved));
        });
        assert!(result.is_err());
        let (new_root, moved_root) = replacement.unwrap();
        assert_eq!(fs::read_dir(&new_root).unwrap().count(), 0);
        assert!(fs::read_dir(&moved_root).unwrap().count() > 0);
    }

    #[test]
    fn wiki_and_obsidian_manual_bytes_are_quarantined_not_deleted() {
        for adapter in ["wiki", "obsidian"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let meeting = meetings.path().join("normal.md");
            fs::write(
                &meeting,
                meeting_markdown("Normal", None, "AUTHORIZED-FACT"),
            )
            .unwrap();
            let config = knowledge_config(&meetings, &knowledge, adapter);
            ingest_file(&meeting, &config).unwrap();

            let people = config.knowledge.path.join("people");
            fs::write(
                people.join("manual-profile.md"),
                "# Manual Person\n\nMANUAL-PROFILE-CANARY\n",
            )
            .unwrap();
            let generated = people.join("alex-kim.md");
            let mut mixed = fs::read_to_string(&generated).unwrap();
            mixed.push_str("\nMANUAL-MIXED-CANARY\n");
            fs::write(&generated, mixed).unwrap();

            reconcile_knowledge_derivatives(&config).unwrap();
            let visible = all_knowledge_text(knowledge.path());
            assert!(visible.contains("AUTHORIZED-FACT"), "adapter={adapter}");
            assert!(
                !visible.contains("MANUAL-PROFILE-CANARY"),
                "adapter={adapter}"
            );
            assert!(
                !visible.contains("MANUAL-MIXED-CANARY"),
                "adapter={adapter}"
            );
            let preserved = all_preserved_text(&config);
            assert!(
                preserved.contains("MANUAL-PROFILE-CANARY"),
                "adapter={adapter}"
            );
            assert!(
                preserved.contains("MANUAL-MIXED-CANARY"),
                "adapter={adapter}"
            );
        }
    }

    #[test]
    fn invalid_utf8_wiki_profile_and_log_are_preserved_outside_visible_store() {
        for adapter in ["wiki", "obsidian"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let meeting = meetings.path().join("normal.md");
            fs::write(&meeting, meeting_markdown("Normal", None, "GENERATED-FACT")).unwrap();
            let config = knowledge_config(&meetings, &knowledge, adapter);
            ingest_file(&meeting, &config).unwrap();
            let profile = config.knowledge.path.join("people/alex-kim.md");
            let log = config.knowledge.path.join("log.md");
            let mut profile_bytes = fs::read(&profile).unwrap();
            profile_bytes.extend_from_slice(b"PROFILE-BYTE-CANARY\xff\n");
            fs::write(&profile, profile_bytes).unwrap();
            let mut log_bytes = fs::read(&log).unwrap();
            log_bytes.extend_from_slice(b"LOG-BYTE-CANARY\xfe\n");
            fs::write(&log, log_bytes).unwrap();

            reconcile_knowledge_derivatives(&config).unwrap();
            let visible = all_knowledge_text(knowledge.path());
            assert!(!visible.contains("PROFILE-BYTE-CANARY"));
            assert!(!visible.contains("LOG-BYTE-CANARY"));
            let preserved = all_preserved_text(&config);
            assert!(preserved.contains("PROFILE-BYTE-CANARY"));
            assert!(preserved.contains("LOG-BYTE-CANARY"));
        }
    }

    #[test]
    fn manifest_rejects_tampered_record_even_with_an_authorized_source() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let meeting = meetings.path().join("normal.md");
        fs::write(
            &meeting,
            meeting_markdown("Normal", None, "AUTHORIZED-FACT"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&meeting, &config).unwrap();
        let source = exact_source_key(&meeting, &config).unwrap();
        let profile = config.knowledge.path.join("people/alex-kim.md");
        let mut content = fs::read_to_string(&profile).unwrap();
        content.push_str(&format!(
            "- TAMPERED-OWNERSHIP-CANARY *(strong; 2026-06-10 — {source})*\n"
        ));
        fs::write(&profile, content).unwrap();

        reconcile_knowledge_derivatives(&config).unwrap();
        assert!(!all_knowledge_text(knowledge.path()).contains("TAMPERED-OWNERSHIP-CANARY"));
        assert!(all_preserved_text(&config).contains("TAMPERED-OWNERSHIP-CANARY"));
    }

    #[test]
    fn adapter_and_log_layout_flips_retract_old_owned_layouts_and_preserve_manual_bytes() {
        for (from, to) in [("wiki", "para"), ("para", "wiki")] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let meeting = meetings.path().join("normal.md");
            fs::write(
                &meeting,
                meeting_markdown("Normal", None, "OLD-LAYOUT-GENERATED-CANARY"),
            )
            .unwrap();
            let mut config = knowledge_config(&meetings, &knowledge, from);
            config.knowledge.log_file = "old-layout-log.md".into();
            ingest_file(&meeting, &config).unwrap();

            let old_person = if from == "para" {
                config
                    .knowledge
                    .path
                    .join("areas/people/alex-kim/items.json")
            } else {
                config.knowledge.path.join("people/alex-kim.md")
            };
            let mut person_bytes = fs::read(&old_person).unwrap();
            person_bytes.extend_from_slice(b"\nOLD-LAYOUT-MANUAL-CANARY\n");
            fs::write(&old_person, person_bytes).unwrap();
            let old_log = knowledge_log_path(&config.knowledge);
            let mut log_bytes = fs::read(&old_log).unwrap();
            log_bytes.extend_from_slice(b"\nOLD-LOG-MANUAL-CANARY\n");
            fs::write(&old_log, log_bytes).unwrap();

            config.knowledge.adapter = to.into();
            config.knowledge.log_file = "new-layout-log.md".into();
            reconcile_knowledge_derivatives(&config).unwrap();

            let visible = all_knowledge_text(knowledge.path());
            assert!(!visible.contains("OLD-LAYOUT-GENERATED-CANARY"));
            assert!(!visible.contains("OLD-LAYOUT-MANUAL-CANARY"));
            assert!(!visible.contains("OLD-LOG-MANUAL-CANARY"));
            assert!(!old_log.exists());
            let preserved = all_preserved_text(&config);
            assert!(preserved.contains("OLD-LAYOUT-MANUAL-CANARY"));
            assert!(preserved.contains("OLD-LOG-MANUAL-CANARY"));
            let manifest: KnowledgeProvenanceManifest = serde_json::from_slice(
                &fs::read(provenance_manifest_path(&config).unwrap()).unwrap(),
            )
            .unwrap();
            assert_eq!(
                manifest.managed_logs,
                BTreeSet::from([managed_log_relative_path(&config).unwrap()])
            );
        }
    }

    #[test]
    fn provenance_temp_is_fixed_bounded_and_reusable_after_post_sync_failures() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let manifest = KnowledgeProvenanceManifest {
            schema: 4,
            sources: BTreeMap::from([("source".into(), "revision".into())]),
            records: BTreeMap::from([("source".into(), BTreeSet::from(["record".into()]))]),
            managed_logs: BTreeSet::new(),
        };
        let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK).unwrap();

        for attempt in 0..12 {
            if attempt % 2 == 0 {
                let error = save_provenance_manifest_with_hook(&config, &manifest, |_| {
                    Err("simulated post-sync provenance error".into())
                })
                .unwrap_err();
                assert!(error.to_string().contains("post-sync"));
            } else {
                let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    save_provenance_manifest_with_hook(&config, &manifest, |_| {
                        panic!("simulated post-sync provenance process loss")
                    })
                    .unwrap();
                }));
                assert!(crashed.is_err());
            }
            let manifest_path = provenance_manifest_path(&config).unwrap();
            let namespace = manifest_path.parent().unwrap();
            let temps = fs::read_dir(namespace)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name() == PRIVATE_KNOWLEDGE_PROVENANCE_TEMP)
                .collect::<Vec<_>>();
            assert_eq!(temps.len(), 1, "the fixed temp slot must not grow");
            assert!(temps[0].metadata().unwrap().len() <= MAX_KNOWLEDGE_PROVENANCE_MANIFEST_BYTES);
        }

        save_provenance_manifest(&config, &manifest).unwrap();

        let manifest_path = provenance_manifest_path(&config).unwrap();
        assert!(!manifest_path
            .parent()
            .unwrap()
            .join(PRIVATE_KNOWLEDGE_PROVENANCE_TEMP)
            .exists());
        assert_eq!(load_provenance_manifest(&config), (manifest, true));
    }

    #[test]
    fn numeric_v3_manifest_migrates_by_retracting_only_owned_records() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let meeting = meetings.path().join("normal.md");
        fs::write(
            &meeting,
            meeting_markdown("Normal", None, "V3-GENERATED-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&meeting, &config).unwrap();
        let manifest_path = provenance_manifest_path(&config).unwrap();
        let current: serde_json::Value =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        let legacy = serde_json::json!({
            "schema": 3,
            "sources": current["sources"]
                .as_object()
                .unwrap()
                .keys()
                .map(|key| (key.clone(), serde_json::json!(42_u64)))
                .collect::<serde_json::Map<String, serde_json::Value>>(),
            "records": current["records"].clone(),
        });
        fs::write(&manifest_path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        #[cfg(windows)]
        {
            // A real v3 installation predates the Windows publication
            // journals. Recreate that upgrade topology instead of leaving a
            // valid v4 terminal receipt whose exact target was then modified
            // behind its back.
            let namespace = manifest_path.parent().unwrap();
            for journal in PRIVATE_KNOWLEDGE_PROVENANCE_JOURNALS {
                fs::write(namespace.join(journal), b"").unwrap();
            }
        }
        let profile = config.knowledge.path.join("people/alex-kim.md");
        let mut profile_content = fs::read_to_string(&profile).unwrap();
        profile_content.push_str("\nV3-MANUAL-CANARY\n");
        fs::write(&profile, profile_content).unwrap();

        reconcile_knowledge_derivatives(&config).unwrap();
        let visible = all_knowledge_text(knowledge.path());
        assert!(!visible.contains("V3-GENERATED-CANARY"));
        assert!(!visible.contains("V3-MANUAL-CANARY"));
        assert!(all_preserved_text(&config).contains("V3-MANUAL-CANARY"));
        let migrated: KnowledgeProvenanceManifest =
            serde_json::from_slice(&fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(migrated.schema, 4);
        assert!(migrated.sources.is_empty());
        assert!(migrated.records.is_empty());
    }

    #[test]
    fn forged_public_manifest_never_authorizes_a_para_generation() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let meeting = meetings.path().join("normal.md");
        fs::write(
            &meeting,
            meeting_markdown("Normal", None, "LEGITIMATE-PROVENANCE-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&meeting, &config).unwrap();

        let source = exact_source_key(&meeting, &config).unwrap();
        let forged = serde_json::json!({
            "id": "forged-public-manifest",
            "fact": "FORGED-PUBLIC-MANIFEST-CANARY",
            "source": source,
            "status": "active"
        });
        let forged_id = record_id_for_json("para", &forged).unwrap();
        let person = config.knowledge.path.join("areas/people/alex-kim");
        fs::write(
            person.join("items.json"),
            serde_json::to_vec_pretty(&vec![forged]).unwrap(),
        )
        .unwrap();
        fs::write(
            person.join("summary.md"),
            "# Alex Kim\n\n- FORGED-PUBLIC-MANIFEST-CANARY\n",
        )
        .unwrap();
        let public_forgery = KnowledgeProvenanceManifest {
            schema: 4,
            sources: BTreeMap::from([(
                source.clone(),
                source_revision(&authorized_meeting(&meeting, &config).unwrap().content),
            )]),
            records: BTreeMap::from([(source, BTreeSet::from([forged_id]))]),
            managed_logs: BTreeSet::new(),
        };
        let public_manifest = legacy_public_provenance_manifest_path(&config);
        fs::write(
            &public_manifest,
            serde_json::to_vec_pretty(&public_forgery).unwrap(),
        )
        .unwrap();

        reconcile_knowledge_derivatives(&config).unwrap();

        assert!(
            !all_text_unfiltered(&config.knowledge.path).contains("FORGED-PUBLIC-MANIFEST-CANARY")
        );
        assert!(!public_manifest.exists());
        assert!(all_preserved_text(&config).contains("FORGED-PUBLIC-MANIFEST-CANARY"));
    }

    #[test]
    fn after_write_manual_injection_is_never_claimed_by_manifest() {
        for adapter in ["wiki", "obsidian", "para"] {
            let meetings = TempDir::new().unwrap();
            let knowledge = TempDir::new().unwrap();
            let meeting = meetings.path().join("normal.md");
            fs::write(&meeting, meeting_markdown("Normal", None, "GENERATED-FACT")).unwrap();
            let config = knowledge_config(&meetings, &knowledge, adapter);
            let source = exact_source_key(&meeting, &config).unwrap();
            let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK).unwrap();
            let mut injected = false;
            update_path_transaction_locked(&meeting, &config, false, &mut |phase| {
                if phase != KnowledgeTxnPhase::AfterWrite || injected {
                    return;
                }
                injected = true;
                if adapter == "para" {
                    let items = config.knowledge.path.join("areas/people/alex-kim/items.json");
                    let mut values: Vec<serde_json::Value> =
                        serde_json::from_slice(&fs::read(&items).unwrap()).unwrap();
                    values.push(serde_json::json!({
                        "id": "manual-claim",
                        "fact": "MANUAL-CLAIM-CANARY",
                        "source": source,
                        "status": "active"
                    }));
                    fs::write(&items, serde_json::to_vec_pretty(&values).unwrap()).unwrap();
                } else {
                    let profile = config.knowledge.path.join("people/alex-kim.md");
                    let mut content = fs::read_to_string(&profile).unwrap();
                    content.push_str(&format!(
                        "- MANUAL-CLAIM-CANARY *(strong; 2026-06-10 — {source})*\n"
                    ));
                    fs::write(&profile, content).unwrap();
                }
                let log = knowledge_log_path(&config.knowledge);
                let mut content = fs::read_to_string(&log).unwrap();
                content.push_str(&format!(
                    "## [2026-06-10 13:00] ingest | MANUAL-LOG-CLAIM\n\n- Source: `{source}`\n- Facts written: 1, skipped: 0\n- People: Alex Kim\n\n"
                ));
                fs::write(log, content).unwrap();
            })
            .unwrap();
            drop(_lock);

            fs::write(
                &meeting,
                meeting_markdown("Normal", Some("restricted"), "GENERATED-FACT"),
            )
            .unwrap();
            reconcile_knowledge_derivatives(&config).unwrap();
            let visible = all_knowledge_text(knowledge.path());
            assert!(
                !visible.contains("MANUAL-CLAIM-CANARY"),
                "adapter={adapter}"
            );
            assert!(!visible.contains("MANUAL-LOG-CLAIM"), "adapter={adapter}");
            let preserved = all_preserved_text(&config);
            assert!(
                preserved.contains("MANUAL-CLAIM-CANARY"),
                "adapter={adapter}"
            );
            assert!(preserved.contains("MANUAL-LOG-CLAIM"), "adapter={adapter}");
        }
    }

    #[test]
    fn para_post_retraction_same_source_injection_is_quarantined_not_self_authorized() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let meeting = meetings.path().join("normal.md");
        fs::write(
            &meeting,
            meeting_markdown("Normal", None, "LEGITIMATE-GENERATED-FACT"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        let source = exact_source_key(&meeting, &config).unwrap();
        let forged = serde_json::json!({
            "id": "forged-after-retraction",
            "fact": "FORGED-SAME-SOURCE-CANARY",
            "source": source,
            "status": "active"
        });
        let forged_id = record_id_for_json("para", &forged).unwrap();
        let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK).unwrap();
        let mut injected = false;
        update_path_transaction_locked(&meeting, &config, false, &mut |phase| {
            if injected || phase != KnowledgeTxnPhase::AfterDerivativeRetraction {
                return;
            }
            injected = true;
            let person = config.knowledge.path.join("areas/people/alex-kim");
            fs::create_dir_all(&person).unwrap();
            fs::write(
                person.join("items.json"),
                serde_json::to_vec_pretty(&vec![forged.clone()]).unwrap(),
            )
            .unwrap();
            fs::write(
                person.join("summary.md"),
                "# Alex Kim\n\n- FORGED-SAME-SOURCE-CANARY\n",
            )
            .unwrap();
        })
        .unwrap();
        drop(_lock);

        assert!(injected);
        assert!(!all_text_unfiltered(&config.knowledge.path).contains("FORGED-SAME-SOURCE-CANARY"));
        let (manifest, valid) = load_provenance_manifest(&config);
        assert!(valid);
        assert!(!manifest
            .records
            .values()
            .any(|records| records.contains(&forged_id)));
        let private_root = para_private_root(&config).unwrap();
        assert!(!private_root.starts_with(&config.knowledge.path));
        assert!(all_text_unfiltered(&private_root).contains("FORGED-SAME-SOURCE-CANARY"));
    }

    #[test]
    fn source_change_hook_replaces_then_deletes_derivatives_without_unrelated_ingest() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("mutable.md");
        fs::write(&path, meeting_markdown("Mutable", None, "OLD-CANARY")).unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&path, &config).unwrap();
        assert!(all_knowledge_text(knowledge.path()).contains("OLD-CANARY"));

        fs::write(&path, meeting_markdown("Mutable", None, "NEW-CANARY")).unwrap();
        refresh_after_source_change(&path, &config).unwrap();
        let edited = all_knowledge_text(knowledge.path());
        assert!(!edited.contains("OLD-CANARY"));
        assert!(edited.contains("NEW-CANARY"));

        fs::remove_file(&path).unwrap();
        refresh_after_source_change(&path, &config).unwrap();
        let deleted = all_knowledge_text(knowledge.path());
        assert!(!deleted.contains("OLD-CANARY"));
        assert!(!deleted.contains("NEW-CANARY"));
    }

    #[test]
    fn source_change_hook_runs_qmd_fail_closed_path_even_when_knowledge_refresh_fails() {
        let meetings = TempDir::new().unwrap();
        let state = TempDir::new().unwrap();
        let knowledge_file = state.path().join("knowledge-is-a-file");
        fs::write(&knowledge_file, "not a directory").unwrap();
        let path = meetings.path().join("changed.md");
        fs::write(&path, meeting_markdown("Changed", None, "CANARY")).unwrap();
        let mut config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge_file,
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };
        config.search.qmd_collection = Some("minutes".into());
        let mut qmd_called = false;
        let result = refresh_after_source_change_with(&path, &config, || {
            qmd_called = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(qmd_called);
        assert!(!result.unwrap_err().to_string().contains("changed.md"));
    }

    #[test]
    fn pre_read_revalidation_retracts_external_change_and_still_runs_qmd_after_knowledge_error() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("external-change.md");
        fs::write(
            &path,
            meeting_markdown("External", None, "EXTERNAL-CHANGE-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&path, &config).unwrap();
        fs::write(
            &path,
            meeting_markdown("External", Some("restricted"), "EXTERNAL-CHANGE-CANARY"),
        )
        .unwrap();
        revalidate_persistent_derivatives_before_read_with(&config, || Ok(())).unwrap();
        assert!(!all_knowledge_text(knowledge.path()).contains("EXTERNAL-CHANGE-CANARY"));

        let broken = config.knowledge.path.join("not-a-directory");
        fs::write(&broken, "file").unwrap();
        let mut broken_config = config.clone();
        broken_config.knowledge.path = broken;
        broken_config.search.qmd_collection = Some("minutes".into());
        let mut qmd_called = false;
        let result = revalidate_persistent_derivatives_before_read_with(&broken_config, || {
            qmd_called = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(qmd_called);
    }

    #[test]
    fn deleted_contained_path_retracts_exact_source_but_outside_collision_cannot() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let first = meetings.path().join("team-a/shared.md");
        let second = meetings.path().join("team-b/shared.md");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, meeting_markdown("First", None, "FIRST-CANARY")).unwrap();
        fs::write(&second, meeting_markdown("Second", None, "SECOND-CANARY")).unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&first, &config).unwrap();
        ingest_file(&second, &config).unwrap();

        let outside = TempDir::new().unwrap().path().join("shared.md");
        retract_meeting_derivatives(&outside, &config).unwrap();
        let after_outside = all_knowledge_text(knowledge.path());
        assert!(after_outside.contains("FIRST-CANARY"));
        assert!(after_outside.contains("SECOND-CANARY"));

        fs::remove_file(&first).unwrap();
        retract_meeting_derivatives(&first, &config).unwrap();
        let after_delete = all_knowledge_text(knowledge.path());
        assert!(!after_delete.contains("FIRST-CANARY"));
        assert!(after_delete.contains("SECOND-CANARY"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn deleted_macos_var_private_var_alias_retracts_all_adapters_and_manifest() {
        let workspace = TempDir::new().unwrap();
        let canonical_workspace = workspace.path().canonicalize().unwrap();
        let relative = canonical_workspace
            .strip_prefix("/private/var")
            .expect("macOS temporary directory must resolve beneath /private/var");
        let raw_workspace = Path::new("/var").join(relative);
        assert_ne!(raw_workspace, canonical_workspace);
        assert_eq!(raw_workspace.canonicalize().unwrap(), canonical_workspace);
        for adapter in ["wiki", "obsidian", "para"] {
            for canonical_config in [false, true] {
                let suffix = if canonical_config {
                    "canonical"
                } else {
                    "short"
                };
                let raw_meetings = raw_workspace.join(format!("meetings-{adapter}-{suffix}"));
                let raw_knowledge = raw_workspace.join(format!("knowledge-{adapter}-{suffix}"));
                fs::create_dir_all(&raw_meetings).unwrap();
                fs::create_dir_all(&raw_knowledge).unwrap();
                let raw_path = raw_meetings.join("alias.md");
                fs::write(&raw_path, meeting_markdown("Alias", None, "ALIAS-CANARY")).unwrap();
                let canonical_path = raw_path.canonicalize().unwrap();
                let config = Config {
                    output_dir: if canonical_config {
                        raw_meetings.canonicalize().unwrap()
                    } else {
                        raw_meetings
                    },
                    knowledge: KnowledgeConfig {
                        enabled: true,
                        path: raw_knowledge,
                        adapter: adapter.into(),
                        ..Default::default()
                    },
                    ..Config::default()
                };
                let ingest_path = if canonical_config {
                    canonical_path.clone()
                } else {
                    raw_path.clone()
                };
                let deleted_hook = if canonical_config {
                    raw_path.clone()
                } else {
                    canonical_path
                };
                ingest_file(&ingest_path, &config).unwrap();
                fs::remove_file(&raw_path).unwrap();
                retract_meeting_derivatives(&deleted_hook, &config).unwrap();
                assert!(!all_knowledge_text(&config.knowledge.path).contains("ALIAS-CANARY"));
                let (manifest, valid) = load_provenance_manifest(&config);
                assert!(valid);
                assert!(manifest.sources.is_empty() && manifest.records.is_empty());
            }
        }
    }

    #[test]
    fn ambiguous_legacy_basename_is_removed_without_touching_other_v2_source() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let first = meetings.path().join("team-a/shared.md");
        let second = meetings.path().join("team-b/shared.md");
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        fs::create_dir_all(second.parent().unwrap()).unwrap();
        fs::write(&first, meeting_markdown("First", None, "unused")).unwrap();
        fs::write(&second, meeting_markdown("Second", None, "unused")).unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        let first_v2 = preferred_source_key(&first, &config).unwrap();
        let second_v2 = preferred_source_key(&second, &config).unwrap();
        let knowledge_config = &config.knowledge;
        WikiAdapter
            .update_person(
                "alex-kim",
                "Alex Kim",
                &[
                    Fact {
                        text: "LEGACY-AMBIGUOUS".into(),
                        category: "context".into(),
                        confidence: Confidence::Strong,
                        source_meeting: "shared".into(),
                        source_date: "2026-06-10".into(),
                    },
                    Fact {
                        text: "FIRST-V2".into(),
                        category: "context".into(),
                        confidence: Confidence::Strong,
                        source_meeting: first_v2,
                        source_date: "2026-06-10".into(),
                    },
                    Fact {
                        text: "SECOND-V2".into(),
                        category: "context".into(),
                        confidence: Confidence::Strong,
                        source_meeting: second_v2,
                        source_date: "2026-06-10".into(),
                    },
                ],
                Confidence::Strong,
                knowledge_config,
            )
            .unwrap();

        retract_meeting_derivatives(&first, &config).unwrap();
        let after = all_knowledge_text(knowledge.path());
        assert!(!after.contains("LEGACY-AMBIGUOUS"));
        assert!(!after.contains("FIRST-V2"));
        assert!(after.contains("SECOND-V2"));
    }

    #[test]
    fn knowledge_reclassification_retracts_fact_title_path_and_log() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("board-secret-name.md");
        fs::write(
            &path,
            meeting_markdown("Board Secret Title", None, "FACT-CANARY-47"),
        )
        .unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.path().to_path_buf(),
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };

        let ingested = ingest_file(&path, &config).unwrap();
        assert_eq!(ingested.facts_written, 1);
        let profile = knowledge.path().join("people/alex-kim.md");
        assert!(fs::read_to_string(&profile)
            .unwrap()
            .contains("FACT-CANARY-47"));

        fs::write(
            &path,
            meeting_markdown("Board Secret Title", Some("restricted"), "FACT-CANARY-47"),
        )
        .unwrap();
        let error = ingest_file(&path, &config).unwrap_err().to_string();
        assert!(error.contains("restricted"));
        assert!(!error.contains("board-secret-name"));
        let profile_after = fs::read_to_string(&profile).unwrap_or_default();
        assert!(!profile_after.contains("FACT-CANARY-47"));
        assert!(!profile_after.contains("board-secret-name"));
        assert!(!profile_after.contains("Alex Kim"));
        let log_after = fs::read_to_string(knowledge.path().join("log.md")).unwrap();
        assert!(!log_after.contains("Board Secret Title"));
        assert!(!log_after.contains("board-secret-name"));
    }

    #[test]
    fn knowledge_reconcile_retracts_normal_to_malformed_without_logging_filename() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("private-codename.md");
        fs::write(
            &path,
            meeting_markdown("Private Codename", None, "MALFORMED-FACT-CANARY"),
        )
        .unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.path().to_path_buf(),
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };
        ingest_file(&path, &config).unwrap();
        fs::write(&path, "---\ntitle: [bad\n---\nMALFORMED-FACT-CANARY").unwrap();

        let reconciled = reconcile_knowledge_derivatives(&config).unwrap();
        assert_eq!(reconciled.facts_removed, 1);
        let profile =
            fs::read_to_string(knowledge.path().join("people/alex-kim.md")).unwrap_or_default();
        assert!(!profile.contains("MALFORMED-FACT-CANARY"));
        let error = authorized_meeting(&path, &config).unwrap_err().to_string();
        assert!(error.contains(&privacy_safe_source_scope(&path)));
        assert!(!error.contains("private-codename"));
    }

    #[test]
    fn knowledge_retraction_preserves_same_fact_from_another_normal_source() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let first = meetings.path().join("first.md");
        let second = meetings.path().join("second.md");
        fs::write(
            &first,
            meeting_markdown("First", None, "SHARED-FACT-CANARY"),
        )
        .unwrap();
        fs::write(
            &second,
            meeting_markdown("Second", None, "SHARED-FACT-CANARY"),
        )
        .unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: KnowledgeConfig {
                enabled: true,
                path: knowledge.path().to_path_buf(),
                adapter: "wiki".into(),
                ..Default::default()
            },
            ..Config::default()
        };
        ingest_file(&first, &config).unwrap();
        ingest_file(&second, &config).unwrap();
        let profile_path = knowledge.path().join("people/alex-kim.md");
        let before = fs::read_to_string(&profile_path).unwrap();
        assert_eq!(before.matches("SHARED-FACT-CANARY").count(), 2);

        fs::write(
            &first,
            meeting_markdown("First", Some("restricted"), "SHARED-FACT-CANARY"),
        )
        .unwrap();
        reconcile_knowledge_derivatives(&config).unwrap();
        let after = fs::read_to_string(&profile_path).unwrap();
        assert_eq!(after.matches("SHARED-FACT-CANARY").count(), 1);
        assert!(after.contains("second"));
        assert!(!after.contains("first"));
    }

    #[test]
    fn source_revision_flip_before_or_after_write_leaves_no_derivative_in_any_adapter() {
        for adapter in ["wiki", "obsidian", "para"] {
            for flip_phase in [
                KnowledgeTxnPhase::BeforeFinalAuthorization,
                KnowledgeTxnPhase::AfterDerivativeRetraction,
                KnowledgeTxnPhase::AfterWrite,
                KnowledgeTxnPhase::AfterManifestCommit,
            ] {
                let meetings = TempDir::new().unwrap();
                let knowledge = TempDir::new().unwrap();
                let path = meetings.path().join("flip.md");
                fs::write(&path, meeting_markdown("Flip", None, "FLIP-SECRET-CANARY")).unwrap();
                let config = knowledge_config(&meetings, &knowledge, adapter);
                let _lock = acquire_policy_lock(KNOWLEDGE_POLICY_LOCK).unwrap();
                let mut flipped = false;
                let update = update_path_transaction_locked(&path, &config, false, &mut |phase| {
                    if !flipped && phase == flip_phase {
                        fs::write(
                            &path,
                            meeting_markdown("Flip", Some("restricted"), "FLIP-SECRET-CANARY"),
                        )
                        .unwrap();
                        flipped = true;
                    }
                })
                .unwrap();
                assert_eq!(
                    update.facts_written, 0,
                    "adapter={adapter} phase={flip_phase:?}"
                );
                let derived = all_knowledge_text(knowledge.path());
                assert!(
                    !derived.contains("FLIP-SECRET-CANARY"),
                    "adapter={adapter} phase={flip_phase:?}"
                );
                assert!(
                    !derived.contains("# Alex Kim"),
                    "adapter={adapter} phase={flip_phase:?}"
                );
                assert!(
                    !derived.contains("## ["),
                    "adapter={adapter} phase={flip_phase:?}"
                );
                let (manifest, valid) = load_provenance_manifest(&config);
                assert!(valid, "adapter={adapter} phase={flip_phase:?}");
                assert!(
                    manifest.sources.is_empty() && manifest.records.is_empty(),
                    "adapter={adapter} phase={flip_phase:?}"
                );
            }
        }
    }

    #[test]
    fn para_reconcile_quarantines_invalid_provenance_and_regenerates_summary() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("normal.md");
        fs::write(
            &path,
            meeting_markdown("Normal", None, "AUTHORIZED-PARA-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&path, &config).unwrap();
        let people = config.knowledge.path.join("areas/people");
        let valid_summary = people.join("alex-kim/summary.md");
        fs::write(&valid_summary, "# Alex Kim\n\nINJECTED-SUMMARY-CANARY\n").unwrap();

        for (slug, source) in [
            ("missing", serde_json::json!({"fact": "MISSING-SOURCE"})),
            (
                "null",
                serde_json::json!({"fact": "NULL-SOURCE", "source": null}),
            ),
            (
                "nonstring",
                serde_json::json!({"fact": "NONSTRING-SOURCE", "source": 42}),
            ),
        ] {
            let dir = people.join(slug);
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("items.json"),
                serde_json::to_vec(&vec![source]).unwrap(),
            )
            .unwrap();
            fs::write(dir.join("summary.md"), "INVALID-SUMMARY-CANARY").unwrap();
        }
        let malformed = people.join("malformed");
        fs::create_dir_all(&malformed).unwrap();
        fs::write(malformed.join("items.json"), "{not-json").unwrap();
        fs::write(malformed.join("summary.md"), "MALFORMED-SUMMARY-CANARY").unwrap();

        reconcile_knowledge_derivatives(&config).unwrap();
        let derived = all_knowledge_text(knowledge.path());
        assert!(derived.contains("AUTHORIZED-PARA-CANARY"));
        for canary in [
            "INJECTED-SUMMARY-CANARY",
            "MISSING-SOURCE",
            "NULL-SOURCE",
            "NONSTRING-SOURCE",
            "INVALID-SUMMARY-CANARY",
            "MALFORMED-SUMMARY-CANARY",
        ] {
            assert!(!derived.contains(canary), "leaked {canary}");
        }
        let preserved = all_preserved_text(&config);
        for canary in [
            "INJECTED-SUMMARY-CANARY",
            "MISSING-SOURCE",
            "NULL-SOURCE",
            "NONSTRING-SOURCE",
            "INVALID-SUMMARY-CANARY",
            "MALFORMED-SUMMARY-CANARY",
        ] {
            assert!(preserved.contains(canary), "did not preserve {canary}");
        }
    }

    #[test]
    fn para_reconciliation_preserves_validated_display_name() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let first = meetings.path().join("first.md");
        let second = meetings.path().join("second.md");
        fs::write(&first, meeting_markdown("First", None, "FIRST-FACT")).unwrap();
        fs::write(&second, meeting_markdown("Second", None, "SECOND-FACT")).unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&first, &config).unwrap();
        ingest_file(&second, &config).unwrap();
        let summary = config
            .knowledge
            .path
            .join("areas/people/alex-kim/summary.md");

        reconcile_knowledge_derivatives(&config).unwrap();
        assert!(fs::read_to_string(&summary)
            .unwrap()
            .starts_with("# Alex Kim\n"));

        fs::write(
            &first,
            meeting_markdown("First", Some("restricted"), "FIRST-FACT"),
        )
        .unwrap();
        reconcile_knowledge_derivatives(&config).unwrap();
        let after = fs::read_to_string(&summary).unwrap();
        assert!(after.starts_with("# Alex Kim\n"));
        assert!(!after.contains("FIRST-FACT"));
        assert!(after.contains("SECOND-FACT"));
    }

    #[test]
    fn reconcile_sanitizes_damaged_wiki_profile_and_log_section_without_source() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let path = meetings.path().join("normal.md");
        fs::write(
            &path,
            meeting_markdown("Normal", None, "AUTHORIZED-WIKI-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "wiki");
        ingest_file(&path, &config).unwrap();
        let profile = config.knowledge.path.join("people/alex-kim.md");
        let mut profile_content = fs::read_to_string(&profile).unwrap();
        profile_content.push_str("DAMAGED-WIKI-CANARY\n");
        fs::write(&profile, profile_content).unwrap();
        let log = config.knowledge.path.join("log.md");
        let mut log_content = fs::read_to_string(&log).unwrap();
        log_content.push_str(
            "## [2026-06-10 12:00] ingest | Missing Source\n\nMISSING-LOG-SOURCE-CANARY\n\n",
        );
        fs::write(&log, log_content).unwrap();

        reconcile_knowledge_derivatives(&config).unwrap();
        let derived = all_knowledge_text(knowledge.path());
        assert!(profile.exists());
        assert!(derived.contains("AUTHORIZED-WIKI-CANARY"));
        assert!(!derived.contains("DAMAGED-WIKI-CANARY"));
        assert!(!derived.contains("MISSING-LOG-SOURCE-CANARY"));
        let preserved = all_preserved_text(&config);
        assert!(preserved.contains("DAMAGED-WIKI-CANARY"));
        assert!(preserved.contains("MISSING-LOG-SOURCE-CANARY"));
    }

    #[cfg(unix)]
    #[test]
    fn generated_knowledge_and_qmd_artifacts_are_private() {
        use std::os::unix::fs::PermissionsExt;

        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let mirror_parent = TempDir::new().unwrap();
        let path = meetings.path().join("normal.md");
        fs::write(&path, meeting_markdown("Normal", None, "PRIVATE-MODE")).unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&path, &config).unwrap();
        let mirror = mirror_parent.path().join("mirror");
        rebuild_qmd_policy_mirror_at(&config, &mirror).unwrap();

        for directory in [
            config.knowledge.path.clone(),
            config.knowledge.path.join("areas"),
            config.knowledge.path.join("areas/people"),
            config.knowledge.path.join("areas/people/alex-kim"),
            config.knowledge.path.join("memory"),
            mirror.clone(),
        ] {
            assert_eq!(
                fs::metadata(directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        for file in [
            provenance_manifest_path(&config).unwrap(),
            config
                .knowledge
                .path
                .join("areas/people/alex-kim/items.json"),
            config
                .knowledge
                .path
                .join("areas/people/alex-kim/summary.md"),
            config.knowledge.path.join("memory/log.md"),
            mirror.join("normal.md"),
            mirror.join(QMD_MIRROR_MARKER),
        ] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn wiki_adapter_creates_person_file() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            adapter: "wiki".into(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "Leads pharmacy operations for RxVIP".into(),
            category: "context".into(),
            confidence: Confidence::Strong,
            source_meeting: "2026-04-03-consult".into(),
            source_date: "2026-04-03".into(),
        }];

        let adapter = WikiAdapter;
        let (written, skipped) = adapter
            .update_person(
                "dan-benamoz",
                "Dan Benamoz",
                &facts,
                Confidence::Strong,
                &config,
            )
            .unwrap();

        assert_eq!(written, 1);
        assert_eq!(skipped, 0);

        let content = fs::read_to_string(dir.path().join("people/dan-benamoz.md")).unwrap();
        assert!(content.contains("# Dan Benamoz"));
        assert!(content.contains("Leads pharmacy operations for RxVIP"));
        assert!(content.contains("strong"));
        assert!(content.contains("2026-04-03-consult"));
    }

    #[test]
    fn wiki_adapter_deduplicates_facts() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "CTO at Acme Corp".into(),
            category: "context".into(),
            confidence: Confidence::Explicit,
            source_meeting: "meeting-1".into(),
            source_date: "2026-04-01".into(),
        }];

        let adapter = WikiAdapter;
        let (w1, _) = adapter
            .update_person("alice", "Alice", &facts, Confidence::Strong, &config)
            .unwrap();
        let (w2, _) = adapter
            .update_person("alice", "Alice", &facts, Confidence::Strong, &config)
            .unwrap();

        assert_eq!(w1, 1);
        assert_eq!(w2, 0); // deduped
    }

    #[test]
    fn wiki_retraction_removes_legacy_multiline_fact_as_one_provenance_block() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let meeting_path = meetings.path().join("legacy-source.md");
        fs::write(&meeting_path, meeting_markdown("Legacy", None, "unused")).unwrap();
        let knowledge_config = KnowledgeConfig {
            enabled: true,
            path: knowledge.path().to_path_buf(),
            ..Default::default()
        };
        WikiAdapter
            .update_person(
                "alex",
                "Alex",
                &[Fact {
                    text: "LEGACY-TOP\nLEGACY-SECRET".into(),
                    category: "context".into(),
                    confidence: Confidence::Strong,
                    source_meeting: "legacy-source".into(),
                    source_date: "2026-06-10".into(),
                }],
                Confidence::Strong,
                &knowledge_config,
            )
            .unwrap();
        // Rewrite the sanitized current representation into the historical
        // multiline format to exercise migration/retraction behavior.
        let profile = knowledge.path().join("people/alex.md");
        let legacy = fs::read_to_string(&profile)
            .unwrap()
            .replace("LEGACY-TOP LEGACY-SECRET", "LEGACY-TOP\nLEGACY-SECRET");
        fs::write(&profile, legacy).unwrap();
        let config = Config {
            output_dir: meetings.path().to_path_buf(),
            knowledge: knowledge_config,
            ..Config::default()
        };

        let removed = retract_meeting_derivatives(&meeting_path, &config).unwrap();
        assert_eq!(removed.facts_removed, 1);
        assert!(!profile.exists());
    }

    #[test]
    fn wiki_adapter_skips_low_confidence() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            min_confidence: "strong".into(),
            ..Default::default()
        };

        let facts = vec![
            Fact {
                text: "Might be interested in partnership".into(),
                category: "context".into(),
                confidence: Confidence::Tentative,
                source_meeting: "meeting-1".into(),
                source_date: "2026-04-01".into(),
            },
            Fact {
                text: "Confirmed: wants monthly billing".into(),
                category: "decision".into(),
                confidence: Confidence::Explicit,
                source_meeting: "meeting-1".into(),
                source_date: "2026-04-01".into(),
            },
        ];

        let adapter = WikiAdapter;
        let (written, skipped) = adapter
            .update_person("bob", "Bob", &facts, Confidence::Strong, &config)
            .unwrap();

        assert_eq!(written, 1);
        assert_eq!(skipped, 1);

        let content = fs::read_to_string(dir.path().join("people/bob.md")).unwrap();
        assert!(content.contains("Confirmed: wants monthly billing"));
        assert!(!content.contains("Might be interested"));
    }

    #[test]
    fn para_adapter_writes_items_json() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            adapter: "para".into(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "Building medicare billing into consultation software".into(),
            category: "commitment".into(),
            confidence: Confidence::Explicit,
            source_meeting: "2026-04-03-consult".into(),
            source_date: "2026-04-03".into(),
        }];

        let adapter = ParaAdapter;
        let (written, _) = adapter
            .update_person(
                "dan-benamoz",
                "Dan Benamoz",
                &facts,
                Confidence::Strong,
                &config,
            )
            .unwrap();

        assert_eq!(written, 1);

        let items_path = dir.path().join("areas/people/dan-benamoz/items.json");
        assert!(items_path.exists());

        let items: Vec<serde_json::Value> =
            serde_json::from_str(&fs::read_to_string(&items_path).unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "active");
        assert_eq!(items[0]["confidence"], "explicit");
        assert_eq!(items[0]["source"], "2026-04-03-consult");

        let summary_path = dir.path().join("areas/people/dan-benamoz/summary.md");
        assert!(summary_path.exists());
        let summary = fs::read_to_string(&summary_path).unwrap();
        assert!(summary.contains("# Dan Benamoz"));
        assert!(summary.contains("medicare billing"));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_normal_updates_recycle_private_generations_beyond_128_then_retract() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let directory = people.join("lifecycle");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        fs::create_dir_all(&directory).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(
            directory.join("items.json"),
            serde_json::to_vec_pretty(&serde_json::json!([
                {"id":"initial","fact":"LIFECYCLE-INITIAL","source":"source","status":"active"}
            ]))
            .unwrap(),
        )
        .unwrap();
        fs::write(directory.join("summary.md"), b"# Lifecycle\n\n- initial\n").unwrap();

        let mut fixed_residue_count = None;
        for revision in 0..1000 {
            let snapshot = inspect_para_person(&directory).unwrap();
            let items = serde_json::to_vec_pretty(&serde_json::json!([{
                "id": format!("revision-{revision}"),
                "fact": format!("LIFECYCLE-{revision:04}"),
                "source": "source",
                "status": "active"
            }]))
            .unwrap();
            let summary = format!("# Lifecycle\n\n- LIFECYCLE-{revision:04}\n").into_bytes();
            replace_para_person_generation_with_hook(
                &directory,
                &private_root,
                &snapshot,
                Some(items),
                Some(summary),
                |_| {},
            )
            .unwrap();
            let count = fs::read_dir(&private_root).unwrap().flatten().count();
            let expected = *fixed_residue_count.get_or_insert(count);
            assert_eq!(count, expected, "fixed PARA slots must not grow per update");
            assert!(
                count <= 3,
                "one generation slot plus two fixed journals is sufficient while active"
            );
        }

        let snapshot = inspect_para_person(&directory).unwrap();
        replace_para_person_generation_with_hook(
            &directory,
            &private_root,
            &snapshot,
            None,
            None,
            |_| {},
        )
        .unwrap();
        assert!(!directory.exists());
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &directory).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
        assert!(fs::read_dir(&private_root).unwrap().flatten().count() <= 4);

        let recreated_items = serde_json::to_vec_pretty(&serde_json::json!([{
            "id":"recreated","fact":"RECREATED","source":"source","status":"active"
        }]))
        .unwrap();
        publish_new_para_person_generation(
            &directory,
            &private_root,
            recreated_items.clone(),
            b"# Lifecycle\n\n- RECREATED\n".to_vec(),
        )
        .unwrap();
        assert_eq!(
            fs::read(directory.join("items.json")).unwrap(),
            recreated_items
        );
        assert!(!parked.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(fs::read_dir(&private_root).unwrap().flatten().count() <= 3);
    }

    #[test]
    fn para_laundered_generation_cannot_republish_a_newly_revoked_source() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        let config = knowledge_config(&meetings, &knowledge, "para");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "LAUNDER-REVOKE-CANARY"),
        )
        .unwrap();
        ingest_file(&source, &config).unwrap();

        let people = config.knowledge.path.join("areas/people");
        let original = people.join("alex-kim");
        let private_root = para_private_root(&config).unwrap();
        let private_capture =
            private_root.join(format!("{PARA_PERSON_CAPTURE_PREFIX}launder-attempt"));
        fs::rename(&original, &private_capture).unwrap();
        fs::rename(&private_capture, people.join("laundered-public-name")).unwrap();

        let mut revoked = false;
        let result = rewrite_para_people_with_hook(
            &config,
            |_, _| RecordDisposition::Keep,
            |_| {
                if !revoked {
                    fs::write(
                        &source,
                        meeting_markdown("Normal", Some("restricted"), "LAUNDER-REVOKE-CANARY"),
                    )
                    .unwrap();
                    revoked = true;
                }
            },
        );
        assert!(result.is_err());
        assert!(revoked);
        assert!(!all_knowledge_text(&config.knowledge.path).contains("LAUNDER-REVOKE-CANARY"));
    }

    #[test]
    fn para_crash_after_old_claim_recovers_intended_generation() {
        let root = TempDir::new().unwrap();
        let knowledge = root.path().join("visible-kb");
        let people = knowledge.join("areas/people");
        let private_root = para_private_root_for_test_knowledge(&knowledge).unwrap();
        assert!(!private_root.starts_with(&knowledge));
        prepare_para_private_root(&private_root).unwrap();
        let directory = people.join("crash-safe");
        fs::create_dir_all(&directory).unwrap();
        let old_items = serde_json::to_vec_pretty(&serde_json::json!([
            {"id":"old","fact":"OLD","source":"source-a","status":"active"}
        ]))
        .unwrap();
        let old_summary = b"# Crash Safe\n\n- OLD\n".to_vec();
        fs::write(directory.join("items.json"), &old_items).unwrap();
        fs::write(directory.join("summary.md"), &old_summary).unwrap();
        let snapshot = inspect_para_person(&directory).unwrap();
        let new_items = serde_json::to_vec_pretty(&serde_json::json!([
            {"id":"new","fact":"NEW","source":"source-b","status":"active"}
        ]))
        .unwrap();
        let new_summary = b"# Crash Safe\n\n## Context\n\n- NEW\n".to_vec();

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replace_para_person_generation_with_hooks(
                &directory,
                &private_root,
                &snapshot,
                Some(new_items.clone()),
                Some(new_summary.clone()),
                |_| panic!("simulated process loss after durable old claim"),
                |_| {},
            )
            .unwrap();
        }));
        assert!(crashed.is_err());
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            assert!(
                directory.exists(),
                "whole-directory exchange has no absence window"
            );
            assert!(all_text_unfiltered(&knowledge).contains("NEW"));
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            assert!(
                !directory.exists(),
                "legacy whole-directory claim must not publish before authorization resumes"
            );
            assert!(!all_text_unfiltered(&knowledge).contains("NEW"));
            assert!(all_text_unfiltered(&private_root).contains("NEW"));
        }
        assert!(!all_text_unfiltered(&knowledge).contains("OLD"));
        assert!(all_text_unfiltered(&private_root).contains("OLD"));

        recover_para_transactions(&people, &private_root, None).unwrap();
        assert_eq!(fs::read(directory.join("items.json")).unwrap(), new_items);
        assert_eq!(fs::read(directory.join("summary.md")).unwrap(), new_summary);
        assert!(fs::read_dir(&private_root).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(PARA_PERSON_CAPTURE_PREFIX)
        }));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_recovery_revalidates_already_public_generation_after_source_restriction() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "CRASH-REVOCATION-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();

        let people = config.knowledge.path.join("areas/people");
        let person = people.join("alex-kim");
        let private_root = para_private_root(&config).unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let successor_items = snapshot.items.bytes.clone();
        let mut successor_summary = snapshot.summary.as_ref().unwrap().bytes.clone();
        successor_summary.extend_from_slice(b"\n<!-- crash-publication-generation -->\n");

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replace_para_person_generation_with_hooks(
                &person,
                &private_root,
                &snapshot,
                Some(successor_items),
                Some(successor_summary),
                |_| {},
                |_| panic!("simulated loss after public publication"),
            )
            .unwrap();
        }));
        assert!(crashed.is_err());
        assert!(all_text_unfiltered(&config.knowledge.path).contains("CRASH-REVOCATION-CANARY"));

        fs::write(
            &source,
            meeting_markdown("Normal", Some("restricted"), "CRASH-REVOCATION-CANARY"),
        )
        .unwrap();
        recover_para_transactions(&people, &private_root, Some(&config)).unwrap();

        assert!(!person.exists());
        assert!(!all_text_unfiltered(&config.knowledge.path).contains("CRASH-REVOCATION-CANARY"));
        assert!(!all_text_unfiltered(&private_root).contains("CRASH-REVOCATION-CANARY"));
        assert_recyclable_terminal_receipt(&private_root, "alex-kim");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_recovery_retracts_revoked_successor_after_full_old_scrub() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "FULL-SCRUB-REVOCATION-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();

        let people = config.knowledge.path.join("areas/people");
        let person = people.join("alex-kim");
        let private_root = para_private_root(&config).unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let successor_items = snapshot.items.bytes.clone();
        let mut successor_summary = snapshot.summary.as_ref().unwrap().bytes.clone();
        successor_summary.extend_from_slice(b"\n<!-- full-old-scrub-crash -->\n");
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replace_para_person_generation_with_retirement_hooks(
                &person,
                &private_root,
                &snapshot,
                Some(successor_items),
                Some(successor_summary),
                |_| {},
                |_| {},
                (
                    |_| {},
                    |_| panic!("crash immediately after the full old-generation scrub"),
                ),
            )
            .unwrap();
        }));
        assert!(crashed.is_err());
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &person).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(person.exists());

        fs::write(
            &source,
            meeting_markdown("Normal", Some("restricted"), "FULL-SCRUB-REVOCATION-CANARY"),
        )
        .unwrap();
        recover_para_transactions(&people, &private_root, Some(&config)).unwrap();

        assert!(!person.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
        assert!(
            !all_text_unfiltered(&config.knowledge.path).contains("FULL-SCRUB-REVOCATION-CANARY")
        );
        assert!(!all_text_unfiltered(&private_root).contains("FULL-SCRUB-REVOCATION-CANARY"));
        assert_recyclable_terminal_receipt(&private_root, "alex-kim");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn assert_baseline_deleted_publication_revalidates_after_revocation(recreation: bool) {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "BASELINE-DELETED-REVOCATION-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();
        let people = config.knowledge.path.join("areas/people");
        let existing = inspect_para_person(&people.join("alex-kim")).unwrap();
        let expected = recyclable_para_expected(
            existing.items.bytes.clone(),
            existing.summary.as_ref().unwrap().bytes.clone(),
        )
        .unwrap();
        let intended = para_successor_proof(&expected).unwrap();
        let private_root = para_private_root(&config).unwrap();
        let target = people.join(if recreation {
            "recreation-post-publication"
        } else {
            "creation-post-publication"
        });
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        if recreation {
            ensure_recyclable_para_members(&parked).unwrap();
        }
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            intended.clone(),
            Some(intended.clone()),
            true,
            recreation,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let successor = fill_recyclable_para_tombstone(&slot, &expected).unwrap();
        move_para_directory_to_claim(&slot, &target, &successor.directory)
            .unwrap()
            .unwrap();
        if recreation {
            let parked_tombstone = attest_recyclable_para_tombstone(&parked).unwrap();
            move_para_directory_to_claim(&parked, &slot, &parked_tombstone)
                .unwrap()
                .unwrap();
        } else {
            ensure_recyclable_para_members(&slot).unwrap();
        }
        attest_recyclable_para_tombstone(&slot).unwrap();

        fs::write(
            &source,
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "BASELINE-DELETED-REVOCATION-CANARY",
            ),
        )
        .unwrap();
        recover_one_para_transaction(&people, &private_root, Some(&config), &mut active).unwrap();

        assert!(!target.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
        assert!(!all_text_unfiltered(&private_root).contains("BASELINE-DELETED-REVOCATION-CANARY"));
        assert_recyclable_terminal_receipt(
            &private_root,
            target.file_name().unwrap().to_str().unwrap(),
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn initial_creation_post_publication_revocation_is_retracted() {
        assert_baseline_deleted_publication_revalidates_after_revocation(false);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recreation_post_publication_revocation_is_retracted() {
        assert_baseline_deleted_publication_revalidates_after_revocation(true);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[derive(Clone, Copy, Debug)]
    enum RevokedCleanupBoundary {
        BeforeExchange,
        AfterExchange,
        AfterItemsScrub,
        AfterFullScrub,
        AfterPark,
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn assert_revoked_cleanup_boundary_converges(
        baseline_deleted: bool,
        recreation: bool,
        boundary: RevokedCleanupBoundary,
    ) {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "REVOCATION-STATE-TABLE-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();
        let people = config.knowledge.path.join("areas/people");
        let original = people.join("alex-kim");
        let original_snapshot = inspect_para_person(&original).unwrap();
        let old_items = original_snapshot.items.bytes.clone();
        let old_summary = original_snapshot.summary.as_ref().unwrap().bytes.clone();
        fs::remove_dir_all(&original).unwrap();

        let private_root = para_private_root(&config).unwrap();
        let mode = if !baseline_deleted {
            "replacement"
        } else if recreation {
            "recreation"
        } else {
            "creation"
        };
        let target = people.join(format!("{mode}-{boundary:?}").to_lowercase());
        let mut intended_summary = old_summary.clone();
        intended_summary.extend_from_slice(b"\n<!-- intended-state-table -->\n");
        let intended_expected =
            recyclable_para_expected(old_items.clone(), intended_summary).unwrap();
        let intended = para_successor_proof(&intended_expected).unwrap();
        let old = if baseline_deleted {
            intended.clone()
        } else {
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("items.json"), &old_items).unwrap();
            fs::write(target.join("summary.md"), &old_summary).unwrap();
            para_snapshot_proof(&inspect_para_person(&target).unwrap()).unwrap()
        };
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        if recreation {
            ensure_recyclable_para_members(&parked).unwrap();
        }
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            old.clone(),
            Some(intended.clone()),
            baseline_deleted,
            recreation,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let successor = fill_recyclable_para_tombstone(&slot, &intended_expected).unwrap();
        if baseline_deleted {
            move_para_directory_to_claim(&slot, &target, &successor.directory)
                .unwrap()
                .unwrap();
            if recreation {
                let parked_tombstone = attest_recyclable_para_tombstone(&parked).unwrap();
                move_para_directory_to_claim(&parked, &slot, &parked_tombstone)
                    .unwrap()
                    .unwrap();
            } else {
                ensure_recyclable_para_members(&slot).unwrap();
            }
        } else {
            exchange_recyclable_para_generations(&people, &target, &private_root, &slot).unwrap();
            scrub_recyclable_para_generation(&slot, &old).unwrap();
        }
        attest_recyclable_para_tombstone(&slot).unwrap();

        if !matches!(boundary, RevokedCleanupBoundary::BeforeExchange) {
            exchange_recyclable_para_generations(&people, &target, &private_root, &slot).unwrap();
        }
        if matches!(boundary, RevokedCleanupBoundary::AfterItemsScrub) {
            let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scrub_recyclable_para_generation_with_hooks(
                    &slot,
                    &intended,
                    |_| panic!("state-table crash after intended items scrub"),
                    |_| {},
                )
                .unwrap();
            }));
            assert!(crashed.is_err());
        } else if matches!(
            boundary,
            RevokedCleanupBoundary::AfterFullScrub | RevokedCleanupBoundary::AfterPark
        ) {
            scrub_recyclable_para_generation(&slot, &intended).unwrap();
        }
        if matches!(boundary, RevokedCleanupBoundary::AfterPark) {
            park_recyclable_public_tombstone(&target, &parked).unwrap();
        }

        fs::write(
            &source,
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "REVOCATION-STATE-TABLE-CANARY",
            ),
        )
        .unwrap();
        recover_one_para_transaction(&people, &private_root, Some(&config), &mut active).unwrap();
        recover_para_transactions(&people, &private_root, Some(&config)).unwrap();

        assert!(!target.exists(), "{mode} {boundary:?} remained public");
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
        if recreation && matches!(boundary, RevokedCleanupBoundary::AfterPark) {
            let (a, b) = recyclable_para_transaction_paths(
                &private_root,
                target.file_name().unwrap().to_str().unwrap(),
            );
            let reads = [
                read_recyclable_para_record(&a).unwrap(),
                read_recyclable_para_record(&b).unwrap(),
            ];
            let baseline = reads
                .iter()
                .find_map(|read| match read {
                    RecyclableParaRecordRead::Valid(record, _)
                        if record.journal_state == Some(RecyclableParaJournalState::Baseline) =>
                    {
                        Some(record.as_ref())
                    }
                    _ => None,
                })
                .expect("the exact prior deletion baseline must survive safe abort");
            assert_eq!(baseline.sequence, 0);
            assert!(baseline.baseline_deleted);
            assert!(baseline.baseline_parked);
            attest_recyclable_journal_layout(&people, &private_root, baseline).unwrap();
            assert_eq!(
                reads
                    .iter()
                    .filter(|read| matches!(read, RecyclableParaRecordRead::Empty))
                    .count(),
                1,
                "the Active receipt must be exactly retired"
            );
        } else {
            assert_recyclable_terminal_receipt(
                &private_root,
                target.file_name().unwrap().to_str().unwrap(),
            );
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn revoked_cleanup_state_table_converges_at_every_boundary() {
        for boundary in [
            RevokedCleanupBoundary::BeforeExchange,
            RevokedCleanupBoundary::AfterExchange,
            RevokedCleanupBoundary::AfterItemsScrub,
            RevokedCleanupBoundary::AfterFullScrub,
            RevokedCleanupBoundary::AfterPark,
        ] {
            assert_revoked_cleanup_boundary_converges(false, false, boundary);
            assert_revoked_cleanup_boundary_converges(true, false, boundary);
            assert_revoked_cleanup_boundary_converges(true, true, boundary);
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn initial_creation_repairs_partial_zero_slot_before_revoked_cleanup() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "PARTIAL-CREATION-REVOCATION-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();
        let people = config.knowledge.path.join("areas/people");
        let original = people.join("alex-kim");
        let snapshot = inspect_para_person(&original).unwrap();
        let expected = recyclable_para_expected(
            snapshot.items.bytes.clone(),
            snapshot.summary.as_ref().unwrap().bytes.clone(),
        )
        .unwrap();
        fs::remove_dir_all(&original).unwrap();
        let private_root = para_private_root(&config).unwrap();
        let target = people.join("partial-created-slot");
        let intended = para_successor_proof(&expected).unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            intended.clone(),
            Some(intended.clone()),
            true,
            false,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        let successor = fill_recyclable_para_tombstone(&slot, &expected).unwrap();
        move_para_directory_to_claim(&slot, &target, &successor.directory)
            .unwrap()
            .unwrap();
        fs::create_dir(&slot).unwrap();
        fs::write(slot.join("items.json"), b"").unwrap();

        fs::write(
            &source,
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "PARTIAL-CREATION-REVOCATION-CANARY",
            ),
        )
        .unwrap();

        recover_one_para_transaction(&people, &private_root, Some(&config), &mut active).unwrap();

        assert!(!target.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_partial_slot_refill_revalidates_and_deletes_newly_revoked_public_old_generation() {
        let meetings = TempDir::new().unwrap();
        let knowledge = TempDir::new().unwrap();
        let source = meetings.path().join("normal.md");
        fs::write(
            &source,
            meeting_markdown("Normal", None, "PARTIAL-SLOT-REVOCATION-CANARY"),
        )
        .unwrap();
        let config = knowledge_config(&meetings, &knowledge, "para");
        ingest_file(&source, &config).unwrap();

        let people = config.knowledge.path.join("areas/people");
        let person = people.join("alex-kim");
        let private_root = para_private_root(&config).unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let old = para_snapshot_proof(&snapshot).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &person).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        let slot_directory = attest_recyclable_para_tombstone(&slot).unwrap();
        let intended = recyclable_para_expected(
            snapshot.items.bytes.clone(),
            b"# Alex Kim\n\n- intended-but-torn\n".to_vec(),
        )
        .unwrap();
        let mut transaction = begin_recyclable_para_transaction(
            &people,
            &private_root,
            ParaTransactionManifest {
                schema: 2,
                target_name: "alex-kim".into(),
                stage_name: Some(slot.file_name().unwrap().to_string_lossy().into_owned()),
                capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
                old,
                intended: Some(para_successor_proof(&intended).unwrap()),
                slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                    .map(|value| value.0),
                slot_items_identity: qmd_file_identity_and_links(
                    &open_regular_file_no_follow(&slot.join("items.json")).unwrap(),
                )
                .map(|value| value.0),
                slot_summary_identity: qmd_file_identity_and_links(
                    &open_regular_file_no_follow(&slot.join("summary.md")).unwrap(),
                )
                .map(|value| value.0),
                sequence: 0,
                journal_state: Some(RecyclableParaJournalState::Active),
                baseline_deleted: false,
                baseline_parked: false,
                prior_sequence: None,
            },
        )
        .unwrap();
        fs::write(slot.join("items.json"), b"{\"partial\":").unwrap();
        fs::write(
            &source,
            meeting_markdown(
                "Normal",
                Some("restricted"),
                "PARTIAL-SLOT-REVOCATION-CANARY",
            ),
        )
        .unwrap();

        recover_one_para_transaction(&people, &private_root, Some(&config), &mut transaction)
            .unwrap();

        assert!(!person.exists());
        assert!(
            !all_text_unfiltered(&config.knowledge.path).contains("PARTIAL-SLOT-REVOCATION-CANARY")
        );
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
        assert!(fs::read(transaction.path).unwrap().is_empty());
        assert_recyclable_terminal_receipt(&private_root, "alex-kim");
    }

    #[cfg(unix)]
    #[test]
    fn para_capture_cleanup_never_deletes_a_replacement_directory() {
        let root = TempDir::new().unwrap();
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        prepare_para_private_root(&private_root).unwrap();
        let capture = private_root.join(format!("{PARA_PERSON_CAPTURE_PREFIX}exact"));
        fs::create_dir(&capture).unwrap();
        fs::write(capture.join("items.json"), b"OLD-CAPTURE").unwrap();
        let proof = ParaGenerationProof {
            entry_names: vec!["items.json".into()],
            items: para_file_proof(b"OLD-CAPTURE").unwrap(),
            summary: None,
        };
        let displaced = private_root.join("displaced-exact-capture");

        let error =
            remove_owner_private_para_capture_with_hook(&private_root, &capture, &proof, || {
                fs::rename(&capture, &displaced).unwrap();
                fs::create_dir(&capture).unwrap();
            })
            .expect_err("a replacement capture name must never be pathname-deleted");

        assert!(!error.to_string().is_empty());
        assert!(capture.is_dir(), "the replacement winner must survive");
        assert!(
            !all_text_unfiltered(&private_root).contains("OLD-CAPTURE"),
            "already-proven old capture bytes must remain retired even when the directory name is replaced"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn assert_recyclable_terminal_receipt(private_root: &Path, target: &str) {
        let (a, b) = recyclable_para_transaction_paths(private_root, target);
        let reads = [
            read_recyclable_para_record(&a).unwrap(),
            read_recyclable_para_record(&b).unwrap(),
        ];
        assert_eq!(
            reads
                .iter()
                .filter(|read| matches!(
                    read,
                    RecyclableParaRecordRead::Valid(record, _)
                        if record.journal_state == Some(RecyclableParaJournalState::Completed)
                ))
                .count(),
            1
        );
        assert_eq!(
            reads
                .iter()
                .filter(|read| matches!(read, RecyclableParaRecordRead::Empty))
                .count(),
            1
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn assert_para_recovery_after_member_retirement_crash(crash_after_summary: bool) {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        prepare_para_private_root(&private_root).unwrap();
        let directory = people.join("member-crash-safe");
        fs::create_dir_all(&directory).unwrap();
        let old_items = serde_json::to_vec_pretty(&serde_json::json!([
            {"id":"old","fact":"OLD","source":"source-a","status":"active"}
        ]))
        .unwrap();
        let old_summary = b"# Member Crash Safe\n\n- OLD\n".to_vec();
        fs::write(directory.join("items.json"), &old_items).unwrap();
        fs::write(directory.join("summary.md"), &old_summary).unwrap();
        let snapshot = inspect_para_person(&directory).unwrap();
        let new_items = serde_json::to_vec_pretty(&serde_json::json!([
            {"id":"new","fact":"NEW","source":"source-b","status":"active"}
        ]))
        .unwrap();
        let new_summary = b"# Member Crash Safe\n\n## Context\n\n- NEW\n".to_vec();

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            replace_para_person_generation_with_retirement_hooks(
                &directory,
                &private_root,
                &snapshot,
                Some(new_items.clone()),
                Some(new_summary.clone()),
                |_| {},
                |_| {},
                (
                    |_| {
                        if !crash_after_summary {
                            panic!("simulated process loss after items.json retention proof");
                        }
                    },
                    |_| {
                        if crash_after_summary {
                            panic!("simulated process loss after summary.md retention proof");
                        }
                    },
                ),
            )
            .unwrap();
        }));
        assert!(crashed.is_err());
        assert_eq!(fs::read(directory.join("items.json")).unwrap(), new_items);
        assert_eq!(fs::read(directory.join("summary.md")).unwrap(), new_summary);
        assert!(fs::read_dir(&private_root).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(PARA_PERSON_TRANSACTION_PREFIX)
                && fs::read(entry.path()).is_ok_and(|bytes| !bytes.is_empty())
        }));

        recover_para_transactions(&people, &private_root, None).unwrap();
        assert_eq!(fs::read(directory.join("items.json")).unwrap(), new_items);
        assert_eq!(fs::read(directory.join("summary.md")).unwrap(), new_summary);
        assert!(fs::read_dir(&private_root).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(PARA_PERSON_CAPTURE_PREFIX)
        }));
        assert_recyclable_terminal_receipt(&private_root, "member-crash-safe");
        assert!(fs::read_dir(&private_root).unwrap().flatten().all(|entry| {
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(PARA_PERSON_COMPLETED_TRANSACTION_PREFIX)
        }));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_crash_after_items_retention_proof_recovers_idempotently() {
        assert_para_recovery_after_member_retirement_crash(false);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn para_crash_after_summary_retention_proof_recovers_idempotently() {
        assert_para_recovery_after_member_retirement_crash(true);
    }

    #[test]
    fn para_journal_is_retained_when_directory_metadata_sync_fails() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("people");
        drop(crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&people).unwrap());
        let proof = ParaGenerationProof {
            entry_names: vec!["items.json".to_string()],
            items: para_file_proof(b"old").unwrap(),
            summary: None,
        };
        let mut transaction = begin_para_transaction(
            &people,
            ParaTransactionManifest {
                schema: 1,
                target_name: "durability".to_string(),
                stage_name: None,
                capture_name: format!("{PARA_PERSON_CAPTURE_PREFIX}durability"),
                old: proof,
                intended: None,
                slot_directory_identity: None,
                slot_items_identity: None,
                slot_summary_identity: None,
                sequence: 0,
                journal_state: None,
                baseline_deleted: false,
                baseline_parked: false,
                prior_sequence: None,
            },
        )
        .unwrap();
        let before = fs::read(&transaction.path).unwrap();

        let error = finish_para_transaction_with_metadata_sync(&mut transaction, &people, |_| {
            Err("simulated unsupported Windows directory flush".into())
        })
        .expect_err("an unflushed rename boundary must retain its journal");

        assert!(error.to_string().contains("unsupported Windows"));
        assert_eq!(fs::read(&transaction.path).unwrap(), before);
        assert!(!before.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn para_journal_rejects_an_existing_leaf_without_the_private_dacl() {
        let root = TempDir::new().unwrap();
        let private_root = root.path().join("private");
        let boundary =
            crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&private_root).unwrap();
        let manifest = ParaTransactionManifest {
            schema: 1,
            target_name: "planted".to_string(),
            stage_name: None,
            capture_name: format!("{PARA_PERSON_CAPTURE_PREFIX}planted"),
            old: ParaGenerationProof {
                entry_names: vec!["items.json".to_string()],
                items: para_file_proof(b"old").unwrap(),
                summary: None,
            },
            intended: None,
            slot_directory_identity: None,
            slot_items_identity: None,
            slot_summary_identity: None,
            sequence: 0,
            journal_state: None,
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        };
        let path = para_transaction_path(&private_root, &manifest.target_name);
        File::create(&path).unwrap();

        let error = match begin_para_transaction(&private_root, manifest) {
            Ok(_) => panic!("a planted journal without the exact private DACL must fail closed"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("DACL"),
            "unexpected journal denial: {error}"
        );
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
        drop(boundary);
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn completed_schema_one_transaction_migrates_to_exact_zero_tombstones() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let person = people.join("legacy");
        let capture = private_root.join(format!("{PARA_PERSON_CAPTURE_PREFIX}legacy"));
        fs::create_dir_all(&person).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        create_recyclable_para_directory(&capture).unwrap();
        ensure_recyclable_para_members(&capture).unwrap();
        let old_items = b"LEGACY-PRIVATE-ITEMS".to_vec();
        let old_summary = b"LEGACY-PRIVATE-SUMMARY".to_vec();
        let intended_items = b"PUBLIC-INTENDED-ITEMS".to_vec();
        let intended_summary = b"PUBLIC-INTENDED-SUMMARY".to_vec();
        fs::write(capture.join("items.json"), &old_items).unwrap();
        fs::write(capture.join("summary.md"), &old_summary).unwrap();
        fs::write(person.join("items.json"), &intended_items).unwrap();
        fs::write(person.join("summary.md"), &intended_summary).unwrap();
        let manifest = ParaTransactionManifest {
            schema: 1,
            target_name: "legacy".into(),
            stage_name: None,
            capture_name: capture.file_name().unwrap().to_string_lossy().into_owned(),
            old: ParaGenerationProof {
                entry_names: vec!["items.json".into(), "summary.md".into()],
                items: para_file_proof(&old_items).unwrap(),
                summary: Some(para_file_proof(&old_summary).unwrap()),
            },
            intended: Some(ParaGenerationProof {
                entry_names: vec!["items.json".into(), "summary.md".into()],
                items: para_file_proof(&intended_items).unwrap(),
                summary: Some(para_file_proof(&intended_summary).unwrap()),
            }),
            slot_directory_identity: None,
            slot_items_identity: None,
            slot_summary_identity: None,
            sequence: 0,
            journal_state: None,
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        };
        let transaction = begin_para_transaction(&private_root, manifest).unwrap();
        let completed =
            private_root.join(format!("{PARA_PERSON_COMPLETED_TRANSACTION_PREFIX}legacy"));
        fs::rename(&transaction.path, &completed).unwrap();
        drop(transaction);

        recover_para_transactions(&people, &private_root, None).unwrap();

        assert_eq!(fs::read(person.join("items.json")).unwrap(), intended_items);
        assert_eq!(
            fs::read(person.join("summary.md")).unwrap(),
            intended_summary
        );
        assert!(fs::read(capture.join("items.json")).unwrap().is_empty());
        assert!(fs::read(capture.join("summary.md")).unwrap().is_empty());
        assert!(fs::read(&completed).unwrap().is_empty());
        recover_para_transactions(&people, &private_root, None).unwrap();
        assert!(fs::read(&completed).unwrap().is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn recyclable_test_manifest(
        private_root: &Path,
        target: &Path,
        old: ParaGenerationProof,
        intended: Option<ParaGenerationProof>,
        baseline_deleted: bool,
        baseline_parked: bool,
    ) -> ParaTransactionManifest {
        let (slot, _) = recyclable_para_generation_paths(private_root, target).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        let slot_directory = attest_recyclable_para_tombstone(&slot).unwrap();
        ParaTransactionManifest {
            schema: 2,
            target_name: target.file_name().unwrap().to_string_lossy().into_owned(),
            stage_name: intended
                .as_ref()
                .map(|_| slot.file_name().unwrap().to_string_lossy().into_owned()),
            capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
            old,
            intended,
            slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                .map(|value| value.0),
            slot_items_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("items.json")).unwrap(),
            )
            .map(|value| value.0),
            slot_summary_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("summary.md")).unwrap(),
            )
            .map(|value| value.0),
            sequence: 0,
            journal_state: Some(RecyclableParaJournalState::Active),
            baseline_deleted,
            baseline_parked,
            prior_sequence: None,
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn replacement_active_fsync_before_slot_write_recovers_as_exact_abort() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("replacement-prewrite");
        fs::create_dir_all(&target).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(target.join("items.json"), b"OLD-ITEMS").unwrap();
        fs::write(target.join("summary.md"), b"OLD-SUMMARY").unwrap();
        let old = para_snapshot_proof(&inspect_para_person(&target).unwrap()).unwrap();
        let intended = para_successor_proof(
            &recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap(),
        )
        .unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            old.clone(),
            Some(intended),
            false,
            false,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        assert!(!fs::read(&active.path).unwrap().is_empty());

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        assert!(fs::read(&active.path).unwrap().is_empty());
        inspect_para_generation_with_proof(&target, &old).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(!parked.exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn initial_creation_active_fsync_before_slot_write_recovers_as_exact_abort() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("creation-prewrite");
        fs::create_dir_all(&people).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        let intended = para_successor_proof(
            &recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap(),
        )
        .unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            intended.clone(),
            Some(intended),
            true,
            false,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        assert!(fs::read(&active.path).unwrap().is_empty());
        assert!(!target.exists());
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(!parked.exists());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recreation_active_fsync_before_slot_write_recovers_as_exact_abort() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("recreation-prewrite");
        fs::create_dir_all(&people).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        let intended = para_successor_proof(
            &recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap(),
        )
        .unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            intended.clone(),
            Some(intended),
            true,
            true,
        );
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        ensure_recyclable_para_members(&parked).unwrap();
        let (journal_a, _) =
            recyclable_para_transaction_paths(&private_root, "recreation-prewrite");
        let mut deleted_receipt = manifest.clone();
        deleted_receipt.intended = None;
        deleted_receipt.sequence = 0;
        deleted_receipt.journal_state = Some(RecyclableParaJournalState::Completed);
        deleted_receipt.baseline_deleted = false;
        deleted_receipt.baseline_parked = false;
        write_recyclable_para_record(&journal_a, &deleted_receipt, false).unwrap();
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        assert!(fs::read(&active.path).unwrap().is_empty());
        assert!(!target.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn recreation_staged_intended_is_not_misclassified_as_revoked_cleanup() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("recreation-staged-intended");
        fs::create_dir_all(&people).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        let expected =
            recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap();
        let intended = para_successor_proof(&expected).unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            intended.clone(),
            Some(intended.clone()),
            true,
            true,
        );
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        ensure_recyclable_para_members(&parked).unwrap();
        let (journal_a, _) =
            recyclable_para_transaction_paths(&private_root, "recreation-staged-intended");
        let mut deleted_receipt = manifest.clone();
        deleted_receipt.intended = None;
        deleted_receipt.sequence = 0;
        deleted_receipt.journal_state = Some(RecyclableParaJournalState::Completed);
        deleted_receipt.baseline_deleted = false;
        deleted_receipt.baseline_parked = false;
        write_recyclable_para_record(&journal_a, &deleted_receipt, false).unwrap();
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        fill_recyclable_para_tombstone(&slot, &expected).unwrap();

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        inspect_para_generation_with_proof(&target, &intended).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_path_is_absent(&parked).unwrap();
        assert_recyclable_terminal_receipt(&private_root, "recreation-staged-intended");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn deletion_recovers_after_items_member_scrub_sync() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("delete-member-crash");
        fs::create_dir_all(&target).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(target.join("items.json"), b"OLD-ITEMS").unwrap();
        fs::write(target.join("summary.md"), b"OLD-SUMMARY").unwrap();
        let old = para_snapshot_proof(&inspect_para_person(&target).unwrap()).unwrap();
        let manifest =
            recyclable_test_manifest(&private_root, &target, old.clone(), None, false, false);
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        exchange_recyclable_para_generations(&people, &target, &private_root, &slot).unwrap();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scrub_recyclable_para_generation_with_hooks(
                &slot,
                &old,
                |_| panic!("crash after deletion items scrub sync"),
                |_| {},
            )
            .unwrap();
        }));
        assert!(crashed.is_err());

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        assert!(!target.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn authorization_failure_recovers_after_intended_items_scrub_sync() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("authorize-member-crash");
        fs::create_dir_all(&target).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(target.join("items.json"), b"OLD-ITEMS").unwrap();
        fs::write(target.join("summary.md"), b"OLD-SUMMARY").unwrap();
        let old = para_snapshot_proof(&inspect_para_person(&target).unwrap()).unwrap();
        let expected =
            recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap();
        let intended = para_successor_proof(&expected).unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            old.clone(),
            Some(intended.clone()),
            false,
            false,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        fill_recyclable_para_tombstone(&slot, &expected).unwrap();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scrub_recyclable_para_generation_with_hooks(
                &slot,
                &intended,
                |_| panic!("crash after authorization-failure intended items scrub sync"),
                |_| {},
            )
            .unwrap();
        }));
        assert!(crashed.is_err());

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        inspect_para_generation_with_proof(&target, &old).unwrap();
        attest_recyclable_para_tombstone(&slot).unwrap();
        assert!(!parked.exists());
        assert!(fs::read(active.path).unwrap().is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn post_publication_revocation_recovers_after_intended_items_scrub_sync() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let target = people.join("revoke-member-crash");
        fs::create_dir_all(&target).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(target.join("items.json"), b"OLD-ITEMS").unwrap();
        fs::write(target.join("summary.md"), b"OLD-SUMMARY").unwrap();
        let old = para_snapshot_proof(&inspect_para_person(&target).unwrap()).unwrap();
        let expected =
            recyclable_para_expected(b"NEW-ITEMS".to_vec(), b"NEW-SUMMARY".to_vec()).unwrap();
        let intended = para_successor_proof(&expected).unwrap();
        let manifest = recyclable_test_manifest(
            &private_root,
            &target,
            old.clone(),
            Some(intended.clone()),
            false,
            false,
        );
        let mut active =
            begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        let (slot, parked) = recyclable_para_generation_paths(&private_root, &target).unwrap();
        fill_recyclable_para_tombstone(&slot, &expected).unwrap();
        exchange_recyclable_para_generations(&people, &target, &private_root, &slot).unwrap();
        scrub_recyclable_para_generation(&slot, &old).unwrap();
        exchange_recyclable_para_generations(&people, &target, &private_root, &slot).unwrap();
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            scrub_recyclable_para_generation_with_hooks(
                &slot,
                &intended,
                |_| panic!("crash after revoked intended items scrub sync"),
                |_| {},
            )
            .unwrap();
        }));
        assert!(crashed.is_err());

        recover_one_para_transaction(&people, &private_root, None, &mut active).unwrap();

        assert!(!target.exists());
        attest_recyclable_para_tombstone(&slot).unwrap();
        attest_recyclable_para_tombstone(&parked).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn schema_two_rejects_a_mismatched_target_receipt_in_its_exact_journal_namespace() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let person = people.join("intended-target");
        fs::create_dir_all(&person).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        fs::write(person.join("items.json"), b"TARGET-ITEMS").unwrap();
        fs::write(person.join("summary.md"), b"TARGET-SUMMARY").unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let old = para_snapshot_proof(&snapshot).unwrap();
        let (slot, _) = recyclable_para_generation_paths(&private_root, &person).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        let slot_directory = attest_recyclable_para_tombstone(&slot).unwrap();
        let (journal_a, _) = recyclable_para_transaction_paths(&private_root, "intended-target");
        assert_ne!(
            recyclable_para_transaction_paths(&private_root, "intended-target"),
            recyclable_para_transaction_paths(&private_root, "collision-peer"),
            "schema-2 journal names must use the full collision-resistant target digest"
        );
        let alien = ParaTransactionManifest {
            schema: 2,
            target_name: "collision-peer".into(),
            stage_name: None,
            capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
            old: old.clone(),
            intended: None,
            slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                .map(|value| value.0),
            slot_items_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("items.json")).unwrap(),
            )
            .map(|value| value.0),
            slot_summary_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("summary.md")).unwrap(),
            )
            .map(|value| value.0),
            sequence: 0,
            journal_state: Some(RecyclableParaJournalState::Baseline),
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        };
        write_recyclable_para_record(&journal_a, &alien, false).unwrap();
        let alien_bytes = fs::read(&journal_a).unwrap();
        let incoming = ParaTransactionManifest {
            target_name: "intended-target".into(),
            capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
            old,
            ..alien.clone()
        };

        let error = match begin_recyclable_para_transaction(&people, &private_root, incoming) {
            Ok(_) => panic!("a colliding or misplaced receipt authorized another target"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("another target"));
        assert_eq!(fs::read(&journal_a).unwrap(), alien_bytes);
        assert_eq!(
            fs::read(person.join("items.json")).unwrap(),
            b"TARGET-ITEMS"
        );
        attest_recyclable_para_tombstone(&slot).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn torn_recyclable_journal_is_exactly_reset_before_any_generation_mutation() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let person = people.join("stable");
        fs::create_dir_all(&person).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        let public_items = serde_json::to_vec_pretty(&serde_json::json!([{
            "id":"stable","fact":"PUBLIC-STABLE","source":"source","status":"active"
        }]))
        .unwrap();
        fs::write(person.join("items.json"), &public_items).unwrap();
        fs::write(person.join("summary.md"), b"PUBLIC-SUMMARY").unwrap();
        let private_canary = private_root.join("unrelated-private-canary");
        fs::write(&private_canary, b"PRIVATE-STABLE").unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let (slot, _) = recyclable_para_generation_paths(&private_root, &person).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        let slot_directory = attest_recyclable_para_tombstone(&slot).unwrap();
        let expected =
            recyclable_para_expected(public_items.clone(), b"PUBLIC-SUMMARY-NEXT".to_vec())
                .unwrap();
        let manifest = ParaTransactionManifest {
            schema: 2,
            target_name: "stable".into(),
            stage_name: Some(slot.file_name().unwrap().to_string_lossy().into_owned()),
            capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
            old: para_snapshot_proof(&snapshot).unwrap(),
            intended: Some(para_successor_proof(&expected).unwrap()),
            slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                .map(|value| value.0),
            slot_items_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("items.json")).unwrap(),
            )
            .map(|value| value.0),
            slot_summary_identity: qmd_file_identity_and_links(
                &open_regular_file_no_follow(&slot.join("summary.md")).unwrap(),
            )
            .map(|value| value.0),
            sequence: 0,
            journal_state: Some(RecyclableParaJournalState::Active),
            baseline_deleted: false,
            baseline_parked: false,
            prior_sequence: None,
        };
        let transaction =
            begin_recyclable_para_transaction(&people, &private_root, manifest.clone()).unwrap();
        fs::write(
            &transaction.path,
            b"{\"schema\":2,\"target_name\":\"stable\"",
        )
        .unwrap();

        recover_para_transactions(&people, &private_root, None).unwrap();

        assert_eq!(fs::read(person.join("items.json")).unwrap(), public_items);
        assert_eq!(
            fs::read(person.join("summary.md")).unwrap(),
            b"PUBLIC-SUMMARY"
        );
        assert_eq!(fs::read(&private_canary).unwrap(), b"PRIVATE-STABLE");
        assert_eq!(fs::read(&transaction.path).unwrap(), b"");

        let retry = begin_recyclable_para_transaction(&people, &private_root, manifest).unwrap();
        assert!(!fs::read(retry.path).unwrap().is_empty());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn corrupted_active_recyclable_journal_after_exchange_is_never_reset_as_pre_mutation() {
        let root = TempDir::new().unwrap();
        let people = root.path().join("areas/people");
        let private_root = root.path().join(PARA_PRIVATE_ROOT);
        let person = people.join("hostile");
        fs::create_dir_all(&person).unwrap();
        prepare_para_private_root(&private_root).unwrap();
        let old_items = serde_json::to_vec_pretty(&serde_json::json!([{
            "id":"old","fact":"OLD-JOURNAL-CANARY","source":"source","status":"active"
        }]))
        .unwrap();
        fs::write(person.join("items.json"), &old_items).unwrap();
        fs::write(person.join("summary.md"), b"# Hostile\n\n- OLD\n").unwrap();
        let snapshot = inspect_para_person(&person).unwrap();
        let old = para_snapshot_proof(&snapshot).unwrap();
        let (slot, _) = recyclable_para_generation_paths(&private_root, &person).unwrap();
        ensure_recyclable_para_members(&slot).unwrap();
        let slot_directory = attest_recyclable_para_tombstone(&slot).unwrap();
        let expected = recyclable_para_expected(
            serde_json::to_vec_pretty(&serde_json::json!([{
                "id":"new","fact":"NEW-JOURNAL-CANARY","source":"source","status":"active"
            }]))
            .unwrap(),
            b"# Hostile\n\n- NEW\n".to_vec(),
        )
        .unwrap();
        let intended = para_successor_proof(&expected).unwrap();
        let transaction = begin_recyclable_para_transaction(
            &people,
            &private_root,
            ParaTransactionManifest {
                schema: 2,
                target_name: "hostile".into(),
                stage_name: Some(slot.file_name().unwrap().to_string_lossy().into_owned()),
                capture_name: slot.file_name().unwrap().to_string_lossy().into_owned(),
                old: old.clone(),
                intended: Some(intended.clone()),
                slot_directory_identity: qmd_file_identity_and_links(&slot_directory)
                    .map(|value| value.0),
                slot_items_identity: qmd_file_identity_and_links(
                    &open_regular_file_no_follow(&slot.join("items.json")).unwrap(),
                )
                .map(|value| value.0),
                slot_summary_identity: qmd_file_identity_and_links(
                    &open_regular_file_no_follow(&slot.join("summary.md")).unwrap(),
                )
                .map(|value| value.0),
                sequence: 0,
                journal_state: Some(RecyclableParaJournalState::Active),
                baseline_deleted: false,
                baseline_parked: false,
                prior_sequence: None,
            },
        )
        .unwrap();
        fill_recyclable_para_tombstone(&slot, &expected).unwrap();
        exchange_recyclable_para_generations(&people, &person, &private_root, &slot).unwrap();
        fs::write(&transaction.path, b"{\"schema\":2,\"corrupted\":true").unwrap();

        let error = recover_para_transactions(&people, &private_root, None)
            .expect_err("an advanced layout cannot be justified by the prior terminal receipt");

        assert!(!error.to_string().is_empty());
        assert!(!fs::read(&transaction.path).unwrap().is_empty());
        inspect_para_generation_with_proof(&person, &intended).unwrap();
        inspect_para_generation_with_proof(&slot, &old).unwrap();
        // Keep the retained active handle live through the assertions so the
        // test also exercises corruption of the exact journal inode.
        transaction.file.sync_all().unwrap();
    }

    #[test]
    fn para_generation_capacity_reserves_complete_failure_envelope() {
        let root = TempDir::new().unwrap();
        for index in 0..61 {
            fs::create_dir(
                root.path()
                    .join(format!("{PARA_PERSON_FAILED_PREFIX}{index:04}")),
            )
            .unwrap();
        }
        require_para_generation_capacity(root.path(), 3).unwrap();
        fs::create_dir(root.path().join(format!("{PARA_PERSON_FAILED_PREFIX}0061"))).unwrap();
        assert!(require_para_generation_capacity(root.path(), 3).is_err());
        require_para_generation_capacity(root.path(), 2).unwrap();
        fs::create_dir(root.path().join(format!("{PARA_PERSON_FAILED_PREFIX}0062"))).unwrap();
        assert!(require_para_generation_capacity(root.path(), 2).is_err());
    }

    #[test]
    fn para_claim_destination_swap_never_rolls_a_winner_public() {
        let root = TempDir::new().unwrap();
        let source = root.path().join("person");
        let claim = root
            .path()
            .join(format!("{PARA_PERSON_CAPTURE_PREFIX}claim"));
        let displaced = root
            .path()
            .join(format!("{PARA_PERSON_CAPTURE_PREFIX}exact"));
        fs::create_dir(&source).unwrap();
        fs::write(source.join("items.json"), b"exact").unwrap();
        let expected = qmd_open_directory_no_follow(&source).unwrap();

        let error = move_para_directory_to_claim_with_hook(&source, &claim, &expected, |claimed| {
            fs::rename(claimed, &displaced).unwrap();
            fs::create_dir(claimed).unwrap();
            fs::write(claimed.join("items.json"), b"winner").unwrap();
        })
        .expect_err("a swapped claim must fail closed");

        assert!(error.to_string().contains("rollback was refused"));
        assert!(!source.exists());
        assert_eq!(fs::read(displaced.join("items.json")).unwrap(), b"exact");
        assert_eq!(fs::read(claim.join("items.json")).unwrap(), b"winner");
    }

    #[test]
    fn para_mutable_directory_rescans_fail_at_the_bound() {
        let root = TempDir::new().unwrap();
        for index in 0..=MAX_PARA_RECONCILIATION_ITEMS {
            fs::write(root.path().join(format!("entry-{index:04}")), b"x").unwrap();
        }
        assert!(bounded_para_entry_names(root.path(), MAX_PARA_RECONCILIATION_ITEMS).is_err());
        assert!(require_para_generation_capacity(root.path(), 1).is_err());
    }

    #[test]
    fn para_summary_category_order_is_deterministic() {
        let items = [
            serde_json::json!({"category":"relationship","fact":"R"}),
            serde_json::json!({"category":"commitment","fact":"C"}),
            serde_json::json!({"category":"decision","fact":"D"}),
        ];
        let rendered = render_para_summary("Ordered", items.iter()).unwrap();
        assert!(rendered.find("## Commitment").unwrap() < rendered.find("## Decision").unwrap());
        assert!(rendered.find("## Decision").unwrap() < rendered.find("## Relationship").unwrap());
    }

    #[test]
    fn log_append_creates_and_appends() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let entry = LogEntry {
            date: chrono::Local::now(),
            meeting_title: "Q2 Pricing Call".into(),
            meeting_path: "~/meetings/2026-04-03-pricing.md".into(),
            people_updated: vec!["Dan".into(), "Mat".into()],
            fact_count: 3,
            skipped_count: 1,
        };

        WikiAdapter.append_log(&entry, &config).unwrap();
        WikiAdapter.append_log(&entry, &config).unwrap();

        let log = fs::read_to_string(dir.path().join("log.md")).unwrap();
        assert!(log.contains("# Knowledge Log"));
        assert!(log.contains("Q2 Pricing Call"));
        assert!(log.contains("Facts written: 3, skipped: 1"));
        assert_eq!(log.matches("Q2 Pricing Call").count(), 2); // two appends
    }

    /// Stands in for a machine where `qmd` was never installed.
    struct AbsentQmdRunner;

    impl QmdRunner for AbsentQmdRunner {
        fn run_until(
            &self,
            _args: &[&str],
            _deadline: Instant,
        ) -> Result<QmdCommandResult, QmdRunError> {
            Err(QmdRunError::Io {
                kind: std::io::ErrorKind::NotFound,
                message: "no such file or directory".into(),
            })
        }
    }

    /// Installed, but cannot be inspected.
    struct BrokenQmdRunner;

    impl QmdRunner for BrokenQmdRunner {
        fn run_until(
            &self,
            _args: &[&str],
            _deadline: Instant,
        ) -> Result<QmdCommandResult, QmdRunError> {
            Err(QmdRunError::Other("qmd exploded".into()))
        }
    }
}
