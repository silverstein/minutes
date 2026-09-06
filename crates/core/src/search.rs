use crate::config::Config;
use crate::error::SearchError;
#[cfg(test)]
use crate::markdown::stable_active_corpus_revision;
use crate::markdown::{
    extract_field, is_inactive_corpus_dir_name, read_stable_active_markdown, split_frontmatter,
    stable_active_corpus_revision_with_budget,
    stable_active_corpus_revision_with_budget_and_snapshot_hook, stable_markdown_file_identity,
    ActiveCorpusReadBudget, ActiveCorpusRevisionError, Frontmatter, IntentKind, Sensitivity,
    StableActiveCorpusRevision, StableMarkdownFileIdentity, StableMarkdownSnapshot,
    ACTIVE_CORPUS_MAX_AUTHORIZATION_ATTEMPTS,
};
use crate::overlays;
use chrono::Local;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use walkdir::WalkDir;

/// Safe search-facade sync policy. The underlying index module is private so
/// downstream callers cannot return cached content without live policy
/// authorization.
pub use crate::search_index::SyncMode;

fn with_stable_active_corpus<T>(
    dir: &Path,
    operation: impl FnMut(&StableActiveCorpusRevision) -> Result<T, SearchError>,
) -> Result<T, SearchError> {
    with_stable_active_corpus_with_hooks(dir, operation, || {}, || {})
}

/// Name which limit stopped a corpus pass, without describing the corpus.
///
/// The previous message collapsed every cause into "could not be verified
/// safely", so a deadline overrun and a corpus genuinely over the byte ceiling
/// were indistinguishable. Diagnosing #679 needed a controlled experiment to
/// recover a fact the error already had.
///
/// This is a deliberate, bounded disclosure rather than no disclosure. The
/// text reaches CLI and desktop callers, so one who cannot read restricted
/// meetings can now tell an unopenable corpus from an oversized one, a walk
/// failure, or an exhausted deadline. It exposes no content, path, or count,
/// and each state is a whole-corpus condition the caller can already provoke
/// and observe by timing an ordinary search against the published ceilings.
/// The diagnosability is worth that; naming a file or a count would not be.
fn corpus_authorization_error(stage: &str, error: ActiveCorpusRevisionError) -> SearchError {
    let cause = match error {
        ActiveCorpusRevisionError::Unavailable => "the corpus could not be opened",
        // Every walk error lands here, including permission denials and files
        // that vanish mid-walk, so this must not claim an unsafe path.
        ActiveCorpusRevisionError::Traversal => "the corpus could not be walked",
        ActiveCorpusRevisionError::Budget => "the corpus exceeds a documented size ceiling",
        ActiveCorpusRevisionError::Deadline => "the authorization deadline elapsed",
    };
    SearchError::Io(std::io::Error::other(format!(
        "meeting corpus could not be {stage} safely: {cause}"
    )))
}

fn with_stable_active_corpus_with_hooks<T>(
    dir: &Path,
    mut operation: impl FnMut(&StableActiveCorpusRevision) -> Result<T, SearchError>,
    mut after_precheck: impl FnMut(),
    mut before_postcheck: impl FnMut(),
) -> Result<T, SearchError> {
    let envelope = ActiveCorpusReadBudget::new();
    for attempt in 0..ACTIVE_CORPUS_MAX_AUTHORIZATION_ATTEMPTS {
        let precheck_started = std::time::Instant::now();
        envelope.check_deadline().map_err(|_| {
            SearchError::Io(std::io::Error::other(
                "meeting corpus could not be verified safely",
            ))
        })?;
        // Every mandatory corpus pass keeps the documented per-pass envelope,
        // while all passes share one absolute operation deadline. This keeps
        // safe rereads from making an otherwise supported corpus unavailable.
        let before = stable_active_corpus_revision_with_budget(dir, envelope.fresh_pass())
            .map_err(|error| corpus_authorization_error("verified", error))?
            .with_read_budget(envelope.fresh_materialization_pass());
        let precheck_duration = precheck_started.elapsed();
        after_precheck();
        envelope.check_deadline().map_err(|_| {
            SearchError::Io(std::io::Error::other(
                "meeting corpus authorization deadline elapsed",
            ))
        })?;
        let operation_started = std::time::Instant::now();
        let value = operation(&before)?;
        let operation_duration = operation_started.elapsed();
        envelope.check_deadline().map_err(|_| {
            SearchError::Io(std::io::Error::other(
                "meeting corpus authorization deadline elapsed",
            ))
        })?;
        before_postcheck();
        let postcheck_started = std::time::Instant::now();
        let after = stable_active_corpus_revision_with_budget(dir, envelope.fresh_pass())
            .map_err(|error| corpus_authorization_error("reverified", error))?;
        let postcheck_duration = postcheck_started.elapsed();
        tracing::debug!(
            attempt = attempt + 1,
            precheck_duration_ms = precheck_duration.as_millis() as u64,
            operation_duration_ms = operation_duration.as_millis() as u64,
            postcheck_duration_ms = postcheck_duration.as_millis() as u64,
            "meeting corpus authorized operation"
        );
        if before == after {
            return Ok(value);
        }
    }
    Err(SearchError::Io(std::io::Error::other(
        "meeting corpus changed while materializing the result",
    )))
}

/// Re-authorize and refresh every index result from one live file snapshot.
///
/// FTS/QMD are ranking hints, not authorization or content sources. Stale
/// titles/snippets are replaced from live bytes; unreadable, malformed,
/// unknown-sensitivity, symlink-escaped, and restricted-without-override
/// results disappear. Even an explicit restricted override cannot authorize
/// a policy-uncertain file.
fn revision_snapshot(
    revision: &StableActiveCorpusRevision,
    path: &Path,
) -> Result<StableMarkdownSnapshot, SearchError> {
    revision.read_snapshot(path).ok_or_else(|| {
        SearchError::Io(std::io::Error::other(
            "an allowlisted meeting changed while materializing the result",
        ))
    })
}

fn policy_verified_result(
    mut result: SearchResult,
    snapshot: &StableMarkdownSnapshot,
    filters: &SearchFilters,
    query: &str,
) -> Option<(SearchResult, bool)> {
    let (frontmatter_str, body) = split_frontmatter(&snapshot.content);
    if frontmatter_str.is_empty() {
        return None;
    }
    let frontmatter = serde_yaml::from_str::<Frontmatter>(frontmatter_str).ok()?;
    let is_restricted = matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted));
    if !filters.include_restricted && is_restricted {
        return None;
    }

    let live_query = result.matched_via_alias.as_deref().unwrap_or(query);
    let live_query_trimmed = live_query.trim();
    let live_snippet = if live_query_trimmed.is_empty() {
        // Empty query is the documented list mode. It still needs every
        // policy/filter check below, but has no text predicate.
        Some(String::new())
    } else {
        crate::search_index::live_fts_match_snippet(&frontmatter.title, body, live_query_trimmed)
    }?;
    if filters.content_type.as_ref().is_some_and(|expected| {
        let actual = match frontmatter.r#type {
            crate::markdown::ContentType::Meeting => "meeting",
            crate::markdown::ContentType::Memo => "memo",
            crate::markdown::ContentType::Dictation => "dictation",
        };
        actual != expected
    }) {
        return None;
    }
    let live_date = frontmatter.date.to_rfc3339();
    if filters
        .since
        .as_ref()
        .is_some_and(|since| live_date < *since)
    {
        return None;
    }
    if filters.attendee.as_ref().is_some_and(|attendee| {
        let needle = attendee.to_lowercase();
        !frontmatter
            .normalized_attendees()
            .iter()
            .chain(frontmatter.people.iter())
            .any(|candidate| candidate.to_lowercase().contains(&needle))
    }) {
        return None;
    }
    if filters.recorded_by.as_ref().is_some_and(|recorded_by| {
        !frontmatter.recorded_by.as_ref().is_some_and(|candidate| {
            candidate
                .to_lowercase()
                .contains(&recorded_by.to_lowercase())
        })
    }) {
        return None;
    }
    if filters.intent_kind.as_ref().is_some_and(|intent_kind| {
        !frontmatter
            .intents
            .iter()
            .any(|intent| &intent.kind == intent_kind)
    }) {
        return None;
    }
    if filters.owner.as_ref().is_some_and(|owner| {
        let needle = owner.to_lowercase();
        !frontmatter
            .action_items
            .iter()
            .any(|item| item.assignee.to_lowercase().contains(&needle))
            && !frontmatter.intents.iter().any(|intent| {
                intent
                    .who
                    .as_ref()
                    .is_some_and(|who| who.to_lowercase().contains(&needle))
            })
    }) {
        return None;
    }

    result.path = snapshot.path.clone();
    result.title = frontmatter.title;
    result.date = live_date;
    result.content_type = match frontmatter.r#type {
        crate::markdown::ContentType::Meeting => "meeting".into(),
        crate::markdown::ContentType::Memo => "memo".into(),
        crate::markdown::ContentType::Dictation => "dictation".into(),
    };
    result.snippet = live_snippet;
    Some((result, is_restricted))
}

fn retain_policy_verified_results(
    results: &mut Vec<SearchResult>,
    filters: &SearchFilters,
    query: &str,
    revision: &StableActiveCorpusRevision,
) -> Result<(), SearchError> {
    let candidates = std::mem::take(results);
    for result in candidates {
        // Stale index rows absent from the pre-operation allowlist are
        // ineligible even if the path becomes readable during this query.
        if !revision.contains_path(&result.path) {
            continue;
        }
        let snapshot = revision_snapshot(revision, &result.path)?;
        if let Some((result, _)) = policy_verified_result(result, &snapshot, filters, query) {
            results.push(result);
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────
// Search always uses the process-private live projection. Legacy
// `[search].engine = "qmd"` configuration is deliberately ignored: a global
// persistent QMD index cannot guarantee prompt revocation after an external
// policy change.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
    pub snippet: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched_via_alias: Option<String>,
}

/// One descriptor-verified meeting snapshot authorized for a read surface.
///
/// Path resolution for human mutations is intentionally separate: being
/// inside the corpus is not, by itself, authorization to return meeting
/// bytes to an agent-facing caller.
#[derive(Debug, Clone)]
pub struct AuthorizedMeetingSnapshot {
    pub path: PathBuf,
    pub content: String,
    pub frontmatter: Frontmatter,
}

impl AuthorizedMeetingSnapshot {
    /// Re-read the source through the same descriptor-stable policy boundary
    /// and require that the canonical path and every source byte still match.
    ///
    /// Long-running agent/provider operations use this immediately before
    /// egress or a derived write so an in-place sensitivity flip cannot reuse
    /// an authorization granted to older bytes at the same pathname.
    pub fn reauthorize_exact(
        &self,
        config: &Config,
        include_restricted: bool,
    ) -> Result<(), SearchError> {
        let current = read_authorized_meeting(&self.path, config, include_restricted)?;
        if current.path != self.path || current.content != self.content {
            return Err(SearchError::Io(std::io::Error::other(
                "meeting changed after authorization",
            )));
        }
        Ok(())
    }
}

/// Derive one search excerpt only from a descriptor-authorized live snapshot.
/// Ranked candidate metadata is never reused after this second policy read.
pub fn authorized_snapshot_search_snippet(
    snapshot: &AuthorizedMeetingSnapshot,
    query: &str,
) -> Option<String> {
    let (_, body) = split_frontmatter(&snapshot.content);
    crate::search_index::live_fts_match_snippet(&snapshot.frontmatter.title, body, query)
}

/// Capability-bound handle for one human-requested meeting mutation.
///
/// The corpus root and source parent directory stay open from authorization
/// through unlink/rename, so replacing an ambient parent path with a
/// symlink/reparse directory cannot redirect the destructive operation.
pub struct MeetingMutation {
    path: PathBuf,
    canonical_root: PathBuf,
    root_dir: cap_std::fs::Dir,
    source_parent: cap_std::fs::Dir,
    source_name: std::ffi::OsString,
    source_identity: StableMarkdownFileIdentity,
    source_file: std::fs::File,
    source_sha256: String,
    sibling_authorizations: Mutex<BTreeMap<std::ffi::OsString, BoundSiblingAuthorization>>,
}

struct BoundMutationArtifact {
    name: std::ffi::OsString,
    identity: StableMarkdownFileIdentity,
    file: std::fs::File,
    expected_sha256: Option<String>,
}

struct BoundSiblingAuthorization {
    identity: StableMarkdownFileIdentity,
    file: std::fs::File,
}

pub struct StagedMeetingDeletion {
    staging_dir: cap_std::fs::Dir,
    staging_path: PathBuf,
    moved_artifacts: Vec<BoundMutationArtifact>,
}

impl StagedMeetingDeletion {
    /// Permanently sanitize the already-hidden group. Windows deletes the exact
    /// retained handles. POSIX cannot unlink by handle, so it truncates and
    /// synchronizes each exact retained object. Every platform deliberately
    /// keeps the inactive staging directory: removing it by name would permit
    /// an interposed unrelated empty directory to be deleted.
    pub fn finalize(self) -> std::io::Result<()> {
        self.finalize_with_hook(|_| {})
    }

    fn finalize_with_hook(
        mut self,
        mut before_exact_delete: impl FnMut(usize),
    ) -> std::io::Result<()> {
        // Preflight the complete group before physical cleanup so a staged
        // replacement cannot hide a group member before sanitization starts.
        for artifact in &self.moved_artifacts {
            if !MeetingMutation::artifact_is_current_at(&self.staging_dir, artifact) {
                return Err(std::io::Error::other(format!(
                    "staged meeting artifact changed before final deletion; recovery retained in {}",
                    self.staging_path.display()
                )));
            }
        }

        for (index, artifact) in self.moved_artifacts.drain(..).enumerate() {
            if !MeetingMutation::artifact_is_current_at(&self.staging_dir, &artifact) {
                return Err(std::io::Error::other(format!(
                    "staged meeting artifact changed during final deletion; recovery retained in {}",
                    self.staging_path.display()
                )));
            }
            // Exact adversarial boundary: no pathname lookup after this hook is
            // permitted to decide which object is destroyed.
            before_exact_delete(index);
            #[cfg(windows)]
            crate::policy_fs::delete_file_by_handle(&artifact.file).map_err(|error| {
                std::io::Error::other(format!(
                    "staged meeting artifact could not be deleted exactly; recovery retained at {}: {error}",
                    self.staging_path.join(&artifact.name).display()
                ))
            })?;
            #[cfg(unix)]
            {
                let recovery_path = self.staging_path.join(&artifact.name);
                artifact.file.set_len(0).map_err(|error| {
                    std::io::Error::other(format!(
                        "staged meeting artifact could not be sanitized; recovery retained at {}: {error}",
                        recovery_path.display()
                    ))
                })?;
                artifact.file.sync_all().map_err(|error| {
                    std::io::Error::other(format!(
                        "staged meeting artifact could not be synchronized; recovery retained at {}: {error}",
                        recovery_path.display()
                    ))
                })?;
                let sanitized_len = artifact.file.metadata().map_err(|error| {
                    std::io::Error::other(format!(
                        "staged meeting artifact could not be re-attested; recovery retained at {}: {error}",
                        recovery_path.display()
                    ))
                })?.len();
                if sanitized_len != 0 {
                    return Err(std::io::Error::other(
                        format!(
                            "staged meeting artifact could not be sanitized exactly; recovery retained at {}",
                            recovery_path.display()
                        ),
                    ));
                }
            }
            #[cfg(not(any(unix, windows)))]
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "safe staged deletion is unavailable on this platform",
            ));

            #[cfg(windows)]
            {
                let name = artifact.name.clone();
                drop(artifact);
                match self.staging_dir.symlink_metadata(&name) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error),
                    Ok(_) => {
                        return Err(std::io::Error::other(
                            "staged meeting artifact name was recreated during final deletion",
                        ));
                    }
                }
            }
        }
        #[cfg(any(unix, windows))]
        return Ok(());
        #[cfg(not(any(unix, windows)))]
        unreachable!()
    }
}

impl MeetingMutation {
    pub fn path(&self) -> &Path {
        &self.path
    }

    fn sibling_name(&self, path: &Path) -> std::io::Result<std::ffi::OsString> {
        let sibling_parent = path.parent().and_then(|parent| parent.canonicalize().ok());
        if sibling_parent.as_deref() != self.path.parent() {
            return Err(std::io::Error::other(
                "meeting artifact is outside the bound source directory",
            ));
        }
        path.file_name()
            .map(std::ffi::OsStr::to_os_string)
            .ok_or_else(|| std::io::Error::other("meeting artifact has no file name"))
    }

    fn open_bound_regular(
        directory: &cap_std::fs::Dir,
        name: &std::ffi::OsStr,
        expected_identity: Option<StableMarkdownFileIdentity>,
        expected_sha256: Option<&str>,
    ) -> std::io::Result<BoundMutationArtifact> {
        Self::open_bound_regular_with_access(
            directory,
            name,
            expected_identity,
            expected_sha256,
            false,
        )
    }

    fn open_bound_regular_with_access(
        directory: &cap_std::fs::Dir,
        name: &std::ffi::OsStr,
        expected_identity: Option<StableMarkdownFileIdentity>,
        expected_sha256: Option<&str>,
        writable: bool,
    ) -> std::io::Result<BoundMutationArtifact> {
        use cap_std::fs::OpenOptionsExt;

        let lexical = directory.symlink_metadata(name)?;
        if !crate::policy_fs::cap_lexical_regular_file_is_safe(&lexical) {
            return Err(std::io::Error::other(
                "meeting artifact is not a safe single-link regular file",
            ));
        }

        let mut options = cap_std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            if writable {
                options.write(true);
            }
            options.custom_flags(libc::O_NOFOLLOW);
        }
        #[cfg(windows)]
        {
            let _ = writable;
            use windows_sys::Win32::Foundation::GENERIC_READ;
            use windows_sys::Win32::Storage::FileSystem::{
                DELETE, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ,
                FILE_SHARE_WRITE,
            };
            options
                .access_mode(GENERIC_READ | DELETE)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
        }
        let mut file = directory.open_with(name, &options)?.into_std();
        if !crate::policy_fs::opened_regular_file_is_safe(&file) {
            return Err(std::io::Error::other(
                "meeting artifact is not a safe single-link regular file",
            ));
        }
        let identity = stable_markdown_file_identity(&file).ok_or_else(|| {
            std::io::Error::other("meeting artifact identity could not be retained")
        })?;
        if expected_identity.is_some_and(|expected| expected != identity) {
            return Err(std::io::Error::other(
                "meeting artifact changed after authorization",
            ));
        }
        if let Some(expected_sha256) = expected_sha256 {
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(crate::policy_fs::MAX_BOUND_TEXT_FILE_BYTES + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() as u64 > crate::policy_fs::MAX_BOUND_TEXT_FILE_BYTES {
                return Err(std::io::Error::other(
                    "meeting exceeded the safe mutation byte ceiling",
                ));
            }
            file.seek(SeekFrom::Start(0))?;
            if crate::policy_fs::content_sha256_hex(&bytes) != expected_sha256 {
                return Err(std::io::Error::other("meeting changed after authorization"));
            }
        }
        Ok(BoundMutationArtifact {
            name: name.to_os_string(),
            identity,
            file,
            expected_sha256: expected_sha256.map(str::to_owned),
        })
    }

    fn bind_source(&self, writable: bool) -> std::io::Result<BoundMutationArtifact> {
        let current = Self::open_bound_regular_with_access(
            &self.source_parent,
            &self.source_name,
            Some(self.source_identity),
            Some(&self.source_sha256),
            writable,
        )?;
        Ok(BoundMutationArtifact {
            name: current.name,
            identity: current.identity,
            file: if writable {
                current.file
            } else {
                self.source_file.try_clone()?
            },
            expected_sha256: current.expected_sha256,
        })
    }

    fn source_identity_is_current(&self) -> bool {
        self.bind_source(false).is_ok()
    }

    fn retain_sibling_authorization(
        &self,
        artifact: &BoundMutationArtifact,
    ) -> std::io::Result<()> {
        let mut authorizations = self
            .sibling_authorizations
            .lock()
            .map_err(|_| std::io::Error::other("meeting artifact authorization was poisoned"))?;
        match authorizations.get(&artifact.name) {
            Some(expected) if expected.identity != artifact.identity => Err(std::io::Error::other(
                "meeting artifact changed after authorization",
            )),
            Some(_) => Ok(()),
            None => {
                authorizations.insert(
                    artifact.name.clone(),
                    BoundSiblingAuthorization {
                        identity: artifact.identity,
                        file: artifact.file.try_clone()?,
                    },
                );
                Ok(())
            }
        }
    }

    fn expected_sibling_identity(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<Option<StableMarkdownFileIdentity>> {
        self.sibling_authorizations
            .lock()
            .map(|authorizations| authorizations.get(name).map(|bound| bound.identity))
            .map_err(|_| std::io::Error::other("meeting artifact authorization was poisoned"))
    }

    fn retained_sibling(
        &self,
        name: &std::ffi::OsStr,
    ) -> std::io::Result<Option<(StableMarkdownFileIdentity, std::fs::File)>> {
        self.sibling_authorizations
            .lock()
            .map_err(|_| std::io::Error::other("meeting artifact authorization was poisoned"))?
            .get(name)
            .map(|bound| bound.file.try_clone().map(|file| (bound.identity, file)))
            .transpose()
    }

    fn archive_dir_with_hook(
        &self,
        before_open: impl FnOnce(),
    ) -> std::io::Result<cap_std::fs::Dir> {
        match self.root_dir.create_dir("archive") {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        crate::policy_fs::open_directory_at_no_follow_with_hook(
            &self.root_dir,
            std::ffi::OsStr::new("archive"),
            before_open,
        )
    }

    /// Confirm that a candidate sibling is a regular file beneath the parent
    /// capability retained when this mutation was authorized.
    ///
    /// Callers use this to select optional artifacts without an ambient
    /// `Path::exists` check. The first successful check retains the artifact's
    /// stable identity; later checks and the eventual atomic transfer must
    /// identify that same file.
    pub fn sibling_exists(&self, path: &Path) -> bool {
        let Ok(name) = self.sibling_name(path) else {
            return false;
        };
        let Ok(bound) = Self::open_bound_regular(
            &self.source_parent,
            &name,
            self.expected_sibling_identity(&name).ok().flatten(),
            None,
        ) else {
            return false;
        };
        self.retain_sibling_authorization(&bound).is_ok()
    }

    fn bind_group(
        &self,
        siblings: &[PathBuf],
        writable: bool,
    ) -> std::io::Result<Vec<BoundMutationArtifact>> {
        let mut artifacts = vec![self.bind_source(writable)?];
        for sibling in siblings {
            let name = self.sibling_name(sibling)?;
            if artifacts.iter().any(|artifact| artifact.name == name) {
                continue;
            }
            let expected = self.expected_sibling_identity(&name)?;
            let current = Self::open_bound_regular_with_access(
                &self.source_parent,
                &name,
                expected,
                None,
                writable,
            )?;
            self.retain_sibling_authorization(&current)?;
            let (identity, file) = self
                .retained_sibling(&name)?
                .ok_or_else(|| std::io::Error::other("meeting artifact was not retained"))?;
            if identity != current.identity {
                return Err(std::io::Error::other(
                    "meeting artifact changed after authorization",
                ));
            }
            artifacts.push(BoundMutationArtifact {
                name,
                identity,
                file: if writable { current.file } else { file },
                expected_sha256: None,
            });
        }
        Ok(artifacts)
    }

    fn artifact_is_current_at(
        directory: &cap_std::fs::Dir,
        artifact: &BoundMutationArtifact,
    ) -> bool {
        Self::open_bound_regular(
            directory,
            &artifact.name,
            Some(artifact.identity),
            artifact.expected_sha256.as_deref(),
        )
        .is_ok()
    }

    #[cfg(unix)]
    fn unpredictable_claim_name() -> std::io::Result<std::ffi::OsString> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|error| std::io::Error::other(error.to_string()))?;
        let suffix = random
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        Ok(std::ffi::OsString::from(format!(
            ".minutes-mutation-claim-{suffix}"
        )))
    }

    /// Perform one descriptor-relative atomic no-replace transfer and attest
    /// that the destination still names the retained artifact. Linux/macOS
    /// rename by name, so an identity mismatch is rolled back with the same
    /// non-clobbering primitive. Windows renames the exact opened handle.
    fn move_file_no_replace(
        source: &cap_std::fs::Dir,
        source_path: &Path,
        artifact: &BoundMutationArtifact,
        destination: &cap_std::fs::Dir,
        destination_path: &Path,
    ) -> std::io::Result<()> {
        Self::move_file_no_replace_with_hooks(
            source,
            source_path,
            artifact,
            destination,
            destination_path,
            || {},
            || {},
            || {},
        )
    }

    #[allow(clippy::too_many_arguments)] // Each closure marks one exact adversarial boundary.
    fn move_file_no_replace_with_hooks(
        source: &cap_std::fs::Dir,
        source_path: &Path,
        artifact: &BoundMutationArtifact,
        destination: &cap_std::fs::Dir,
        destination_path: &Path,
        before_atomic_claim: impl FnOnce(),
        after_atomic_claim: impl FnOnce(),
        after_atomic_promotion: impl FnOnce(),
    ) -> std::io::Result<()> {
        #[cfg(not(unix))]
        let _ = (source_path, destination_path);

        #[cfg(unix)]
        {
            // POSIX cannot rename an opened file handle. First atomically claim
            // the current source name into an unpredictable inactive name, then
            // attest the claimed object. A replacement that wins the final
            // source-name race is quarantined and never promoted, deleted, or
            // restored over a newer source.
            let claim_name = Self::unpredictable_claim_name()?;
            before_atomic_claim();
            crate::policy_fs::move_entry_at_no_replace(
                source,
                &artifact.name,
                &artifact.file,
                destination,
                &claim_name,
            )?;
            after_atomic_claim();
            let claimed = BoundMutationArtifact {
                name: claim_name,
                identity: artifact.identity,
                file: artifact.file.try_clone()?,
                expected_sha256: artifact.expected_sha256.clone(),
            };
            if !Self::artifact_is_current_at(destination, &claimed) {
                return match crate::policy_fs::move_entry_at_no_replace(
                    destination,
                    &claimed.name,
                    &claimed.file,
                    source,
                    &artifact.name,
                ) {
                    Ok(()) => Err(std::io::Error::other(format!(
                        "meeting artifact replacement was restored to {}",
                        source_path.join(&artifact.name).display()
                    ))),
                    Err(_) => Err(std::io::Error::other(format!(
                        "meeting artifact replacement was preserved in inactive quarantine at {}",
                        destination_path.join(&claimed.name).display()
                    ))),
                };
            }

            if let Err(promotion_error) = crate::policy_fs::move_entry_at_no_replace(
                destination,
                &claimed.name,
                &claimed.file,
                destination,
                &artifact.name,
            ) {
                return match crate::policy_fs::move_entry_at_no_replace(
                    destination,
                    &claimed.name,
                    &claimed.file,
                    source,
                    &artifact.name,
                ) {
                    Ok(()) => Err(promotion_error),
                    Err(_) => Err(std::io::Error::other(format!(
                        "meeting artifact claim could not be promoted and was preserved at {}",
                        destination_path.join(&claimed.name).display()
                    ))),
                };
            }
            after_atomic_promotion();
            if Self::artifact_is_current_at(destination, artifact) {
                return Ok(());
            }
            match crate::policy_fs::move_entry_at_no_replace(
                destination,
                &artifact.name,
                &artifact.file,
                source,
                &artifact.name,
            ) {
                Ok(()) => Err(std::io::Error::other(format!(
                    "promoted meeting artifact replacement was restored to {}",
                    source_path.join(&artifact.name).display()
                ))),
                Err(_) => {
                    let quarantine_name = Self::unpredictable_claim_name()?;
                    match crate::policy_fs::move_entry_at_no_replace(
                        destination,
                        &artifact.name,
                        &artifact.file,
                        destination,
                        &quarantine_name,
                    ) {
                        Ok(()) => Err(std::io::Error::other(format!(
                            "promoted meeting artifact replacement was preserved in inactive quarantine at {}",
                            destination_path.join(quarantine_name).display()
                        ))),
                        Err(_) => Err(std::io::Error::other(format!(
                            "promoted meeting artifact replacement remains recoverable at {}",
                            destination_path.join(&artifact.name).display()
                        ))),
                    }
                }
            }
        }

        #[cfg(windows)]
        {
            before_atomic_claim();
            after_atomic_claim();
        }
        #[cfg(not(any(unix, windows)))]
        {
            before_atomic_claim();
            after_atomic_claim();
        }

        #[cfg(not(unix))]
        crate::policy_fs::move_entry_at_no_replace(
            source,
            &artifact.name,
            &artifact.file,
            destination,
            &artifact.name,
        )?;
        #[cfg(not(unix))]
        after_atomic_promotion();
        #[cfg(not(unix))]
        if Self::artifact_is_current_at(destination, artifact) {
            return Ok(());
        }

        #[cfg(not(unix))]
        let identity_error =
            std::io::Error::other("meeting artifact changed at the atomic transfer boundary");
        #[cfg(not(unix))]
        match crate::policy_fs::move_entry_at_no_replace(
            destination,
            &artifact.name,
            &artifact.file,
            source,
            &artifact.name,
        ) {
            Ok(()) => Err(identity_error),
            Err(rollback_error) => Err(rollback_error),
        }
    }

    fn move_group_with_hooks(
        &self,
        siblings: &[PathBuf],
        destination: &cap_std::fs::Dir,
        destination_path: &Path,
        writable: bool,
        before_move: impl FnMut(usize),
        after_move: impl FnMut(usize),
    ) -> std::io::Result<Vec<BoundMutationArtifact>> {
        self.move_group_with_claim_hooks(
            siblings,
            destination,
            destination_path,
            writable,
            before_move,
            |_| {},
            |_| {},
            after_move,
        )
    }

    #[allow(clippy::too_many_arguments)] // Test seams mirror the transfer's ordered boundaries.
    fn move_group_with_claim_hooks(
        &self,
        siblings: &[PathBuf],
        destination: &cap_std::fs::Dir,
        destination_path: &Path,
        writable: bool,
        mut before_move: impl FnMut(usize),
        mut after_claim: impl FnMut(usize),
        mut after_promotion: impl FnMut(usize),
        mut after_move: impl FnMut(usize),
    ) -> std::io::Result<Vec<BoundMutationArtifact>> {
        let artifacts = self.bind_group(siblings, writable)?;
        let source_parent_path = self
            .path
            .parent()
            .ok_or_else(|| std::io::Error::other("meeting source has no parent"))?;
        for artifact in &artifacts {
            if destination.symlink_metadata(&artifact.name).is_ok() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "mutation destination already exists",
                ));
            }
        }

        for (index, artifact) in artifacts.iter().enumerate() {
            let result = if !Self::artifact_is_current_at(&self.source_parent, artifact) {
                Err(std::io::Error::other(
                    "meeting artifact changed during the requested mutation",
                ))
            } else {
                Self::move_file_no_replace_with_hooks(
                    &self.source_parent,
                    source_parent_path,
                    artifact,
                    destination,
                    destination_path,
                    || before_move(index),
                    || after_claim(index),
                    || after_promotion(index),
                )
            };
            if let Err(error) = result {
                let mut rollback_error = None;
                for moved_artifact in artifacts[..index].iter().rev() {
                    if let Err(rollback) = Self::move_file_no_replace(
                        destination,
                        destination_path,
                        moved_artifact,
                        &self.source_parent,
                        source_parent_path,
                    ) {
                        rollback_error.get_or_insert(rollback);
                    }
                }
                return Err(rollback_error.unwrap_or(error));
            }
            after_move(index);
        }
        Ok(artifacts)
    }

    #[cfg(test)]
    fn delete_source_with_hook(&self, hook: impl FnOnce()) -> std::io::Result<()> {
        if !self.source_identity_is_current() {
            return Err(std::io::Error::other(
                "meeting changed before the requested deletion",
            ));
        }
        hook();
        if !self.source_identity_is_current() {
            return Err(std::io::Error::other(
                "meeting changed during the requested deletion",
            ));
        }
        self.source_parent.remove_file(&self.source_name)
    }

    pub fn archive_group(&self, siblings: &[PathBuf]) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
        self.archive_group_with_hook(siblings, |_| {})
    }

    fn archive_group_with_hook(
        &self,
        siblings: &[PathBuf],
        after_move: impl FnMut(usize),
    ) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
        self.archive_group_with_hooks(siblings, |_| {}, after_move)
    }

    fn archive_group_with_hooks(
        &self,
        siblings: &[PathBuf],
        before_move: impl FnMut(usize),
        after_move: impl FnMut(usize),
    ) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
        self.archive_group_with_all_hooks(siblings, before_move, after_move, || {})
    }

    fn archive_group_with_all_hooks(
        &self,
        siblings: &[PathBuf],
        before_move: impl FnMut(usize),
        after_move: impl FnMut(usize),
        before_open_archive: impl FnOnce(),
    ) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
        if !self.source_identity_is_current() {
            return Err(std::io::Error::other(
                "meeting changed before the requested archive",
            ));
        }
        let archive = self.archive_dir_with_hook(before_open_archive)?;
        let archive_path = self.canonical_root.join("archive");
        let moved = self.move_group_with_hooks(
            siblings,
            &archive,
            &archive_path,
            false,
            before_move,
            after_move,
        )?;
        let destinations = moved
            .iter()
            .map(|artifact| self.canonical_root.join("archive").join(&artifact.name))
            .collect::<Vec<_>>();
        Ok((destinations[0].clone(), destinations[1..].to_vec()))
    }

    pub fn stage_delete_group(
        self,
        siblings: &[PathBuf],
    ) -> std::io::Result<StagedMeetingDeletion> {
        let staged = self.stage_delete_group_with_hooks(siblings, |_| {}, |_| {})?;
        // On Windows the mutation retains companion handles that deliberately
        // bind the source and optional siblings through the ordered move.
        // Retire those authorization handles before the caller asks the
        // staged transaction to apply exact POSIX-style disposition.
        drop(self);
        Ok(staged)
    }

    fn stage_delete_group_with_hooks(
        &self,
        siblings: &[PathBuf],
        before_move: impl FnMut(usize),
        after_move: impl FnMut(usize),
    ) -> std::io::Result<StagedMeetingDeletion> {
        self.stage_delete_group_with_all_hooks(siblings, before_move, after_move, || {})
    }

    fn stage_delete_group_with_all_hooks(
        &self,
        siblings: &[PathBuf],
        before_move: impl FnMut(usize),
        after_move: impl FnMut(usize),
        before_open_staging: impl FnOnce(),
    ) -> std::io::Result<StagedMeetingDeletion> {
        let mut before_open_staging = Some(before_open_staging);
        let mut created = None;
        for _ in 0..8 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            let suffix = random
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let name = std::ffi::OsString::from(format!(".delete-staging-{suffix}"));
            match self.root_dir.create_dir(&name) {
                Ok(()) => {
                    let dir = match before_open_staging.take() {
                        Some(hook) => crate::policy_fs::open_directory_at_no_follow_with_hook(
                            &self.root_dir,
                            &name,
                            hook,
                        )?,
                        None => {
                            crate::policy_fs::open_directory_at_no_follow(&self.root_dir, &name)?
                        }
                    };
                    created = Some((name, dir));
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        let (staging_name, staging_dir) = created
            .ok_or_else(|| std::io::Error::other("could not create safe deletion staging"))?;

        let staging_path = self.canonical_root.join(&staging_name);
        let moved = self.move_group_with_hooks(
            siblings,
            &staging_dir,
            &staging_path,
            true,
            before_move,
            after_move,
        )?;
        Ok(StagedMeetingDeletion {
            staging_dir,
            staging_path,
            moved_artifacts: moved,
        })
    }

    #[cfg(test)]
    fn archive_group_with_collision_after_first_move(
        &self,
        siblings: &[PathBuf],
    ) -> std::io::Result<(PathBuf, Vec<PathBuf>)> {
        let collision = siblings
            .first()
            .and_then(|path| path.file_name())
            .map(|name| self.canonical_root.join("archive").join(name));
        self.archive_group_with_hook(siblings, |index| {
            if index == 0 {
                if let Some(path) = &collision {
                    std::fs::write(path, b"collision canary").unwrap();
                }
            }
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IntentResult {
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

#[derive(Debug, Clone, Serialize)]
pub struct ReportEntry {
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
    /// Frontmatter v2: optional authority grade ("high" | "medium" | "low").
    /// Propagated from the source decision when present. None for pre-v2
    /// frontmatter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionConflict {
    pub topic: String,
    pub latest: ReportEntry,
    pub previous: Vec<ReportEntry>,
    /// Frontmatter v2: when the latest decision explicitly `supersedes` an
    /// earlier one, this carries the supersession rationale. Consumers like
    /// `/minutes-lint` should treat resolved conflicts as informational
    /// rather than red flags. None means this is an unresolved contradiction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct OwnerResolution {
    who: Option<String>,
    who_original: Option<String>,
    who_provenance: Option<String>,
}

#[derive(Debug, Clone)]
struct SpeakerOwner {
    name: String,
    provenance: String,
}

fn speaker_overlay_map(
    frontmatter: &Frontmatter,
    _overlay_db_path: &Path,
    _meeting_path: &Path,
) -> HashMap<String, SpeakerOwner> {
    // Agent-facing aggregates are derived only from the policy-authorized
    // source snapshot. The mutable overlay database has an independent
    // lifecycle and cannot be included without binding its revision into the
    // same aggregate authorization transaction.
    frontmatter
        .speaker_map
        .iter()
        .filter(|attr| attr.confidence == crate::diarize::Confidence::High)
        .map(|attr| {
            (
                attr.speaker_label.clone(),
                SpeakerOwner {
                    name: attr.name.clone(),
                    provenance: "speaker_map".to_string(),
                },
            )
        })
        .collect::<HashMap<_, _>>()
}

fn resolve_owner_with_speaker_overlays(
    who: Option<&str>,
    speaker_overlays: &HashMap<String, SpeakerOwner>,
) -> OwnerResolution {
    let Some(raw) = who.map(str::trim).filter(|value| !value.is_empty()) else {
        return OwnerResolution::default();
    };

    if let Some(speaker) = speaker_overlays.get(raw) {
        return OwnerResolution {
            who: Some(speaker.name.clone()),
            who_original: Some(raw.to_string()),
            who_provenance: Some(speaker.provenance.clone()),
        };
    }

    OwnerResolution {
        who: Some(raw.to_string()),
        who_original: None,
        who_provenance: None,
    }
}

fn owner_matches(resolution: &OwnerResolution, owner_lower: &str) -> bool {
    resolution
        .who
        .as_ref()
        .is_some_and(|who| who.to_lowercase().contains(owner_lower))
        || resolution
            .who_original
            .as_ref()
            .is_some_and(|who| who.to_lowercase().contains(owner_lower))
}

fn explicit_supersedes_resolution(
    latest_supersedes: Option<&str>,
    conflicting_previous: &[ReportEntry],
) -> Option<String> {
    let value = latest_supersedes
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    // `supersedes` is a free-text pointer. It's reliable for a simple
    // one-new-decision-replaces-one-prior-decision case, but not strong enough
    // to auto-resolve an entire topic arc when multiple contradictory prior
    // decisions remain. Stay conservative so `/minutes-lint` doesn't hide a
    // still-live conflict as "resolved".
    if conflicting_previous.len() != 1 {
        return None;
    }

    if !supersedes_references_previous_decision(value, &conflicting_previous[0]) {
        return None;
    }

    Some(format!("Resolved by explicit supersedes: {}", value))
}

fn supersedes_references_previous_decision(supersedes: &str, previous: &ReportEntry) -> bool {
    let supersedes_norm = normalize_decision_value(supersedes);
    if supersedes_norm.is_empty() {
        return false;
    }

    let previous_date = previous
        .date
        .split('T')
        .next()
        .unwrap_or(previous.date.as_str());
    let previous_date_norm = normalize_decision_value(previous_date);
    if !previous_date_norm.is_empty() && supersedes_norm.contains(&previous_date_norm) {
        return true;
    }

    let previous_title_norm = normalize_decision_value(&previous.title);
    if previous_title_norm.len() >= 4 && supersedes_norm.contains(&previous_title_norm) {
        return true;
    }

    let previous_what_norm = normalize_decision_value(&previous.what);
    if previous_what_norm.is_empty() {
        return false;
    }

    let supersedes_tokens = supersedes_norm
        .split_whitespace()
        .filter(|token| token.len() >= 4)
        .collect::<std::collections::HashSet<_>>();
    let previous_tokens = previous_what_norm
        .split_whitespace()
        .filter(|token| token.len() >= 4)
        .collect::<std::collections::HashSet<_>>();

    supersedes_tokens
        .intersection(&previous_tokens)
        .take(2)
        .count()
        >= 2
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleCommitment {
    pub kind: IntentKind,
    pub entry: ReportEntry,
    pub meetings_since: usize,
    pub age_days: i64,
    pub reasons: Vec<String>,
    pub latest_follow_up: Option<MeetingReference>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsistencyReport {
    pub decision_conflicts: Vec<DecisionConflict>,
    pub stale_commitments: Vec<StaleCommitment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicSummary {
    pub topic: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingReference {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PersonProfile {
    pub name: String,
    pub recent_meetings: Vec<MeetingReference>,
    pub open_intents: Vec<IntentResult>,
    pub recent_decisions: Vec<ReportEntry>,
    pub top_topics: Vec<TopicSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossMeetingResearch {
    pub query: String,
    pub related_decisions: Vec<ReportEntry>,
    pub related_open_intents: Vec<IntentResult>,
    pub recent_meetings: Vec<MeetingReference>,
    pub related_topics: Vec<TopicSummary>,
}

#[derive(Default)]
pub struct SearchFilters {
    pub content_type: Option<String>,
    pub since: Option<String>,
    pub attendee: Option<String>,
    pub intent_kind: Option<IntentKind>,
    pub owner: Option<String>,
    pub recorded_by: Option<String>,
    /// Include meetings designated `sensitivity: restricted`. Default false:
    /// restricted meetings are excluded from search and intent results unless
    /// the caller opts in explicitly (consent layer Wave 2). Callers that set
    /// this on an agent-facing surface must record the override on the event
    /// bus (`sensitivity.override`).
    pub include_restricted: bool,
}

/// Resolve a meeting file by slug prefix (date-title pattern).
/// Returns the first match found in the output directory.
pub fn resolve_slug(slug: &str, config: &Config) -> Option<PathBuf> {
    resolve_slug_with_budget(slug, config, ActiveCorpusReadBudget::new())
        .ok()
        .flatten()
}

fn resolve_slug_with_budget(
    slug: &str,
    config: &Config,
    budget: ActiveCorpusReadBudget,
) -> Result<Option<PathBuf>, ActiveCorpusRevisionError> {
    if slug.is_empty() {
        return Ok(None);
    }

    let dir = &config.output_dir;
    if !dir.exists() {
        return Ok(None);
    }
    let canonical_root = dir
        .canonicalize()
        .map_err(|_| ActiveCorpusRevisionError::Unavailable)?;
    budget.consume_path(&canonical_root)?;

    // Paths are scope-resolved without opening any unrelated meeting. The
    // later authorized read remains the exact-byte and sensitivity boundary.
    let candidate = Path::new(slug);
    if candidate.is_absolute()
        || candidate.components().count() > 1
        || candidate
            .extension()
            .is_some_and(|extension| extension == "md")
    {
        let canonical = candidate
            .canonicalize()
            .map_err(|_| ActiveCorpusRevisionError::Unavailable)?;
        budget.consume_path(&canonical)?;
        return Ok((canonical.starts_with(&canonical_root)
            && canonical
                .extension()
                .is_some_and(|extension| extension == "md"))
        .then_some(canonical));
    }

    let slug = slug.to_lowercase();
    let entries = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            entry.depth() == 0
                || !entry.file_type().is_dir()
                || !is_inactive_corpus_dir_name(entry.file_name())
        });
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => {
                budget.consume(1, 0, 0)?;
                return Err(ActiveCorpusRevisionError::Traversal);
            }
        };
        if entry.file_type().is_dir() {
            budget.consume(0, 1, 0)?;
        } else {
            budget.consume(1, 0, 0)?;
        }
        budget.consume_path(entry.path())?;
        if !entry.file_type().is_file()
            || entry
                .path()
                .extension()
                .is_none_or(|extension| extension != "md")
        {
            continue;
        }
        let filename = entry
            .path()
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        if filename.to_lowercase().contains(&slug) {
            let canonical = entry
                .path()
                .canonicalize()
                .map_err(|_| ActiveCorpusRevisionError::Unavailable)?;
            budget.consume_path(&canonical)?;
            if canonical.starts_with(&canonical_root) {
                return Ok(Some(canonical));
            }
            return Ok(None);
        }
    }

    Ok(None)
}

/// Bind one exact active meeting to retained root/parent capabilities for a
/// human mutation. Sensitivity policy does not block deletion or archival.
pub fn open_meeting_mutation(path: &Path, config: &Config) -> Option<MeetingMutation> {
    let canonical_root = config.output_dir.canonicalize().ok()?;
    let snapshot = read_stable_active_markdown(path, &canonical_root)?;
    meeting_mutation_from_snapshot(canonical_root, snapshot)
}

fn meeting_mutation_from_snapshot(
    canonical_root: PathBuf,
    snapshot: StableMarkdownSnapshot,
) -> Option<MeetingMutation> {
    meeting_mutation_from_snapshot_with_parent_open_hook(canonical_root, snapshot, |_| {})
}

fn meeting_mutation_from_snapshot_with_parent_open_hook(
    canonical_root: PathBuf,
    snapshot: StableMarkdownSnapshot,
    mut before_open_parent: impl FnMut(&std::ffi::OsStr),
) -> Option<MeetingMutation> {
    let relative = snapshot.path.strip_prefix(&canonical_root).ok()?;
    let source_name = relative.file_name()?.to_os_string();
    let relative_parent = relative.parent().unwrap_or_else(|| Path::new(""));

    let root_dir = crate::policy_fs::open_directory_no_follow(&canonical_root).ok()?;
    let mut source_parent = root_dir.try_clone().ok()?;
    for component in relative_parent.components() {
        let std::path::Component::Normal(name) = component else {
            return None;
        };
        source_parent =
            crate::policy_fs::open_directory_at_no_follow_with_hook(&source_parent, name, || {
                before_open_parent(name)
            })
            .ok()?;
    }

    let source_sha256 = crate::policy_fs::content_sha256_hex(snapshot.content.as_bytes());
    let source_file = MeetingMutation::open_bound_regular(
        &source_parent,
        &source_name,
        Some(snapshot.file_identity),
        Some(&source_sha256),
    )
    .ok()?
    .file;
    let mutation = MeetingMutation {
        path: snapshot.path,
        canonical_root,
        root_dir,
        source_parent,
        source_name,
        source_identity: snapshot.file_identity,
        source_file,
        source_sha256,
        sibling_authorizations: Mutex::new(BTreeMap::new()),
    };
    mutation.source_identity_is_current().then_some(mutation)
}

fn authorize_meeting_snapshot(
    snapshot: &StableMarkdownSnapshot,
    include_restricted: bool,
) -> Result<Frontmatter, SearchError> {
    let (frontmatter_yaml, _) = split_frontmatter(&snapshot.content);
    if frontmatter_yaml.is_empty() {
        return Err(SearchError::Io(std::io::Error::other(
            "meeting policy metadata is missing",
        )));
    }
    let frontmatter = serde_yaml::from_str::<Frontmatter>(frontmatter_yaml).map_err(|_| {
        SearchError::Io(std::io::Error::other("meeting policy metadata is invalid"))
    })?;
    if !include_restricted && matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) {
        return Err(SearchError::Io(std::io::Error::other(
            "meeting is restricted; an explicit audited override is required",
        )));
    }
    Ok(frontmatter)
}

/// Read one active meeting through the same authorization policy whether the
/// caller supplied a slug-resolved path or an exact path.
pub fn read_authorized_meeting(
    path: &Path,
    config: &Config,
    include_restricted: bool,
) -> Result<AuthorizedMeetingSnapshot, SearchError> {
    let canonical_root = config.output_dir.canonicalize().map_err(|_| {
        SearchError::Io(std::io::Error::other(
            "meeting corpus could not be verified safely",
        ))
    })?;
    let snapshot = read_stable_active_markdown(path, &canonical_root).ok_or_else(|| {
        SearchError::Io(std::io::Error::other(
            "meeting could not be read as a stable policy snapshot",
        ))
    })?;
    let frontmatter = authorize_meeting_snapshot(&snapshot, include_restricted)?;

    Ok(AuthorizedMeetingSnapshot {
        path: snapshot.path,
        content: snapshot.content,
        frontmatter,
    })
}

/// Bind one exact policy-authorized meeting to the same retained capabilities
/// used by destructive corpus mutations. Classification and the mutation's
/// identity/hash originate from one descriptor-stable snapshot, so an in-place
/// sensitivity flip cannot race between authorization and archive/delete.
pub fn open_authorized_meeting_mutation(
    path: &Path,
    config: &Config,
    include_restricted: bool,
) -> Result<MeetingMutation, SearchError> {
    let canonical_root = config.output_dir.canonicalize().map_err(|_| {
        SearchError::Io(std::io::Error::other(
            "meeting corpus could not be verified safely",
        ))
    })?;
    let snapshot = read_stable_active_markdown(path, &canonical_root).ok_or_else(|| {
        SearchError::Io(std::io::Error::other(
            "meeting could not be read as a stable policy snapshot",
        ))
    })?;
    authorize_meeting_snapshot(&snapshot, include_restricted)?;
    meeting_mutation_from_snapshot(canonical_root, snapshot).ok_or_else(|| {
        SearchError::Io(std::io::Error::other(
            "meeting changed before the requested mutation",
        ))
    })
}

pub fn cross_meeting_research(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<CrossMeetingResearch, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    with_stable_active_corpus(dir, |revision| {
        cross_meeting_research_once(query, config, filters, revision)
    })
}

fn cross_meeting_research_once(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    revision: &StableActiveCorpusRevision,
) -> Result<CrossMeetingResearch, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let query_lower = query.to_lowercase();
    let mut related_decisions = Vec::new();
    let mut related_open_intents = Vec::new();
    let mut recent_meetings = Vec::new();
    let mut topic_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let overlay_db_path = overlays::default_db_path();
    for path in revision.paths() {
        let snapshot = revision_snapshot(revision, path)?;
        let path = snapshot.path.as_path();

        let (frontmatter_str, _) = split_frontmatter(&snapshot.content);
        if frontmatter_str.is_empty() {
            continue;
        }

        let frontmatter: Frontmatter = match serde_yaml::from_str(frontmatter_str) {
            Ok(frontmatter) => frontmatter,
            Err(_) => {
                tracing::warn!("skipping policy-uncertain meeting in cross-meeting research");
                continue;
            }
        };

        // Sensitivity enforcement (consent layer Wave 2): restricted meetings
        // stay out of cross-meeting research unless explicitly overridden.
        if !filters.include_restricted
            && matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted))
        {
            continue;
        }

        let content_type = match frontmatter.r#type {
            crate::markdown::ContentType::Meeting => "meeting".to_string(),
            crate::markdown::ContentType::Memo => "memo".to_string(),
            crate::markdown::ContentType::Dictation => "dictation".to_string(),
        };
        let speaker_overlays = speaker_overlay_map(&frontmatter, &overlay_db_path, path);
        if let Some(ref type_filter) = filters.content_type {
            if content_type != *type_filter {
                continue;
            }
        }

        let date = frontmatter.date.to_rfc3339();
        if let Some(ref since) = filters.since {
            if date < *since {
                continue;
            }
        }
        if let Some(ref attendee) = filters.attendee {
            let attendee_lower = attendee.to_lowercase();
            let attendee_match = frontmatter
                .attendees
                .iter()
                .any(|name| name.to_lowercase().contains(&attendee_lower))
                || frontmatter
                    .people
                    .iter()
                    .any(|person| person.to_lowercase().contains(&attendee_lower));
            if !attendee_match {
                continue;
            }
        }

        let meeting_matches = frontmatter.title.to_lowercase().contains(&query_lower)
            || frontmatter
                .context
                .as_ref()
                .map(|context| context.to_lowercase().contains(&query_lower))
                .unwrap_or(false);

        let mut matched_this_meeting = meeting_matches;

        for decision in &frontmatter.decisions {
            let topic = decision
                .topic
                .clone()
                .unwrap_or_else(|| normalize_topic(&decision.text));
            let haystack = format!("{} {}", topic, decision.text).to_lowercase();
            if haystack.contains(&query_lower) {
                matched_this_meeting = true;
                if !topic.is_empty() {
                    *topic_counts.entry(topic).or_insert(0) += 1;
                }
                related_decisions.push(ReportEntry {
                    path: path.to_path_buf(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    what: decision.text.clone(),
                    who: None,
                    who_original: None,
                    who_provenance: None,
                    by_date: None,
                    authority: decision.authority.clone(),
                });
            }
        }

        for intent in &frontmatter.intents {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            let haystack = format!(
                "{} {} {} {} {}",
                intent.what,
                owner_resolution.who.clone().unwrap_or_default(),
                owner_resolution.who_original.clone().unwrap_or_default(),
                intent.status,
                intent.by_date.clone().unwrap_or_default()
            )
            .to_lowercase();
            if !haystack.contains(&query_lower) {
                continue;
            }

            matched_this_meeting = true;
            let topic = normalize_topic(&intent.what);
            if !topic.is_empty() {
                *topic_counts.entry(topic).or_insert(0) += 1;
            }

            if intent.status == "open" {
                related_open_intents.push(IntentResult {
                    path: path.to_path_buf(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    content_type: content_type.clone(),
                    kind: intent.kind,
                    what: intent.what.clone(),
                    who: owner_resolution.who.clone(),
                    who_original: owner_resolution.who_original.clone(),
                    who_provenance: owner_resolution.who_provenance.clone(),
                    status: intent.status.clone(),
                    by_date: intent.by_date.clone(),
                });
            }
        }

        if matched_this_meeting {
            recent_meetings.push(MeetingReference {
                path: path.to_path_buf(),
                title: frontmatter.title.clone(),
                date,
                content_type,
            });
        }
    }

    related_decisions.sort_by(|a, b| b.date.cmp(&a.date));
    related_open_intents.sort_by(|a, b| b.date.cmp(&a.date));
    recent_meetings.sort_by(|a, b| b.date.cmp(&a.date));

    let mut related_topics: Vec<TopicSummary> = topic_counts
        .into_iter()
        .map(|(topic, count)| TopicSummary { topic, count })
        .collect();
    related_topics.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.topic.cmp(&b.topic)));

    related_decisions.truncate(10);
    related_open_intents.truncate(10);
    recent_meetings.truncate(10);
    related_topics.truncate(5);

    Ok(CrossMeetingResearch {
        query: query.to_string(),
        related_decisions,
        related_open_intents,
        recent_meetings,
        related_topics,
    })
}

/// Search all markdown files in the meetings directory.
pub fn search(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, SearchError> {
    search_with_mode(query, config, filters, SyncMode::Auto)
}

/// Search with explicit sync mode. Lets the CLI expose `--sync` / `--no-sync`
/// flags for piped/scripted use cases without making every other caller think
/// about freshness.
///
/// Logs sync stats (indexed/updated/removed/duration_ms) at INFO level when
/// any work was done. Empty/no-op syncs stay silent. Public search deliberately
/// rebuilds a private projection from stable live-source snapshots on every
/// call so revoked plaintext cannot persist in a durable cache.
pub fn search_with_mode(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    mode: SyncMode,
) -> Result<Vec<SearchResult>, SearchError> {
    search_with_mode_and_vocabulary(query, config, filters, mode, None)
}

fn search_with_mode_and_vocabulary(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    mode: SyncMode,
    vocabulary_override: Option<&crate::vocabulary::VocabularyStore>,
) -> Result<Vec<SearchResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    if query.trim().is_empty() {
        return list_from_stable_corpus(dir, filters, mode);
    }
    with_stable_active_corpus(dir, |revision| {
        search_with_mode_and_vocabulary_once(
            query,
            config,
            filters,
            mode,
            vocabulary_override,
            revision,
        )
    })
}

/// Materialize empty-query results during the mandatory pre-operation corpus
/// attestation. List mode has no text predicate, so building a full FTS5
/// projection and then re-reading every projected row cannot add authority or
/// improve the result. The complete post-operation attestation remains the
/// publication gate: additions, removals, replacements, content changes, and
/// sensitivity flips still fail closed exactly as they do for text search.
fn list_from_stable_corpus(
    dir: &Path,
    filters: &SearchFilters,
    mode: SyncMode,
) -> Result<Vec<SearchResult>, SearchError> {
    crate::policy_fs::retire_legacy_policy_caches().map_err(|error| {
        SearchError::Io(std::io::Error::other(format!(
            "retire legacy durable policy caches before listing: {error}"
        )))
    })?;
    let envelope = ActiveCorpusReadBudget::new();
    for attempt in 0..ACTIVE_CORPUS_MAX_AUTHORIZATION_ATTEMPTS {
        envelope.check_deadline().map_err(|_| {
            SearchError::Io(std::io::Error::other(
                "meeting corpus could not be verified safely",
            ))
        })?;

        let precheck_started = std::time::Instant::now();
        let mut results = Vec::new();
        let mut restricted_results = Vec::new();
        let before = stable_active_corpus_revision_with_budget_and_snapshot_hook(
            dir,
            envelope.fresh_pass(),
            |snapshot| {
                let candidate = SearchResult {
                    path: snapshot.path.clone(),
                    title: String::new(),
                    date: String::new(),
                    content_type: String::new(),
                    snippet: String::new(),
                    matched_via_alias: None,
                };
                if let Some((result, is_restricted)) =
                    policy_verified_result(candidate, snapshot, filters, "")
                {
                    if is_restricted {
                        restricted_results.push(result);
                    } else {
                        results.push(result);
                    }
                }
            },
        )
        .map_err(|error| corpus_authorization_error("verified", error))?
        .with_read_budget(envelope.fresh_materialization_pass());
        results.sort_by(|left, right| right.date.cmp(&left.date));
        restricted_results.sort_by(|left, right| right.date.cmp(&left.date));
        results.append(&mut restricted_results);
        let precheck_duration = precheck_started.elapsed();

        envelope.check_deadline().map_err(|_| {
            SearchError::Io(std::io::Error::other(
                "meeting corpus authorization deadline elapsed",
            ))
        })?;
        let postcheck_started = std::time::Instant::now();
        let after = stable_active_corpus_revision_with_budget(dir, envelope.fresh_pass())
            .map_err(|error| corpus_authorization_error("reverified", error))?;
        let postcheck_duration = postcheck_started.elapsed();

        tracing::debug!(
            attempt = attempt + 1,
            ?mode,
            precheck_duration_ms = precheck_duration.as_millis() as u64,
            postcheck_duration_ms = postcheck_duration.as_millis() as u64,
            result_count = results.len(),
            "meeting list phases"
        );
        if before == after {
            return Ok(results);
        }
    }
    Err(SearchError::Io(std::io::Error::other(
        "meeting corpus changed while materializing the result",
    )))
}

fn search_with_mode_and_vocabulary_once(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    mode: SyncMode,
    vocabulary_override: Option<&crate::vocabulary::VocabularyStore>,
    revision: &StableActiveCorpusRevision,
) -> Result<Vec<SearchResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    let open_started = std::time::Instant::now();
    let index = crate::search_index::SearchIndex::open(config)?;
    let open_duration = open_started.elapsed();
    let sync_started = std::time::Instant::now();
    let stats = index.sync_for_active_corpus(config, mode, revision)?;
    let sync_duration = sync_started.elapsed();
    if stats.indexed + stats.updated + stats.removed + stats.errored > 0 {
        tracing::info!(
            indexed = stats.indexed,
            updated = stats.updated,
            removed = stats.removed,
            errored = stats.errored,
            duration_ms = stats.duration_ms,
            "search index sync"
        );
    }

    let expansions = vocabulary_search_expansions(query, vocabulary_override);
    if expansions.len() <= 1 {
        let search_started = std::time::Instant::now();
        let mut results = index.search(query, filters, None)?;
        if filters.include_restricted {
            results
                .extend(index.search_restricted_live_for_active_corpus(query, filters, revision)?);
        }
        let search_duration = search_started.elapsed();
        // Normal and explicit-override restricted candidates pass through one
        // identical live-snapshot, filter, and exact-FTS authorization gate.
        let policy_started = std::time::Instant::now();
        retain_policy_verified_results(&mut results, filters, query, revision)?;
        let policy_duration = policy_started.elapsed();
        tracing::debug!(
            list_mode = query.trim().is_empty(),
            open_duration_ms = open_duration.as_millis() as u64,
            sync_duration_ms = sync_duration.as_millis() as u64,
            search_duration_ms = search_duration.as_millis() as u64,
            policy_duration_ms = policy_duration.as_millis() as u64,
            result_count = results.len(),
            "meeting search phases"
        );
        return Ok(results);
    }

    let original_key = search_expansion_key(query);
    let mut merged = Vec::new();
    let mut seen_paths = std::collections::HashSet::new();
    let mut restricted = Vec::new();
    if filters.include_restricted {
        // Restricted files are intentionally absent from the ephemeral FTS
        // index, so collect their policy-filtered candidates once. Determine
        // the first matching vocabulary expansion from the already-bound
        // revision rather than rewalking the entire corpus for every alias.
        for mut result in
            index.search_restricted_live_for_active_corpus(query, filters, revision)?
        {
            let snapshot = revision_snapshot(revision, &result.path)?;
            let (frontmatter_text, body) = split_frontmatter(&snapshot.content);
            let Ok(frontmatter) = serde_yaml::from_str::<Frontmatter>(frontmatter_text) else {
                continue;
            };
            let Some(expansion) = expansions.iter().find(|expansion| {
                crate::search_index::live_fts_match_snippet(&frontmatter.title, body, expansion)
                    .is_some()
            }) else {
                continue;
            };
            if search_expansion_key(expansion) != original_key {
                result.matched_via_alias = Some(expansion.clone());
            }
            restricted.push(result);
        }
    }

    for (expansion_index, expansion) in expansions.into_iter().enumerate() {
        let expansion_key = search_expansion_key(&expansion);
        for mut result in index.search(&expansion, filters, None)? {
            if !seen_paths.insert(result.path.clone()) {
                continue;
            }
            if expansion_key != original_key {
                result.matched_via_alias = Some(expansion.clone());
            }
            merged.push(result);
        }
        if expansion_index == 0 {
            for result in std::mem::take(&mut restricted) {
                if !seen_paths.insert(result.path.clone()) {
                    continue;
                }
                merged.push(result);
            }
        }
    }

    retain_policy_verified_results(&mut merged, filters, query, revision)?;
    Ok(merged)
}

fn vocabulary_search_expansions(
    query: &str,
    vocabulary_override: Option<&crate::vocabulary::VocabularyStore>,
) -> Vec<String> {
    if query.trim().is_empty() {
        return Vec::new();
    }

    let mut expansions = vocabulary_override
        .map(|store| store.search_expansions(query))
        .unwrap_or_else(|| {
            crate::vocabulary::load()
                .map(|store| store.search_expansions(query))
                .unwrap_or_else(|error| {
                    tracing::debug!(error = %error, "could not load vocabulary for search expansion");
                    Vec::new()
                })
        });

    if expansions.is_empty() {
        expansions.push(query.trim().to_string());
    } else if !expansions
        .iter()
        .any(|candidate| search_expansion_key(candidate) == search_expansion_key(query))
    {
        expansions.insert(0, query.trim().to_string());
    }

    let mut seen = std::collections::HashSet::new();
    expansions
        .into_iter()
        .filter_map(|candidate| {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                return None;
            }
            let key = search_expansion_key(trimmed);
            if seen.insert(key) {
                Some(trimmed.to_string())
            } else {
                None
            }
        })
        .take(8)
        .collect()
}

fn search_expansion_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase()
}

/// Search structured intents across all markdown files in the meetings directory.
pub fn search_intents(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<Vec<IntentResult>, SearchError> {
    search_intents_at(query, config, filters, &overlays::default_db_path())
}

fn search_intents_at(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    overlay_db_path: &Path,
) -> Result<Vec<IntentResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    with_stable_active_corpus(dir, |revision| {
        search_intents_at_once(query, config, filters, overlay_db_path, revision)
    })
}

fn search_intents_at_once(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    overlay_db_path: &Path,
    revision: &StableActiveCorpusRevision,
) -> Result<Vec<IntentResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let query_lower = query.to_lowercase();
    let mut results = Vec::new();
    for path in revision.paths() {
        let snapshot = revision_snapshot(revision, path)?;
        match process_intent_snapshot(snapshot, &query_lower, filters, overlay_db_path) {
            Ok(mut file_results) => results.append(&mut file_results),
            Err(_) => {
                tracing::warn!("skipping policy-uncertain meeting in intent search");
            }
        }
    }

    results.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(results)
}

pub fn consistency_report(
    config: &Config,
    owner: Option<&str>,
    stale_after_days: i64,
) -> Result<ConsistencyReport, SearchError> {
    consistency_report_at(
        config,
        owner,
        stale_after_days,
        &overlays::default_db_path(),
    )
}

fn consistency_report_at(
    config: &Config,
    owner: Option<&str>,
    stale_after_days: i64,
    overlay_db_path: &Path,
) -> Result<ConsistencyReport, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    with_stable_active_corpus(dir, |revision| {
        consistency_report_at_once(config, owner, stale_after_days, overlay_db_path, revision)
    })
}

fn consistency_report_at_once(
    config: &Config,
    owner: Option<&str>,
    stale_after_days: i64,
    overlay_db_path: &Path,
    revision: &StableActiveCorpusRevision,
) -> Result<ConsistencyReport, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let mut parsed_frontmatters = Vec::new();
    for path in revision.paths() {
        let snapshot = revision_snapshot(revision, path)?;
        let path = snapshot.path.as_path();

        let (frontmatter_str, _) = split_frontmatter(&snapshot.content);
        if frontmatter_str.is_empty() {
            continue;
        }

        match serde_yaml::from_str::<Frontmatter>(frontmatter_str) {
            // Sensitivity enforcement (consent layer Wave 2): restricted
            // meetings never feed the consistency report. No override on this
            // surface in this wave — like the graph, exclusion is complete.
            Ok(frontmatter) if matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) => {
                tracing::debug!(
                    "skipping restricted meeting in consistency report (sensitivity enforcement)"
                );
            }
            Ok(frontmatter) => parsed_frontmatters.push((path.to_path_buf(), frontmatter)),
            Err(_) => {
                tracing::warn!("skipping policy-uncertain meeting in consistency report");
            }
        }
    }

    parsed_frontmatters.sort_by_key(|entry| entry.1.date);

    let owner_lower = owner.map(|value| value.to_lowercase());
    let now = Local::now();
    // Each entry carries its source decision's `supersedes` value alongside the
    // ReportEntry so we can detect documented supersessions when the topic
    // group has conflicting decisions.
    let mut decision_groups: std::collections::HashMap<String, Vec<(ReportEntry, Option<String>)>> =
        std::collections::HashMap::new();
    let mut stale_commitments = Vec::new();

    for (path, frontmatter) in &parsed_frontmatters {
        let speaker_overlays = speaker_overlay_map(frontmatter, overlay_db_path, path);

        for decision in &frontmatter.decisions {
            let topic = decision
                .topic
                .as_deref()
                .map(normalize_topic)
                .filter(|topic| !topic.is_empty())
                .unwrap_or_else(|| normalize_topic(&decision.text));
            if topic.is_empty() {
                continue;
            }

            decision_groups.entry(topic).or_default().push((
                ReportEntry {
                    path: path.clone(),
                    title: frontmatter.title.clone(),
                    date: frontmatter.date.to_rfc3339(),
                    what: decision.text.clone(),
                    who: None,
                    who_original: None,
                    who_provenance: None,
                    by_date: None,
                    authority: decision.authority.clone(),
                },
                decision.supersedes.clone(),
            ));
        }

        for intent in &frontmatter.intents {
            if !matches!(intent.kind, IntentKind::Commitment | IntentKind::ActionItem) {
                continue;
            }
            if intent.status != "open" {
                continue;
            }

            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            if let Some(ref owner_lower) = owner_lower {
                if !owner_matches(&owner_resolution, owner_lower) {
                    continue;
                }
            }

            let newer_meetings: Vec<_> = parsed_frontmatters
                .iter()
                .filter(|(_, newer)| newer.date > frontmatter.date)
                .collect();
            let meetings_since = newer_meetings.len();
            let age_days = now.signed_duration_since(frontmatter.date).num_days();
            let latest_follow_up =
                newer_meetings
                    .last()
                    .map(|(path, frontmatter)| MeetingReference {
                        path: path.clone(),
                        title: frontmatter.title.clone(),
                        date: frontmatter.date.to_rfc3339(),
                        content_type: match frontmatter.r#type {
                            crate::markdown::ContentType::Meeting => "meeting".to_string(),
                            crate::markdown::ContentType::Memo => "memo".to_string(),
                            crate::markdown::ContentType::Dictation => "dictation".to_string(),
                        },
                    });

            let mut reasons = Vec::new();
            if age_days >= stale_after_days {
                reasons.push(format!("{} days old", age_days));
            }
            if meetings_since >= 3 {
                reasons.push(format!("{} newer meetings since", meetings_since));
            }
            if let Some(by_date) = &intent.by_date {
                if meetings_since >= 1 || age_days >= 1 {
                    reasons.push(format!("still open with due date {}", by_date));
                }
            }
            if intent
                .who
                .as_deref()
                .is_none_or(|who| who.trim().is_empty())
            {
                reasons.push("still open without an owner".to_string());
            }

            if !reasons.is_empty() {
                stale_commitments.push(StaleCommitment {
                    kind: intent.kind,
                    entry: ReportEntry {
                        path: path.clone(),
                        title: frontmatter.title.clone(),
                        date: frontmatter.date.to_rfc3339(),
                        what: intent.what.clone(),
                        who: owner_resolution.who.clone(),
                        who_original: owner_resolution.who_original.clone(),
                        who_provenance: owner_resolution.who_provenance.clone(),
                        by_date: intent.by_date.clone(),
                        authority: None,
                    },
                    meetings_since,
                    age_days,
                    reasons,
                    latest_follow_up,
                });
            }
        }
    }

    let mut decision_conflicts = Vec::new();
    for (topic, mut entries) in decision_groups {
        entries.sort_by(|a, b| a.0.date.cmp(&b.0.date));
        let mut unique_values = std::collections::HashSet::new();
        for (entry, _) in &entries {
            unique_values.insert(normalize_decision_value(&entry.what));
        }

        if unique_values.len() > 1 {
            let (latest_entry, latest_supersedes) = entries.pop().expect("entries not empty");
            let previous_entries: Vec<ReportEntry> =
                entries.into_iter().map(|(entry, _)| entry).collect();
            let resolution =
                explicit_supersedes_resolution(latest_supersedes.as_deref(), &previous_entries);
            decision_conflicts.push(DecisionConflict {
                topic,
                latest: latest_entry,
                previous: previous_entries,
                resolution,
            });
        }
    }

    decision_conflicts.sort_by(|a, b| b.latest.date.cmp(&a.latest.date));
    stale_commitments.sort_by(|a, b| b.entry.date.cmp(&a.entry.date));

    Ok(ConsistencyReport {
        decision_conflicts,
        stale_commitments,
    })
}

pub fn person_profile(config: &Config, person: &str) -> Result<PersonProfile, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }
    with_stable_active_corpus(dir, |revision| {
        person_profile_once(config, person, revision)
    })
}

fn person_profile_once(
    config: &Config,
    person: &str,
    revision: &StableActiveCorpusRevision,
) -> Result<PersonProfile, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let person_lower = person.to_lowercase();
    let mut parsed_frontmatters = Vec::new();
    let overlay_db_path = overlays::default_db_path();
    for path in revision.paths() {
        let snapshot = revision_snapshot(revision, path)?;
        let path = snapshot.path.as_path();

        let (frontmatter_str, _) = split_frontmatter(&snapshot.content);
        if frontmatter_str.is_empty() {
            continue;
        }

        match serde_yaml::from_str::<Frontmatter>(frontmatter_str) {
            // Sensitivity enforcement (consent layer Wave 2): restricted
            // meetings never feed person profiles. No override on this
            // surface in this wave — like the graph, exclusion is complete.
            Ok(frontmatter) if matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) => {
                tracing::debug!(
                    "skipping restricted meeting in person profile (sensitivity enforcement)"
                );
            }
            Ok(frontmatter) => parsed_frontmatters.push((path.to_path_buf(), frontmatter)),
            Err(_) => {
                tracing::warn!("skipping policy-uncertain meeting in person profile");
            }
        }
    }

    parsed_frontmatters.sort_by_key(|entry| std::cmp::Reverse(entry.1.date));

    let mut recent_meetings = Vec::new();
    let mut open_intents = Vec::new();
    let mut recent_decisions = Vec::new();
    let mut topic_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (path, frontmatter) in parsed_frontmatters {
        let content_type = match frontmatter.r#type {
            crate::markdown::ContentType::Meeting => "meeting".to_string(),
            crate::markdown::ContentType::Memo => "memo".to_string(),
            crate::markdown::ContentType::Dictation => "dictation".to_string(),
        };
        let date = frontmatter.date.to_rfc3339();
        let speaker_overlays = speaker_overlay_map(&frontmatter, &overlay_db_path, &path);

        let attendee_match = frontmatter
            .attendees
            .iter()
            .any(|attendee| attendee.to_lowercase().contains(&person_lower));
        let linked_person_match = frontmatter
            .people
            .iter()
            .any(|person| person.to_lowercase().contains(&person_lower))
            || frontmatter.entities.people.iter().any(|entity| {
                entity.label.to_lowercase().contains(&person_lower)
                    || entity
                        .aliases
                        .iter()
                        .any(|alias| alias.to_lowercase().contains(&person_lower))
            });
        let owned_intent_match = frontmatter.intents.iter().any(|intent| {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            owner_matches(&owner_resolution, &person_lower)
        });

        if !(attendee_match || linked_person_match || owned_intent_match) {
            continue;
        }

        recent_meetings.push(MeetingReference {
            path: path.clone(),
            title: frontmatter.title.clone(),
            date: date.clone(),
            content_type: content_type.clone(),
        });

        for decision in &frontmatter.decisions {
            recent_decisions.push(ReportEntry {
                path: path.clone(),
                title: frontmatter.title.clone(),
                date: date.clone(),
                what: decision.text.clone(),
                who: None,
                who_original: None,
                who_provenance: None,
                by_date: None,
                authority: decision.authority.clone(),
            });

            let topic = decision
                .topic
                .clone()
                .unwrap_or_else(|| normalize_topic(&decision.text));
            if !topic.is_empty() {
                *topic_counts.entry(topic).or_insert(0) += 1;
            }
        }

        for intent in &frontmatter.intents {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            let owned_by_person = owner_matches(&owner_resolution, &person_lower);

            if owned_by_person
                && intent.status == "open"
                && matches!(intent.kind, IntentKind::ActionItem | IntentKind::Commitment)
            {
                open_intents.push(IntentResult {
                    path: path.clone(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    content_type: content_type.clone(),
                    kind: intent.kind,
                    what: intent.what.clone(),
                    who: owner_resolution.who.clone(),
                    who_original: owner_resolution.who_original.clone(),
                    who_provenance: owner_resolution.who_provenance.clone(),
                    status: intent.status.clone(),
                    by_date: intent.by_date.clone(),
                });
            }

            if attendee_match || owned_by_person {
                let topic = normalize_topic(&intent.what);
                if !topic.is_empty() {
                    *topic_counts.entry(topic).or_insert(0) += 1;
                }
            }
        }
    }

    recent_meetings.truncate(5);
    recent_decisions.truncate(5);
    open_intents.truncate(10);

    let mut top_topics: Vec<TopicSummary> = topic_counts
        .into_iter()
        .map(|(topic, count)| TopicSummary { topic, count })
        .collect();
    top_topics.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.topic.cmp(&b.topic)));
    top_topics.truncate(5);

    Ok(PersonProfile {
        name: person.to_string(),
        recent_meetings,
        open_intents,
        recent_decisions,
        top_topics,
    })
}

// Legacy walk-and-grep helper. The `search()` public API now delegates to the
// FTS5 index, but `cross_meeting_research`, `person_profile`, and
// `find_open_actions` (deferred to follow-up PRs) still walk files. They'll be
// migrated in their own PRs; meanwhile this stays so the helpers don't need
// to be reinvented later.
#[allow(dead_code)]
fn process_file(
    path: &Path,
    query: &str,
    filters: &SearchFilters,
) -> Result<Option<SearchResult>, SearchError> {
    let content = std::fs::read_to_string(path)?;

    // Parse frontmatter
    let (frontmatter_str, body) = split_frontmatter(&content);
    let title = extract_field(frontmatter_str, "title").unwrap_or_default();
    let date = extract_field(frontmatter_str, "date").unwrap_or_default();
    let content_type = extract_field(frontmatter_str, "type").unwrap_or_else(|| "meeting".into());

    // Apply filters
    if let Some(ref type_filter) = filters.content_type {
        if content_type != *type_filter {
            return Ok(None);
        }
    }
    if let Some(ref since) = filters.since {
        if date < *since {
            return Ok(None);
        }
    }
    if let Some(ref attendee) = filters.attendee {
        let attendees = extract_field(frontmatter_str, "attendees").unwrap_or_default();
        if !attendees.to_lowercase().contains(&attendee.to_lowercase()) {
            return Ok(None);
        }
    }
    if let Some(ref recorded_by) = filters.recorded_by {
        let recorded = extract_field(frontmatter_str, "recorded_by").unwrap_or_default();
        if !recorded
            .to_lowercase()
            .contains(&recorded_by.to_lowercase())
        {
            return Ok(None);
        }
    }

    // Text search (case-insensitive)
    let body_lower = body.to_lowercase();
    let title_lower = title.to_lowercase();

    if body_lower.contains(query) || title_lower.contains(query) {
        let snippet = extract_snippet(body, query);
        Ok(Some(SearchResult {
            path: path.to_path_buf(),
            title,
            date,
            content_type,
            snippet,
            matched_via_alias: None,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
fn process_intent_file(
    path: &Path,
    canonical_root: &Path,
    query: &str,
    filters: &SearchFilters,
    overlay_db_path: &Path,
) -> Result<Vec<IntentResult>, SearchError> {
    let snapshot = read_stable_active_markdown(path, canonical_root).ok_or_else(|| {
        SearchError::Io(std::io::Error::other(
            "meeting could not be read as a stable policy snapshot",
        ))
    })?;
    process_intent_snapshot(snapshot, query, filters, overlay_db_path)
}

fn process_intent_snapshot(
    snapshot: StableMarkdownSnapshot,
    query: &str,
    filters: &SearchFilters,
    overlay_db_path: &Path,
) -> Result<Vec<IntentResult>, SearchError> {
    let path = snapshot.path.as_path();
    let (frontmatter_str, _) = split_frontmatter(&snapshot.content);
    if frontmatter_str.is_empty() {
        return Ok(vec![]);
    }

    let frontmatter: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .map_err(|e| SearchError::Io(std::io::Error::other(e.to_string())))?;

    // Sensitivity enforcement (consent layer Wave 2): restricted meetings
    // contribute no intent records unless explicitly overridden.
    if !filters.include_restricted
        && matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted))
    {
        return Ok(vec![]);
    }

    let date = frontmatter.date.to_rfc3339();
    let content_type = match frontmatter.r#type {
        crate::markdown::ContentType::Meeting => "meeting".to_string(),
        crate::markdown::ContentType::Memo => "memo".to_string(),
        crate::markdown::ContentType::Dictation => "dictation".to_string(),
    };

    if let Some(ref type_filter) = filters.content_type {
        if content_type != *type_filter {
            return Ok(vec![]);
        }
    }
    if let Some(ref since) = filters.since {
        if date < *since {
            return Ok(vec![]);
        }
    }
    if let Some(ref attendee) = filters.attendee {
        let attendee_lower = attendee.to_lowercase();
        let attendee_match = frontmatter
            .attendees
            .iter()
            .any(|name| name.to_lowercase().contains(&attendee_lower));
        if !attendee_match {
            return Ok(vec![]);
        }
    }
    if let Some(ref recorded_by) = filters.recorded_by {
        let matches = frontmatter
            .recorded_by
            .as_ref()
            .is_some_and(|r| r.to_lowercase().contains(&recorded_by.to_lowercase()));
        if !matches {
            return Ok(vec![]);
        }
    }

    let speaker_overlays = speaker_overlay_map(&frontmatter, overlay_db_path, path);
    let mut results = Vec::new();
    for intent in frontmatter.intents {
        if let Some(kind) = filters.intent_kind {
            if intent.kind != kind {
                continue;
            }
        }
        let owner_resolution =
            resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
        if let Some(ref owner) = filters.owner {
            let owner_lower = owner.to_lowercase();
            if !owner_matches(&owner_resolution, &owner_lower) {
                continue;
            }
        }

        let haystack = format!(
            "{} {} {} {} {} {}",
            frontmatter.title,
            intent.what,
            owner_resolution.who.clone().unwrap_or_default(),
            owner_resolution.who_original.clone().unwrap_or_default(),
            intent.status,
            intent.by_date.clone().unwrap_or_default()
        )
        .to_lowercase();

        if !query.is_empty() && !haystack.contains(query) {
            continue;
        }

        results.push(IntentResult {
            path: path.to_path_buf(),
            title: frontmatter.title.clone(),
            date: date.clone(),
            content_type: content_type.clone(),
            kind: intent.kind,
            what: intent.what,
            who: owner_resolution.who,
            who_original: owner_resolution.who_original,
            who_provenance: owner_resolution.who_provenance,
            status: intent.status,
            by_date: intent.by_date,
        });
    }

    Ok(results)
}

// split_frontmatter and extract_field are in markdown.rs (shared)

/// Find meetings with open action items, optionally filtered by assignee.
/// Parses YAML frontmatter for the structured action_items field.
///
/// Meetings designated `sensitivity: restricted` are excluded unless
/// `include_restricted` is true; callers that set it on an agent-facing
/// surface must record the override on the event bus (consent layer Wave 2).
pub fn find_open_actions(
    config: &Config,
    assignee: Option<&str>,
    include_restricted: bool,
) -> Result<Vec<ActionResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Ok(vec![]);
    }
    with_stable_active_corpus(dir, |revision| {
        find_open_actions_once(config, assignee, include_restricted, revision)
    })
}

fn find_open_actions_once(
    config: &Config,
    assignee: Option<&str>,
    include_restricted: bool,
    revision: &StableActiveCorpusRevision,
) -> Result<Vec<ActionResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();

    for path in revision.paths() {
        let snapshot = revision_snapshot(revision, path)?;
        let path = snapshot.path;

        let (fm_str, _) = split_frontmatter(&snapshot.content);
        if fm_str.is_empty() {
            continue;
        }
        let frontmatter = match serde_yaml::from_str::<Frontmatter>(fm_str) {
            Ok(frontmatter) => frontmatter,
            Err(_) => continue,
        };
        if !include_restricted && matches!(frontmatter.sensitivity, Some(Sensitivity::Restricted)) {
            continue;
        }

        for item in frontmatter.action_items {
            if item.status != "open" {
                continue;
            }
            if let Some(filter) = assignee {
                let candidate = item.assignee.to_lowercase();
                let filter = filter.to_lowercase();
                if candidate != filter && !candidate.contains(&filter) {
                    continue;
                }
            }

            results.push(ActionResult {
                meeting_path: path.clone(),
                meeting_title: frontmatter.title.clone(),
                meeting_date: frontmatter.date.to_rfc3339(),
                assignee: item.assignee,
                task: item.task,
                due: item.due,
            });
        }
    }

    results.sort_by(|a, b| b.meeting_date.cmp(&a.meeting_date));
    Ok(results)
}

/// A structured action item result from cross-meeting search.
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    pub meeting_path: PathBuf,
    pub meeting_title: String,
    pub meeting_date: String,
    pub assignee: String,
    pub task: String,
    pub due: Option<String>,
}

/// Extract a snippet around the first match of the query.
#[allow(dead_code)]
fn extract_snippet(body: &str, query: &str) -> String {
    // Find the query in the body case-insensitively.
    // We search the original body to avoid byte-offset mismatch from to_lowercase().
    let pos = body
        .char_indices()
        .position(|(i, _)| body[i..].to_lowercase().starts_with(query))
        .and_then(|char_idx| body.char_indices().nth(char_idx).map(|(i, _)| i));

    if let Some(pos) = pos {
        let start = body[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = body[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(body.len());

        let line = body[start..end].trim();
        if line.chars().count() > 200 {
            let truncated: String = line.chars().take(200).collect();
            format!("{}...", truncated)
        } else {
            line.to_string()
        }
    } else {
        String::new()
    }
}

fn normalize_topic(text: &str) -> String {
    let stopwords = [
        "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
        "with", "we", "should", "will", "be", "is", "are", "use", "using",
    ];

    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .filter(|word| !stopwords.contains(&word.to_lowercase().as_str()))
        .take(4)
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_decision_value(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    fn delete_staging_entries(root: &Path) -> Vec<std::ffi::OsString> {
        std::fs::read_dir(root)
            .unwrap()
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .filter(|name| name.to_string_lossy().starts_with(".delete-staging-"))
            .collect()
    }

    fn assert_single_retained_empty_staging(root: &Path) {
        let entries = delete_staging_entries(root);
        assert_eq!(entries.len(), 1, "one inactive staging quarantine");
        assert!(
            std::fs::read_dir(root.join(&entries[0]))
                .unwrap()
                .next()
                .is_none(),
            "rolled-back staging quarantine should be empty"
        );
    }

    fn mutation_claim_entries(directory: &Path) -> Vec<PathBuf> {
        std::fs::read_dir(directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".minutes-mutation-claim-")
            })
            .map(|entry| entry.path())
            .collect()
    }

    #[test]
    fn search_finds_matching_content() {
        // Search hits the HOME-derived sqlite index; serialize with the
        // crate HOME-env lock or a concurrently HOME-swapping test yanks
        // the index mid-write (sqlite disk I/O error).
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Test Meeting\ndate: 2026-03-17\ntype: meeting\n---\n\n## Transcript\n\nWe discussed pricing strategy in detail.",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };

        let results = search("pricing", &config, &filters).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("pricing"));
    }

    #[test]
    fn search_returns_empty_for_no_match() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ndate: 2026-03-17\n---\n\nHello world.",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };

        let results = search("nonexistent", &config, &filters).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_is_case_insensitive() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ntype: meeting\ndate: 2026-03-17\n---\n\nPRICING discussion",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };

        let results = search("pricing", &config, &filters).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn configured_persistent_qmd_engine_is_ignored_for_private_search() {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Private Projection\ntype: meeting\ndate: 2026-07-15\n---\n\nEphemeral search canary",
        );
        let mut config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        config.search.engine = "qmd".into();
        config.search.qmd_collection = Some("persistent-target-must-not-run".into());

        let results = search("ephemeral", &config, &SearchFilters::default()).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Private Projection");
    }

    #[test]
    fn search_expands_vocabulary_aliases_with_provenance() {
        let _guard = crate::test_support::home_env_lock();
        let home = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("USERPROFILE", home.path());
        }

        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Writing Tools\ndate: 2026-05-01\ntype: meeting\n---\n\nWe discussed Automatic and Harper.",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters::default();
        let vocabulary = crate::vocabulary::VocabularyStore {
            entries: vec![crate::vocabulary::VocabularyEntry {
                kind: crate::vocabulary::VocabularyKind::Organization,
                canonical: "Automattic".into(),
                aliases: vec!["Automatic".into()],
                ..crate::vocabulary::VocabularyEntry::default()
            }],
        }
        .normalized()
        .unwrap();

        let results = search_with_mode_and_vocabulary(
            "Automattic",
            &config,
            &filters,
            crate::search_index::SyncMode::Force,
            Some(&vocabulary),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matched_via_alias.as_deref(), Some("Automatic"));
    }

    #[test]
    fn restricted_override_scans_once_and_preserves_alias_provenance() {
        let _guard = crate::test_home_env_lock();
        let home = TempDir::new().unwrap();
        unsafe {
            std::env::set_var("HOME", home.path());
            std::env::set_var("USERPROFILE", home.path());
        }
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "restricted.md",
            "---\ntitle: Private Writing Tools\ndate: 2026-05-01\ntype: meeting\nsensitivity: restricted\n---\n\nWe discussed Automatic.",
        );
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            include_restricted: true,
            ..Default::default()
        };
        let vocabulary = crate::vocabulary::VocabularyStore {
            entries: vec![crate::vocabulary::VocabularyEntry {
                kind: crate::vocabulary::VocabularyKind::Organization,
                canonical: "Automattic".into(),
                aliases: vec!["Automatic".into()],
                ..crate::vocabulary::VocabularyEntry::default()
            }],
        }
        .normalized()
        .unwrap();

        let results = search_with_mode_and_vocabulary(
            "Automattic",
            &config,
            &filters,
            crate::search_index::SyncMode::Force,
            Some(&vocabulary),
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].matched_via_alias.as_deref(), Some("Automatic"));
    }

    #[test]
    fn search_filters_by_recorded_by() {
        // Search hits the HOME-derived sqlite index; serialize with the
        // crate HOME-env lock or a concurrently HOME-swapping test yanks
        // the index mid-write (sqlite disk I/O error).
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ndate: 2026-03-17\nrecorded_by: Mat Silver\ntype: meeting\n---\n\nPricing discussion",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("mat".into()),
            include_restricted: false,
        };
        let non_matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("sarah".into()),
            include_restricted: false,
        };

        let matching_results = search("pricing", &config, &matching_filters).unwrap();
        let non_matching_results = search("pricing", &config, &non_matching_filters).unwrap();

        assert_eq!(matching_results.len(), 1);
        assert!(non_matching_results.is_empty());
    }

    #[test]
    fn search_empty_directory() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };

        let results = search("anything", &config, &filters).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn split_frontmatter_works() {
        let _guard = crate::test_support::home_env_lock();
        let content = "---\ntitle: Test\ndate: 2026-03-17\n---\n\nBody text here.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("title: Test"));
        assert!(body.contains("Body text here"));
    }

    #[test]
    fn extract_field_finds_value() {
        let _guard = crate::test_support::home_env_lock();
        let fm = "title: My Meeting\ndate: 2026-03-17\ntype: meeting";
        assert_eq!(extract_field(fm, "title"), Some("My Meeting".into()));
        assert_eq!(extract_field(fm, "type"), Some("meeting".into()));
        assert_eq!(extract_field(fm, "nonexistent"), None);
    }

    #[test]
    fn search_intents_returns_matching_structured_records() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n  - kind: commitment\n    what: Share revised pricing model\n    who: sarah\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };

        let overlay_db = dir.path().join("overlays.db");
        let canonical_root = dir.path().canonicalize().unwrap();
        let results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            &canonical_root,
            "pricing",
            &filters,
            &overlay_db,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Pricing Review");
        assert!(results
            .iter()
            .any(|item| item.kind == IntentKind::ActionItem));
        assert!(results
            .iter()
            .any(|item| item.kind == IntentKind::Commitment));
    }

    #[test]
    fn search_intents_filters_by_kind_and_owner() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n  - kind: commitment\n    what: Share revised pricing model\n    who: sarah\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: Some(IntentKind::Commitment),
            owner: Some("sarah".into()),
            recorded_by: None,
            include_restricted: false,
        };

        let overlay_db = dir.path().join("overlays.db");
        let canonical_root = dir.path().canonicalize().unwrap();
        let results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            &canonical_root,
            "",
            &filters,
            &overlay_db,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, IntentKind::Commitment);
        assert_eq!(results[0].who.as_deref(), Some("sarah"));
    }

    #[test]
    fn search_intents_resolves_owner_only_from_authorized_source_speaker_map() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nspeaker_map:\n  - speaker_label: SPEAKER_0\n    name: Alex Kim\n    confidence: high\n    source: manual\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: SPEAKER_0\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\n[SPEAKER_0 0:00] I'll send pricing.\n",
        );

        let overlay_db = dir.path().join("overlays.db");

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: Some(IntentKind::ActionItem),
            owner: Some("alex".into()),
            recorded_by: None,
            include_restricted: false,
        };

        let results = search_intents_at("", &config, &filters, &overlay_db).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].who.as_deref(), Some("Alex Kim"));
        assert_eq!(results[0].who_original.as_deref(), Some("SPEAKER_0"));
        assert_eq!(results[0].who_provenance.as_deref(), Some("speaker_map"));
    }

    #[test]
    fn search_intents_filter_by_recorded_by() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\nrecorded_by: Mat Silver\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("mat".into()),
            include_restricted: false,
        };
        let non_matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("sarah".into()),
            include_restricted: false,
        };

        let canonical_root = dir.path().canonicalize().unwrap();
        let matching_results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            &canonical_root,
            "",
            &matching_filters,
            &dir.path().join("overlays.db"),
        )
        .unwrap();
        let non_matching_results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            &canonical_root,
            "",
            &non_matching_filters,
            &dir.path().join("overlays.db"),
        )
        .unwrap();

        assert_eq!(matching_results.len(), 1);
        assert!(non_matching_results.is_empty());
    }

    #[test]
    fn consistency_report_flags_conflicts_and_stale_commitments() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: Pricing Decision\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at annual billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Send pricing doc\n    who: case\n    status: open\n    by_date: March 8\n---\n\n## Transcript\n\nPricing discussion.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: Pricing Revisit\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nPricing changed.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-c.md",
            "---\ntitle: Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nFollow-up.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-d.md",
            "---\ntitle: Another Follow-up\ntype: meeting\ndate: 2026-03-25T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nAnother follow-up.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert_eq!(report.decision_conflicts[0].topic, "pricing");
        assert_eq!(report.decision_conflicts[0].previous.len(), 1);
        assert_eq!(report.stale_commitments.len(), 1);
        assert_eq!(
            report.stale_commitments[0].entry.who.as_deref(),
            Some("case")
        );
        assert!(report.stale_commitments[0].meetings_since >= 3);
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("days old")));
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("newer meetings since")));
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("still open with due date March 8")));
        assert_eq!(
            report.stale_commitments[0]
                .latest_follow_up
                .as_ref()
                .map(|meeting| meeting.title.as_str()),
            Some("Another Follow-up")
        );
    }

    #[test]
    fn consistency_report_resolves_stale_owner_from_authorized_source_speaker_map() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2020-03-01-a.md",
            "---\ntitle: Follow-up Owner\ntype: meeting\ndate: 2020-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nspeaker_map:\n  - speaker_label: SPEAKER_0\n    name: Alex Kim\n    confidence: high\n    source: manual\nintents:\n  - kind: commitment\n    what: Send the rollout memo\n    who: SPEAKER_0\n    status: open\n    by_date: March 8\n---\n\n## Transcript\n\n[SPEAKER_0 0:00] I'll send it.\n",
        );

        let overlay_db = dir.path().join("overlays.db");

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let report = consistency_report_at(&config, Some("alex"), 7, &overlay_db).unwrap();

        assert_eq!(report.stale_commitments.len(), 1);
        let entry = &report.stale_commitments[0].entry;
        assert_eq!(entry.who.as_deref(), Some("Alex Kim"));
        assert_eq!(entry.who_original.as_deref(), Some("SPEAKER_0"));
        assert_eq!(entry.who_provenance.as_deref(), Some("speaker_map"));
    }

    #[test]
    fn consistency_report_marks_conflict_resolved_when_supersedes_is_set() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-02-28-a.md",
            "---\ntitle: Pricing Strategy\ntype: meeting\ndate: 2026-02-28T10:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing for consultants\n    topic: pricing\n    authority: high\nintents: []\n---\n\n## Transcript\n\nDecision A.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-b.md",
            "---\ntitle: Pricing Reversal\ntype: meeting\ndate: 2026-03-25T10:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Revert to annual-only billing across all segments\n    topic: pricing\n    authority: high\n    supersedes: \"2026-02-28 monthly billing decision\"\nintents: []\n---\n\n## Transcript\n\nDecision B reverses A.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        let conflict = &report.decision_conflicts[0];
        assert_eq!(conflict.topic, "pricing");
        assert!(conflict.resolution.is_some());
        assert!(conflict.resolution.as_ref().unwrap().contains("2026-02-28"));
        assert_eq!(conflict.latest.authority.as_deref(), Some("high"));
        assert_eq!(conflict.previous[0].authority.as_deref(), Some("high"));
    }

    #[test]
    fn consistency_report_leaves_resolution_none_without_supersedes() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay on annual billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nB without supersedes.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert!(report.decision_conflicts[0].resolution.is_none());
        assert!(report.decision_conflicts[0].latest.authority.is_none());
    }

    #[test]
    fn consistency_report_does_not_mark_resolution_when_other_conflicts_remain() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay annual only\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nB.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-c.md",
            "---\ntitle: C\ntype: meeting\ndate: 2026-03-25T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Test monthly billing for consultants only\n    topic: pricing\n    supersedes: \"2026-03-01 monthly billing decision\"\nintents: []\n---\n\n## Transcript\n\nC.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        let conflict = &report.decision_conflicts[0];
        assert_eq!(conflict.previous.len(), 2);
        assert!(conflict.resolution.is_none());
    }

    #[test]
    fn consistency_report_requires_supersedes_to_reference_the_prior_decision() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay annual only\n    topic: pricing\n    supersedes: \"some old plan\"\nintents: []\n---\n\n## Transcript\n\nB.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert!(report.decision_conflicts[0].resolution.is_none());
    }

    #[test]
    fn consistency_report_ignores_near_duplicate_decisions() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: Pricing Decision\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at 399 per month\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nPricing discussion.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: Pricing Follow-up\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at 399 per month.\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nPricing repeated.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert!(report.decision_conflicts.is_empty());
    }

    #[test]
    fn person_profile_aggregates_recent_meetings_topics_and_open_intents() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Share revised pricing model\n    who: Alex\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-b.md",
            "---\ntitle: Onboarding Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Review onboarding copy\n    who: Alex\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\nWe discussed onboarding.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let profile = person_profile(&config, "alex").unwrap();
        assert_eq!(profile.name, "alex");
        assert_eq!(profile.recent_meetings.len(), 2);
        assert_eq!(profile.open_intents.len(), 2);
        assert_eq!(profile.recent_decisions.len(), 1);
        assert!(profile
            .top_topics
            .iter()
            .any(|topic| topic.topic == "pricing"));
    }

    #[test]
    fn person_profile_matches_linked_people_entities() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: [Alex Chen]\nentities:\n  people:\n    - slug: sarah-chen\n      label: Alex Chen\n      aliases: [sarah]\n  projects:\n    - slug: pricing-review\n      label: Pricing Review\n      aliases: [pricing]\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let profile = person_profile(&config, "sarah").unwrap();
        assert_eq!(profile.recent_meetings.len(), 1);
        assert_eq!(profile.recent_meetings[0].title, "Pricing Review");
    }

    #[test]
    fn cross_meeting_research_collects_decisions_intents_and_meetings() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: [Alex]\nentities:\n  people:\n    - slug: sarah\n      label: Alex\n      aliases: []\n  projects:\n    - slug: pricing\n      label: Pricing\n      aliases: []\ncontext: pricing review\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Share revised pricing model\n    who: Alex\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-b.md",
            "---\ntitle: Onboarding Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nWe discussed onboarding.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
            include_restricted: false,
        };
        let report = cross_meeting_research("pricing", &config, &filters).unwrap();

        assert_eq!(report.related_decisions.len(), 1);
        assert_eq!(report.related_open_intents.len(), 1);
        assert_eq!(report.recent_meetings.len(), 1);
        assert_eq!(report.recent_meetings[0].title, "Pricing Review");
        assert!(report
            .related_topics
            .iter()
            .any(|topic| topic.topic == "pricing"));
    }

    #[test]
    fn find_open_actions_parses_frontmatter() {
        let _guard = crate::test_support::home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Test\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nstatus: complete\naction_items:\n  - assignee: mat\n    task: Send doc\n    status: open\n  - assignee: alex\n    task: Review PR\n    status: done\ndecisions: []\nintents: []\n---\n\nTranscript\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = find_open_actions(&config, None, false).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].assignee, "mat");
        assert_eq!(results[0].task, "Send doc");

        // Filter by assignee
        let filtered = find_open_actions(&config, Some("nobody"), false).unwrap();
        assert!(filtered.is_empty());
    }

    // ── Sensitivity enforcement (consent layer Wave 2) ──────────

    const RESTRICTED_MEETING: &str = "---\ntitle: Board Pricing Strategy\ntype: meeting\ndate: 2026-06-11T12:00:00-07:00\nduration: 30m\nstatus: complete\nsensitivity: restricted\nattendees: [Alex Kim]\npeople: [Alex Kim]\naction_items:\n  - assignee: Alex Kim\n    task: Draft board pricing memo\n    status: open\ndecisions:\n  - text: Hold the pricing floor at current levels\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Draft board pricing memo\n    who: Alex Kim\n    status: open\n---\n\n## Transcript\n\nRestricted pricing discussion.\n";

    const NORMAL_MEETING: &str = "---\ntitle: Pricing Sync\ntype: meeting\ndate: 2026-06-10T12:00:00-07:00\nduration: 30m\nstatus: complete\nattendees: [Sam Lee]\npeople: [Sam Lee]\naction_items:\n  - assignee: Sam Lee\n    task: Share pricing deck\n    status: open\ndecisions:\n  - text: Ship monthly pricing page\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Share pricing deck\n    who: Sam Lee\n    status: open\n---\n\n## Transcript\n\nWe discussed pricing.\n";

    #[test]
    fn aggregate_surfaces_keep_stable_peer_when_neighbors_are_invalid_or_oversized() {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "stable.md", NORMAL_MEETING);
        std::fs::write(dir.path().join("invalid.md"), [0xff, 0xfe, 0xfd]).unwrap();
        let oversized = std::fs::File::create(dir.path().join("oversized.md")).unwrap();
        oversized
            .set_len(crate::policy_fs::MAX_BOUND_TEXT_FILE_BYTES + 1)
            .unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        assert_eq!(
            search("pricing", &config, &SearchFilters::default())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            search_intents("pricing", &config, &SearchFilters::default())
                .unwrap()
                .len(),
            1
        );
        assert_eq!(find_open_actions(&config, None, false).unwrap().len(), 1);
        assert_eq!(
            person_profile(&config, "Sam Lee")
                .unwrap()
                .recent_meetings
                .len(),
            1
        );
        assert_eq!(
            cross_meeting_research("pricing", &config, &SearchFilters::default())
                .unwrap()
                .recent_meetings
                .len(),
            1
        );
        consistency_report(&config, None, 30).unwrap();
    }

    fn restricted_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "2026-06-10-pricing-sync.md", NORMAL_MEETING);
        create_test_file(dir.path(), "2026-06-11-board.md", RESTRICTED_MEETING);
        dir
    }

    #[test]
    fn direct_list_matches_policy_verified_private_projection() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        for filters in [
            SearchFilters::default(),
            SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        ] {
            let projected = with_stable_active_corpus(dir.path(), |revision| {
                search_with_mode_and_vocabulary_once(
                    "",
                    &config,
                    &filters,
                    SyncMode::Auto,
                    None,
                    revision,
                )
            })
            .unwrap();
            let direct = search_with_mode("", &config, &filters, SyncMode::Auto).unwrap();

            assert_eq!(
                serde_json::to_value(direct).unwrap(),
                serde_json::to_value(projected).unwrap()
            );
        }
    }

    #[test]
    fn search_excludes_restricted_meetings_by_default() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let default_results = search("pricing", &config, &SearchFilters::default()).unwrap();
        assert_eq!(default_results.len(), 1);
        assert_eq!(default_results[0].title, "Pricing Sync");

        let overridden = search(
            "pricing",
            &config,
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(overridden.len(), 2);
        assert!(overridden
            .iter()
            .any(|result| result.title == "Board Pricing Strategy"));
    }

    #[test]
    fn restricted_override_uses_exact_live_fts_semantics() {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "restricted.md",
            "---\ntitle: Private Plan\ntype: meeting\ndate: 2026-06-11T12:00:00-07:00\nsensitivity: restricted\n---\n\nThe roadmap comes first. Later we revisited prices and resumes at the cafe.\n",
        );
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            include_restricted: true,
            ..Default::default()
        };

        for query in ["pricing roadm", "café résu"] {
            let results = search(query, &config, &filters).unwrap();
            assert_eq!(results.len(), 1, "restricted FTS parity failed for {query}");
            assert_eq!(results[0].title, "Private Plan");
        }
    }

    #[test]
    fn aggregate_retry_replaces_pre_flip_result_with_exact_restricted_snapshot() {
        use std::cell::Cell;

        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            include_restricted: true,
            ..Default::default()
        };
        let flipped = Cell::new(false);

        let results = with_stable_active_corpus_with_hooks(
            dir.path(),
            |revision| {
                search_with_mode_and_vocabulary_once(
                    "pricing",
                    &config,
                    &filters,
                    SyncMode::Skip,
                    None,
                    revision,
                )
            },
            || {},
            || {
                if !flipped.replace(true) {
                    std::fs::write(
                        &path,
                        RESTRICTED_MEETING
                            .replace("Board Pricing Strategy", "POST-FLIP-RESTRICTED-TITLE"),
                    )
                    .unwrap();
                }
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "POST-FLIP-RESTRICTED-TITLE");
        assert!(results.iter().all(|result| result.title != "Pricing Sync"));
    }

    #[test]
    fn bad_good_bad_aba_file_never_enters_pre_attested_search_result() {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "stable.md", NORMAL_MEETING);
        let transient = dir.path().join("transient.md");
        std::fs::write(&transient, [0xff, 0xfe, 0xfd]).unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = with_stable_active_corpus_with_hooks(
            dir.path(),
            |revision| {
                search_with_mode_and_vocabulary_once(
                    "pricing",
                    &config,
                    &SearchFilters::default(),
                    SyncMode::Skip,
                    None,
                    revision,
                )
            },
            || {
                std::fs::write(
                    &transient,
                    NORMAL_MEETING.replace("Pricing Sync", "TRANSIENT ABA CANARY"),
                )
                .unwrap();
            },
            || {
                std::fs::write(&transient, [0xff, 0xfe, 0xfd]).unwrap();
            },
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Pricing Sync");
        assert!(results
            .iter()
            .all(|result| result.title != "TRANSIENT ABA CANARY"));
    }

    #[test]
    fn aggregate_churn_stops_at_the_shared_retry_budget() {
        use std::cell::Cell;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let operation_calls = Cell::new(0usize);
        let revision_number = Cell::new(0usize);

        let result = with_stable_active_corpus_with_hooks(
            dir.path(),
            |_revision| {
                operation_calls.set(operation_calls.get() + 1);
                Ok(())
            },
            || {},
            || {
                let next = revision_number.get() + 1;
                revision_number.set(next);
                std::fs::write(&path, format!("continuously changing revision {next}")).unwrap();
            },
        );

        assert!(result.is_err());
        assert_eq!(
            operation_calls.get(),
            ACTIVE_CORPUS_MAX_AUTHORIZATION_ATTEMPTS
        );
    }

    #[test]
    fn search_drops_policy_uncertain_meetings_even_with_override() {
        let _guard = crate::test_home_env_lock();
        let dir = restricted_test_dir();
        create_test_file(
            dir.path(),
            "2026-06-12-unknown.md",
            &NORMAL_MEETING.replace(
                "title: Pricing Sync",
                "title: Unknown Policy\nsensitivity: confidential",
            ),
        );
        create_test_file(
            dir.path(),
            "2026-06-13-malformed.md",
            "---\ntitle: Malformed Policy\ntype: meeting\ndate: [not valid\nsensitivity: restricted\n---\n\nPOLICY_UNCERTAIN_CANARY pricing\n",
        );
        for (suffix, title, sensitivity) in [
            ("null", "Null Policy", "null"),
            ("empty", "Empty Policy", ""),
            ("list", "List Policy", "[normal]"),
            ("map", "Map Policy", "{policy: normal}"),
        ] {
            create_test_file(
                dir.path(),
                &format!("2026-06-14-{suffix}.md"),
                &NORMAL_MEETING.replace(
                    "title: Pricing Sync",
                    &format!("title: {title}\nsensitivity: {sensitivity}"),
                ),
            );
        }
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = search(
            "pricing",
            &config,
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|result| result.title != "Unknown Policy"));
        for title in ["Null Policy", "Empty Policy", "List Policy", "Map Policy"] {
            assert!(
                results.iter().all(|result| result.title != title),
                "policy-uncertain meeting leaked with override: {title}"
            );
        }
        assert!(results
            .iter()
            .all(|result| !result.snippet.contains("POLICY_UNCERTAIN_CANARY")));
    }

    #[test]
    fn indexed_search_result_is_refreshed_from_live_verified_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let mut results = vec![SearchResult {
            path,
            title: "STALE_TITLE_CANARY".into(),
            date: "stale".into(),
            content_type: "meeting".into(),
            snippet: "STALE_RESTRICTED_CANARY".into(),
            matched_via_alias: None,
        }];
        let revision = stable_active_corpus_revision(dir.path()).unwrap();

        retain_policy_verified_results(
            &mut results,
            &SearchFilters::default(),
            "pricing",
            &revision,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Pricing Sync");
        assert!(!results[0].snippet.contains("STALE_RESTRICTED_CANARY"));
        assert!(results[0].snippet.contains("pricing"));
    }

    #[test]
    fn policy_verified_results_preserve_empty_query_list_mode() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let mut results = vec![SearchResult {
            path,
            title: "STALE_TITLE_CANARY".into(),
            date: "stale".into(),
            content_type: "meeting".into(),
            snippet: "STALE_SNIPPET_CANARY".into(),
            matched_via_alias: None,
        }];
        let revision = stable_active_corpus_revision(dir.path()).unwrap();

        retain_policy_verified_results(&mut results, &SearchFilters::default(), "", &revision)
            .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Pricing Sync");
        assert!(results[0].snippet.is_empty());
    }

    #[test]
    fn default_budget_supports_realistic_1500_meeting_multi_pass_list() {
        let dir = TempDir::new().unwrap();
        let filler = ".".repeat(25_800);
        let meeting = format!(
            "---\ntitle: Corpus Scale Meeting\ntype: meeting\ndate: 2026-07-23T12:00:00Z\nduration: 30m\nstatus: complete\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\n{filler}\n"
        );
        let aggregate_bytes = meeting.len() * 1_500;
        assert!(
            (38_000_000..=40_000_000).contains(&aggregate_bytes),
            "fixture should stay representative of the 1,399-file / 39 MB corpus"
        );
        for index in 0..1_500 {
            create_test_file(
                dir.path(),
                &format!("2026-07-23-corpus-scale-{index:04}.md"),
                &meeting,
            );
        }
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results =
            search_with_mode("", &config, &SearchFilters::default(), SyncMode::Skip).unwrap();

        assert_eq!(results.len(), 1_500);
        assert!(results
            .iter()
            .all(|result| result.title == "Corpus Scale Meeting"));
    }

    #[test]
    fn slug_scope_and_exact_path_share_restricted_read_policy() {
        let dir = TempDir::new().unwrap();
        let restricted = NORMAL_MEETING.replace(
            "title: Pricing Sync",
            "title: Private Pricing\nsensitivity: restricted",
        );
        create_test_file(dir.path(), "2026-07-15-private-pricing.md", &restricted);
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let slug_path = resolve_slug("private-pricing", &config)
            .expect("scope-only slug resolution must retain human mutation access");
        let exact_path = dir.path().join("2026-07-15-private-pricing.md");
        assert_eq!(slug_path, exact_path.canonicalize().unwrap());

        for candidate in [&slug_path, &exact_path] {
            assert!(read_authorized_meeting(candidate, &config, false).is_err());
            let authorized = read_authorized_meeting(candidate, &config, true).unwrap();
            assert_eq!(authorized.frontmatter.title, "Private Pricing");
        }
    }

    #[cfg(unix)]
    #[test]
    fn exact_path_slug_resolution_never_opens_unrelated_corpus_members() {
        use std::os::unix::ffi::OsStrExt;

        let dir = TempDir::new().unwrap();
        let fifo = dir.path().join("000-unrelated.md");
        let fifo_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo_path.as_ptr(), 0o600) }, 0);
        create_test_file(dir.path(), "target.md", NORMAL_MEETING);
        let exact = dir.path().join("target.md");
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let started = std::time::Instant::now();
        assert_eq!(
            resolve_slug(exact.to_str().unwrap(), &config),
            Some(exact.canonicalize().unwrap())
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn slug_resolution_charges_non_markdown_traversal_entries() {
        let dir = TempDir::new().unwrap();
        for index in 0..3 {
            std::fs::write(
                dir.path().join(format!("ignored-{index:04}.txt")),
                b"ignored",
            )
            .unwrap();
        }
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let budget =
            ActiveCorpusReadBudget::for_test(2, 8, 1_024, std::time::Duration::from_secs(1));

        assert_eq!(
            resolve_slug_with_budget("missing", &config, budget),
            Err(ActiveCorpusRevisionError::Budget)
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_agent_read_and_mutation_scope_reject_in_root_symlink() {
        use std::os::unix::fs::symlink;

        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "real.md", NORMAL_MEETING);
        let real = dir.path().join("real.md");
        let alias = dir.path().join("alias.md");
        symlink(&real, &alias).unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        assert!(open_meeting_mutation(&alias, &config).is_none());
        assert!(read_authorized_meeting(&alias, &config, true).is_err());
        assert!(real.exists());
        assert!(alias.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[test]
    fn exact_authorization_rejects_same_path_policy_or_byte_flip() {
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let path = dir.path().join("meeting.md");
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let authorized = read_authorized_meeting(&path, &config, false).unwrap();

        std::fs::write(
            &path,
            NORMAL_MEETING.replacen(
                "status: complete\n",
                "status: complete\nsensitivity: restricted\n",
                1,
            ),
        )
        .unwrap();
        assert!(authorized.reauthorize_exact(&config, false).is_err());

        std::fs::write(
            &path,
            NORMAL_MEETING.replace("Pricing Sync", "Different Normal Meeting"),
        )
        .unwrap();
        assert!(authorized.reauthorize_exact(&config, false).is_err());
    }

    #[test]
    fn authorized_search_snippet_is_derived_from_the_supplied_live_snapshot() {
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let path = dir.path().join("meeting.md");
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let first = read_authorized_meeting(&path, &config, false).unwrap();
        assert!(authorized_snapshot_search_snippet(&first, "pricing")
            .is_some_and(|snippet| snippet.contains("pricing")));

        std::fs::write(
            &path,
            NORMAL_MEETING
                .replace("Pricing Sync", "Roadmap Sync")
                .replace(
                    "We discussed pricing.",
                    "We discussed the release calendar instead.",
                ),
        )
        .unwrap();
        let current = read_authorized_meeting(&path, &config, false).unwrap();
        assert!(authorized_snapshot_search_snippet(&current, "pricing").is_none());
        assert!(authorized_snapshot_search_snippet(&current, "calendar")
            .is_some_and(|snippet| snippet.contains("calendar")));
    }

    #[test]
    fn authorized_mutation_binds_policy_and_exact_source_bytes() {
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let path = dir.path().join("meeting.md");
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mutation = open_authorized_meeting_mutation(&path, &config, false).unwrap();
        std::fs::write(
            &path,
            NORMAL_MEETING.replacen(
                "status: complete\n",
                "status: complete\nsensitivity: restricted\n",
                1,
            ),
        )
        .unwrap();
        assert!(mutation.archive_group(&[]).is_err());
        assert!(path.exists());
        assert!(!dir.path().join("archive").exists());
        drop(mutation);

        assert!(open_authorized_meeting_mutation(&path, &config, false).is_err());
        assert!(open_authorized_meeting_mutation(&path, &config, true).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn bound_delete_cannot_be_redirected_by_parent_directory_swap() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        let parent = root.join("nested");
        let retained_parent = root.join("retained-nested");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&parent).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        create_test_file(&parent, "meeting.md", NORMAL_MEETING);
        create_test_file(
            &outside,
            "meeting.md",
            &NORMAL_MEETING.replace("Pricing Sync", "OUTSIDE-DELETE-CANARY"),
        );
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&parent.join("meeting.md"), &config).unwrap();

        mutation
            .delete_source_with_hook(|| {
                std::fs::rename(&parent, &retained_parent).unwrap();
                symlink(&outside, &parent).unwrap();
            })
            .unwrap();

        assert!(!retained_parent.join("meeting.md").exists());
        assert!(outside.join("meeting.md").exists());
        assert!(std::fs::read_to_string(outside.join("meeting.md"))
            .unwrap()
            .contains("OUTSIDE-DELETE-CANARY"));
        assert!(parent.symlink_metadata().unwrap().file_type().is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn mutation_parent_open_rejects_real_directory_replacement() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        let nested = root.join("nested");
        let displaced = root.join("authorized-nested");
        let replacement = root.join("replacement-nested");
        create_test_file(&nested, "meeting.md", NORMAL_MEETING);
        create_test_file(&replacement, "meeting.md", NORMAL_MEETING);
        let canonical_root = root.canonicalize().unwrap();
        let snapshot = read_stable_active_markdown(&nested.join("meeting.md"), &canonical_root)
            .expect("initial stable snapshot");

        let mutation = meeting_mutation_from_snapshot_with_parent_open_hook(
            canonical_root,
            snapshot,
            |name| {
                if name == std::ffi::OsStr::new("nested") {
                    std::fs::rename(&nested, &displaced).unwrap();
                    std::fs::rename(&replacement, &nested).unwrap();
                }
            },
        );

        assert!(mutation.is_none());
        assert_eq!(
            std::fs::read_to_string(displaced.join("meeting.md")).unwrap(),
            NORMAL_MEETING
        );
        assert_eq!(
            std::fs::read_to_string(nested.join("meeting.md")).unwrap(),
            NORMAL_MEETING
        );
    }

    #[cfg(unix)]
    #[test]
    fn archive_and_staging_open_reject_real_directory_replacements() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(root.join("archive")).unwrap();
        std::fs::create_dir_all(root.join("archive-replacement")).unwrap();
        std::fs::write(root.join("archive/identity"), b"AUTHORIZED_ARCHIVE").unwrap();
        std::fs::write(
            root.join("archive-replacement/identity"),
            b"REPLACEMENT_ARCHIVE",
        )
        .unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();

        mutation
            .archive_group_with_all_hooks(
                &[],
                |_| {},
                |_| {},
                || {
                    std::fs::rename(root.join("archive"), root.join("archive-authorized")).unwrap();
                    std::fs::rename(root.join("archive-replacement"), root.join("archive"))
                        .unwrap();
                },
            )
            .expect_err("archive directory identity replacement must fail closed");
        assert!(source.exists());
        assert_eq!(
            std::fs::read(root.join("archive-authorized/identity")).unwrap(),
            b"AUTHORIZED_ARCHIVE"
        );
        assert_eq!(
            std::fs::read(root.join("archive/identity")).unwrap(),
            b"REPLACEMENT_ARCHIVE"
        );

        let mutation = open_meeting_mutation(&source, &config).unwrap();
        let staging_result = mutation.stage_delete_group_with_all_hooks(
            &[],
            |_| {},
            |_| {},
            || {
                let staging_name = delete_staging_entries(&root)
                    .into_iter()
                    .next()
                    .expect("new staging directory");
                let staging = root.join(&staging_name);
                let displaced = root.join(format!("{}-authorized", staging_name.to_string_lossy()));
                std::fs::rename(&staging, &displaced).unwrap();
                std::fs::create_dir(&staging).unwrap();
                std::fs::write(staging.join("identity"), b"REPLACEMENT_STAGING").unwrap();
            },
        );
        assert!(
            staging_result.is_err(),
            "staging directory identity replacement must fail closed"
        );
        assert!(source.exists());
        assert!(std::fs::read_dir(&root).unwrap().any(|entry| {
            let entry = entry.unwrap();
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(".delete-staging-")
                && entry.path().join("identity").exists()
        }));
    }

    #[cfg(unix)]
    #[test]
    fn claim_mismatch_with_recreated_source_surfaces_exact_quarantine() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let displaced = root.join("authorized-meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();
        let archive = mutation.archive_dir_with_hook(|| {}).unwrap();
        let archive_path = root.join("archive");

        let error = mutation
            .move_group_with_claim_hooks(
                &[],
                &archive,
                &archive_path,
                false,
                |index| {
                    if index == 0 {
                        std::fs::rename(&source, &displaced).unwrap();
                        std::fs::write(&source, b"CLAIM_REPLACEMENT").unwrap();
                    }
                },
                |index| {
                    if index == 0 {
                        std::fs::write(&source, b"SOURCE_RECREATION_BLOCKER").unwrap();
                    }
                },
                |_| {},
                |_| {},
            )
            .err()
            .expect("claim mismatch must fail closed");

        let claims = mutation_claim_entries(&archive_path);
        assert_eq!(claims.len(), 1);
        assert!(error.to_string().contains(&claims[0].display().to_string()));
        assert_eq!(std::fs::read(&claims[0]).unwrap(), b"CLAIM_REPLACEMENT");
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"SOURCE_RECREATION_BLOCKER"
        );
        assert_eq!(std::fs::read_to_string(displaced).unwrap(), NORMAL_MEETING);
        assert!(!archive_path.join("meeting.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn promotion_mismatch_with_recreated_source_surfaces_exact_quarantine() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();
        let archive = mutation.archive_dir_with_hook(|| {}).unwrap();
        let archive_path = root.join("archive");

        let error = mutation
            .move_group_with_claim_hooks(
                &[],
                &archive,
                &archive_path,
                false,
                |_| {},
                |_| {},
                |index| {
                    if index == 0 {
                        std::fs::write(archive_path.join("meeting.md"), b"PROMOTION_REPLACEMENT")
                            .unwrap();
                        std::fs::write(&source, b"SOURCE_RECREATION_BLOCKER").unwrap();
                    }
                },
                |_| {},
            )
            .err()
            .expect("promotion mismatch must fail closed");

        let claims = mutation_claim_entries(&archive_path);
        assert_eq!(claims.len(), 1);
        assert!(error.to_string().contains(&claims[0].display().to_string()));
        assert_eq!(std::fs::read(&claims[0]).unwrap(), b"PROMOTION_REPLACEMENT");
        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"SOURCE_RECREATION_BLOCKER"
        );
        assert!(!archive_path.join("meeting.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn read_only_meeting_can_archive_but_cannot_enter_delete_staging() {
        use std::os::unix::fs::PermissionsExt;

        let temp = TempDir::new().unwrap();
        let archive_root = temp.path().join("archive-case");
        std::fs::create_dir_all(&archive_root).unwrap();
        create_test_file(&archive_root, "meeting.md", NORMAL_MEETING);
        let archive_source = archive_root.join("meeting.md");
        std::fs::set_permissions(&archive_source, std::fs::Permissions::from_mode(0o444)).unwrap();
        let archive_config = Config {
            output_dir: archive_root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&archive_source, &archive_config).unwrap();
        mutation
            .archive_group(&[])
            .expect("directory-authorized rename must not require inode write access");
        assert_eq!(
            std::fs::read_to_string(archive_root.join("archive/meeting.md")).unwrap(),
            NORMAL_MEETING
        );

        let delete_root = temp.path().join("delete-case");
        std::fs::create_dir_all(&delete_root).unwrap();
        create_test_file(&delete_root, "meeting.md", NORMAL_MEETING);
        let delete_source = delete_root.join("meeting.md");
        std::fs::set_permissions(&delete_source, std::fs::Permissions::from_mode(0o444)).unwrap();
        let delete_config = Config {
            output_dir: delete_root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&delete_source, &delete_config).unwrap();
        assert!(
            mutation.stage_delete_group(&[]).is_err(),
            "deletion must bind writable exact handles before moving anything"
        );
        assert_eq!(
            std::fs::read_to_string(&delete_source).unwrap(),
            NORMAL_MEETING
        );
        assert_single_retained_empty_staging(&delete_root);
    }

    #[cfg(unix)]
    #[test]
    fn staged_delete_error_never_removes_interposed_empty_directory() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let authorized_source = root.join("authorized-meeting.md");
        let retained_staging = root.join("authorized-staging");
        let staging_name = std::cell::RefCell::new(None);
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();

        let result = mutation.stage_delete_group_with_hooks(
            &[],
            |index| {
                if index != 0 {
                    return;
                }
                let name = delete_staging_entries(&root)
                    .into_iter()
                    .next()
                    .expect("created staging directory");
                let ambient_staging = root.join(&name);
                std::fs::rename(&ambient_staging, &retained_staging).unwrap();
                std::fs::create_dir(&ambient_staging).unwrap();
                *staging_name.borrow_mut() = Some(name);

                std::fs::rename(&source, &authorized_source).unwrap();
                std::fs::write(&source, b"CLAIM_REPLACEMENT").unwrap();
            },
            |_| {},
        );

        assert!(result.is_err());
        let staging_name = staging_name.into_inner().expect("captured staging name");
        assert!(
            root.join(staging_name).is_dir(),
            "an interposed empty directory must never be removed by name"
        );
        assert!(
            std::fs::read_dir(&retained_staging)
                .unwrap()
                .next()
                .is_none(),
            "the exact staging directory is retained after rollback"
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"CLAIM_REPLACEMENT");
        assert_eq!(
            std::fs::read_to_string(authorized_source).unwrap(),
            NORMAL_MEETING
        );
    }

    #[test]
    fn archive_group_rolls_back_source_when_late_destination_collides() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        std::fs::write(&audio, b"audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        let error = mutation
            .archive_group_with_collision_after_first_move(std::slice::from_ref(&audio))
            .expect_err("late collision must fail the group move");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::AlreadyExists,
            "unexpected group-move error: {error}"
        );

        assert!(root.join("meeting.md").exists());
        assert_eq!(std::fs::read(&audio).unwrap(), b"audio canary");
        assert!(!root.join("archive/meeting.md").exists());
        assert_eq!(
            std::fs::read(root.join("archive/meeting.wav")).unwrap(),
            b"collision canary"
        );
    }

    #[test]
    fn archive_group_never_clobbers_a_source_created_during_rollback() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        std::fs::write(&audio, b"audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        let archive_audio = root.join("archive/meeting.wav");
        let replacement_source = root.join("meeting.md");
        let error = mutation
            .archive_group_with_hook(std::slice::from_ref(&audio), |index| {
                if index == 0 {
                    // Force the second destination claim to fail, then make
                    // rollback contend with a newly created source path.
                    std::fs::write(&archive_audio, b"destination collision canary").unwrap();
                    std::fs::write(&replacement_source, b"replacement source canary").unwrap();
                }
            })
            .expect_err("rollback collision must fail without replacing either file");
        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);

        assert_eq!(
            std::fs::read(&replacement_source).unwrap(),
            b"replacement source canary"
        );
        assert_eq!(std::fs::read(&audio).unwrap(), b"audio canary");
        assert!(root.join("archive/meeting.md").exists());
        assert_eq!(
            std::fs::read(&archive_audio).unwrap(),
            b"destination collision canary"
        );
    }

    #[test]
    fn archive_group_retains_optional_sibling_identity_from_selection() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        let displaced = root.join("authorized-meeting.wav");
        std::fs::write(&audio, b"authorized audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        assert!(mutation.sibling_exists(&audio));
        std::fs::rename(&audio, &displaced).unwrap();
        std::fs::write(&audio, b"replacement audio canary").unwrap();

        mutation
            .archive_group(std::slice::from_ref(&audio))
            .expect_err("a sibling replacement after selection must fail closed");
        assert!(root.join("meeting.md").exists());
        assert_eq!(std::fs::read(&audio).unwrap(), b"replacement audio canary");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"authorized audio canary"
        );
        assert!(!root.join("archive/meeting.md").exists());
        assert!(!root.join("archive/meeting.wav").exists());
    }

    #[cfg(unix)]
    #[test]
    fn archive_and_staged_delete_reject_source_symlink_swap_at_atomic_claim() {
        use std::os::unix::fs::symlink;

        for staged_delete in [false, true] {
            let temp = TempDir::new().unwrap();
            let root = temp.path().join("meetings");
            std::fs::create_dir_all(&root).unwrap();
            create_test_file(&root, "meeting.md", NORMAL_MEETING);
            let source = root.join("meeting.md");
            let displaced = root.join("authorized-meeting.md");
            let outside = temp.path().join("outside.md");
            std::fs::write(&outside, b"outside markdown canary").unwrap();
            let config = Config {
                output_dir: root.clone(),
                ..Config::default()
            };
            let mutation = open_meeting_mutation(&source, &config).unwrap();

            if staged_delete {
                let result = mutation.stage_delete_group_with_hooks(
                    &[],
                    |index| {
                        if index == 0 {
                            std::fs::rename(&source, &displaced).unwrap();
                            symlink(&outside, &source).unwrap();
                        }
                    },
                    |_| {},
                );
                assert!(
                    result.is_err(),
                    "staged deletion must reject a source symlink swap"
                );
            } else {
                mutation
                    .archive_group_with_hooks(
                        &[],
                        |index| {
                            if index == 0 {
                                std::fs::rename(&source, &displaced).unwrap();
                                symlink(&outside, &source).unwrap();
                            }
                        },
                        |_| {},
                    )
                    .expect_err("archive must reject a source symlink swap");
            }

            assert!(source.symlink_metadata().unwrap().file_type().is_symlink());
            assert_eq!(std::fs::read(&outside).unwrap(), b"outside markdown canary");
            assert_eq!(std::fs::read_to_string(&displaced).unwrap(), NORMAL_MEETING);
            assert!(!root.join("archive/meeting.md").exists());
            if staged_delete {
                assert_single_retained_empty_staging(&root);
            } else {
                assert!(delete_staging_entries(&root).is_empty());
            }
        }
    }

    // POSIX renames by source name and must reject a name winner installed at
    // the claim boundary. Windows renames the already-authorized exact handle,
    // so the safe result there is to move the old object while preserving the
    // new source-name winner; the adjacent atomic-transfer tests enforce that.
    #[cfg(unix)]
    #[test]
    fn archive_and_staged_delete_reject_source_inode_swap_at_atomic_claim() {
        for staged_delete in [false, true] {
            let temp = TempDir::new().unwrap();
            let root = temp.path().join("meetings");
            std::fs::create_dir_all(&root).unwrap();
            create_test_file(&root, "meeting.md", NORMAL_MEETING);
            let source = root.join("meeting.md");
            let displaced = root.join("authorized-meeting.md");
            let config = Config {
                output_dir: root.clone(),
                ..Config::default()
            };
            let mutation = open_meeting_mutation(&source, &config).unwrap();

            if staged_delete {
                let result = mutation.stage_delete_group_with_hooks(
                    &[],
                    |index| {
                        if index == 0 {
                            std::fs::rename(&source, &displaced).unwrap();
                            std::fs::write(&source, b"replacement markdown canary").unwrap();
                        }
                    },
                    |_| {},
                );
                assert!(
                    result.is_err(),
                    "staged deletion must reject a source inode swap"
                );
            } else {
                mutation
                    .archive_group_with_hooks(
                        &[],
                        |index| {
                            if index == 0 {
                                std::fs::rename(&source, &displaced).unwrap();
                                std::fs::write(&source, b"replacement markdown canary").unwrap();
                            }
                        },
                        |_| {},
                    )
                    .expect_err("archive must reject a source inode swap");
            }

            assert_eq!(
                std::fs::read(&source).unwrap(),
                b"replacement markdown canary"
            );
            assert_eq!(std::fs::read_to_string(&displaced).unwrap(), NORMAL_MEETING);
            assert!(!root.join("archive/meeting.md").exists());
            if staged_delete {
                assert_single_retained_empty_staging(&root);
            } else {
                assert!(delete_staging_entries(&root).is_empty());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn archive_group_rejects_sibling_symlink_swap_at_atomic_claim() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        let displaced = root.join("authorized-meeting.wav");
        let outside = temp.path().join("outside.wav");
        std::fs::write(&audio, b"authorized audio canary").unwrap();
        std::fs::write(&outside, b"outside audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();
        assert!(mutation.sibling_exists(&audio));

        mutation
            .archive_group_with_hooks(
                std::slice::from_ref(&audio),
                |index| {
                    if index == 1 {
                        std::fs::rename(&audio, &displaced).unwrap();
                        symlink(&outside, &audio).unwrap();
                    }
                },
                |_| {},
            )
            .expect_err("a symlink installed at the atomic claim must fail closed");

        assert!(root.join("meeting.md").exists());
        assert!(audio.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside audio canary");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"authorized audio canary"
        );
        assert!(!root.join("archive/meeting.md").exists());
        assert!(!root.join("archive/meeting.wav").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staged_delete_rejects_sibling_symlink_swap_at_atomic_claim() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        let displaced = root.join("authorized-meeting.wav");
        let outside = temp.path().join("outside.wav");
        std::fs::write(&audio, b"authorized audio canary").unwrap();
        std::fs::write(&outside, b"outside audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();
        assert!(mutation.sibling_exists(&audio));

        let result = mutation.stage_delete_group_with_hooks(
            std::slice::from_ref(&audio),
            |index| {
                if index == 1 {
                    std::fs::rename(&audio, &displaced).unwrap();
                    symlink(&outside, &audio).unwrap();
                }
            },
            |_| {},
        );
        assert!(
            result.is_err(),
            "a symlink installed at the staged claim must fail closed"
        );

        assert!(root.join("meeting.md").exists());
        assert!(audio.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside audio canary");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"authorized audio canary"
        );
        assert_single_retained_empty_staging(&root);
    }

    // See the source-swap test above: this fixture exercises POSIX's
    // name-based primitive. Windows binds and moves the exact sibling handle.
    #[cfg(unix)]
    #[test]
    fn archive_group_rejects_sibling_inode_swap_at_atomic_claim() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        let displaced = root.join("authorized-meeting.wav");
        std::fs::write(&audio, b"authorized audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();
        assert!(mutation.sibling_exists(&audio));

        mutation
            .archive_group_with_hooks(
                std::slice::from_ref(&audio),
                |index| {
                    if index == 1 {
                        std::fs::rename(&audio, &displaced).unwrap();
                        std::fs::write(&audio, b"replacement audio canary").unwrap();
                    }
                },
                |_| {},
            )
            .expect_err("a different inode installed at the atomic claim must fail closed");

        assert!(root.join("meeting.md").exists());
        assert_eq!(std::fs::read(&audio).unwrap(), b"replacement audio canary");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"authorized audio canary"
        );
        assert!(!root.join("archive/meeting.md").exists());
        assert!(!root.join("archive/meeting.wav").exists());
    }

    #[cfg(unix)]
    #[test]
    fn staged_delete_rejects_sibling_inode_swap_at_atomic_claim() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        let displaced = root.join("authorized-meeting.wav");
        std::fs::write(&audio, b"authorized audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();
        assert!(mutation.sibling_exists(&audio));

        let result = mutation.stage_delete_group_with_hooks(
            std::slice::from_ref(&audio),
            |index| {
                if index == 1 {
                    std::fs::rename(&audio, &displaced).unwrap();
                    std::fs::write(&audio, b"replacement audio canary").unwrap();
                }
            },
            |_| {},
        );
        assert!(
            result.is_err(),
            "a different inode installed at the staged claim must fail closed"
        );

        assert!(root.join("meeting.md").exists());
        assert_eq!(std::fs::read(&audio).unwrap(), b"replacement audio canary");
        assert_eq!(
            std::fs::read(&displaced).unwrap(),
            b"authorized audio canary"
        );
        assert_single_retained_empty_staging(&root);
    }

    #[test]
    fn archive_atomic_transfer_never_unlinks_a_new_source() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();

        mutation
            .archive_group_with_hook(&[], |index| {
                if index == 0 {
                    std::fs::write(&source, b"new atomic-save source canary").unwrap();
                }
            })
            .unwrap();

        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"new atomic-save source canary"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("archive/meeting.md")).unwrap(),
            NORMAL_MEETING
        );
    }

    #[test]
    fn staged_delete_atomic_transfer_never_unlinks_a_new_source() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();

        let staged = mutation
            .stage_delete_group_with_hooks(
                &[],
                |_| {},
                |index| {
                    if index == 0 {
                        std::fs::write(&source, b"new atomic-save source canary").unwrap();
                    }
                },
            )
            .unwrap();
        staged.finalize().unwrap();

        assert_eq!(
            std::fs::read(&source).unwrap(),
            b"new atomic-save source canary"
        );
        let staging_name = delete_staging_entries(&root)
            .into_iter()
            .next()
            .expect("deletion keeps one inactive recovery quarantine");
        #[cfg(windows)]
        assert!(
            std::fs::read_dir(root.join(&staging_name))
                .unwrap()
                .next()
                .is_none(),
            "Windows exact-handle deletion leaves an empty quarantine"
        );
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::metadata(root.join(&staging_name).join("meeting.md"))
                    .unwrap()
                    .len(),
                0
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn staged_delete_finalize_never_unlinks_a_replaced_staged_entry() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let source = root.join("meeting.md");
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&source, &config).unwrap();
        let staged = mutation.stage_delete_group(&[]).unwrap();
        let staging_name = delete_staging_entries(&root)
            .into_iter()
            .next()
            .expect("staged deletion directory");
        let staged_source = root.join(&staging_name).join("meeting.md");
        let retained_source = root.join(&staging_name).join("authorized-meeting.md");

        let result = staged.finalize_with_hook(|index| {
            if index == 0 {
                std::fs::rename(&staged_source, &retained_source).unwrap();
                std::fs::write(&staged_source, b"replacement staged canary").unwrap();
            }
        });

        result.expect("the exact retained inode can be sanitized after a pathname swap");
        assert_eq!(
            std::fs::read(&staged_source).unwrap(),
            b"replacement staged canary"
        );
        assert_eq!(std::fs::metadata(&retained_source).unwrap().len(), 0);
        assert!(root.join(staging_name).exists());
    }

    #[test]
    fn archive_group_rejects_nonregular_sibling_before_moving_source() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        std::fs::create_dir(&audio).unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        assert!(!mutation.sibling_exists(&audio));
        assert!(mutation
            .archive_group(std::slice::from_ref(&audio))
            .is_err());
        assert!(root.join("meeting.md").exists());
        assert!(!root.join("archive/meeting.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn archive_group_rejects_symlink_sibling_before_moving_source() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let outside = temp.path().join("outside-audio.wav");
        std::fs::write(&outside, b"outside canary").unwrap();
        let audio = root.join("meeting.wav");
        symlink(&outside, &audio).unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        assert!(!mutation.sibling_exists(&audio));
        assert!(mutation
            .archive_group(std::slice::from_ref(&audio))
            .is_err());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside canary");
        assert!(root.join("meeting.md").exists());
        assert!(!root.join("archive/meeting.md").exists());
    }

    #[test]
    fn staged_delete_hides_the_complete_group_before_physical_cleanup() {
        let temp = TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        std::fs::create_dir_all(&root).unwrap();
        create_test_file(&root, "meeting.md", NORMAL_MEETING);
        let audio = root.join("meeting.wav");
        std::fs::write(&audio, b"audio canary").unwrap();
        let config = Config {
            output_dir: root.clone(),
            ..Config::default()
        };
        let mutation = open_meeting_mutation(&root.join("meeting.md"), &config).unwrap();

        let staged = mutation
            .stage_delete_group(std::slice::from_ref(&audio))
            .unwrap();

        assert!(!root.join("meeting.md").exists());
        assert!(!audio.exists());
        let staging = std::fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .find(|name| name.to_string_lossy().starts_with(".delete-staging-"))
            .expect("staged group must remain privately retained until finalize");
        assert!(root.join(&staging).join("meeting.md").exists());
        assert_eq!(
            std::fs::read(root.join(&staging).join("meeting.wav")).unwrap(),
            b"audio canary"
        );

        staged.finalize().unwrap();
        let quarantine = root.join(staging);
        assert!(quarantine.exists());
        #[cfg(windows)]
        assert!(
            std::fs::read_dir(&quarantine).unwrap().next().is_none(),
            "Windows exact-handle deletion leaves an empty quarantine"
        );
        #[cfg(unix)]
        {
            assert_eq!(
                std::fs::metadata(quarantine.join("meeting.md"))
                    .unwrap()
                    .len(),
                0
            );
            assert_eq!(
                std::fs::metadata(quarantine.join("meeting.wav"))
                    .unwrap()
                    .len(),
                0
            );
        }
    }

    #[test]
    fn skip_mode_populates_fresh_private_projection() {
        let _guard = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        create_test_file(dir.path(), "meeting.md", NORMAL_MEETING);
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = search_with_mode(
            "pricing",
            &config,
            &SearchFilters::default(),
            SyncMode::Skip,
        )
        .unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Pricing Sync");
    }

    #[cfg(unix)]
    #[test]
    fn policy_verified_results_reject_symlink_escape() {
        use std::os::unix::fs::symlink;

        let root = TempDir::new().unwrap();
        let outside = TempDir::new().unwrap();
        create_test_file(outside.path(), "outside.md", NORMAL_MEETING);
        let link = root.path().join("linked.md");
        symlink(outside.path().join("outside.md"), &link).unwrap();
        let mut results = vec![SearchResult {
            path: link,
            title: "OUTSIDE_CANARY".into(),
            date: "stale".into(),
            content_type: "meeting".into(),
            snippet: "OUTSIDE_CANARY".into(),
            matched_via_alias: None,
        }];
        let revision = stable_active_corpus_revision(root.path()).unwrap();

        retain_policy_verified_results(
            &mut results,
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
            "pricing",
            &revision,
        )
        .unwrap();

        assert!(results.is_empty());
    }

    #[test]
    fn search_intents_excludes_restricted_meetings_by_default() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let default_results =
            search_intents("pricing", &config, &SearchFilters::default()).unwrap();
        assert_eq!(default_results.len(), 1);
        assert_eq!(default_results[0].what, "Share pricing deck");

        let overridden = search_intents(
            "pricing",
            &config,
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(overridden.len(), 2);
    }

    #[test]
    fn find_open_actions_excludes_restricted_meetings_by_default() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let default_results = find_open_actions(&config, None, false).unwrap();
        assert_eq!(default_results.len(), 1);
        assert_eq!(default_results[0].task, "Share pricing deck");

        let overridden = find_open_actions(&config, None, true).unwrap();
        assert_eq!(overridden.len(), 2);
        assert!(overridden
            .iter()
            .any(|action| action.task == "Draft board pricing memo"));
    }

    #[test]
    fn find_open_actions_drops_unknown_sensitivity_even_with_override() {
        let _guard = crate::test_home_env_lock();
        let dir = restricted_test_dir();
        create_test_file(
            dir.path(),
            "2026-06-12-unknown.md",
            &NORMAL_MEETING.replace(
                "title: Pricing Sync",
                "title: Unknown Policy\nsensitivity: confidential",
            ),
        );
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = find_open_actions(&config, None, true).unwrap();

        assert_eq!(results.len(), 2);
        assert!(results
            .iter()
            .all(|action| action.meeting_title != "Unknown Policy"));
    }

    #[test]
    fn cross_meeting_research_excludes_restricted_meetings_by_default() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = cross_meeting_research("pricing", &config, &SearchFilters::default()).unwrap();
        assert!(report
            .recent_meetings
            .iter()
            .all(|meeting| meeting.title != "Board Pricing Strategy"));
        assert!(report
            .related_decisions
            .iter()
            .all(|decision| !decision.what.contains("pricing floor")));

        let overridden = cross_meeting_research(
            "pricing",
            &config,
            &SearchFilters {
                include_restricted: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(overridden
            .recent_meetings
            .iter()
            .any(|meeting| meeting.title == "Board Pricing Strategy"));
    }

    #[test]
    fn person_profile_always_excludes_restricted_meetings() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        // Alex Kim only appears in the restricted meeting, so the profile
        // must come back empty — no override exists on this surface.
        let profile = person_profile(&config, "Alex Kim").unwrap();
        assert!(profile.recent_meetings.is_empty());
        assert!(profile.open_intents.is_empty());
        assert!(profile.recent_decisions.is_empty());
    }

    #[test]
    fn consistency_report_always_excludes_restricted_meetings() {
        let _guard = crate::test_support::home_env_lock();
        let dir = restricted_test_dir();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let overlay_db = TempDir::new().unwrap().path().join("overlays.db");
        let report = consistency_report_at(&config, None, 0, &overlay_db).unwrap();
        assert!(report
            .stale_commitments
            .iter()
            .all(|stale| stale.entry.what != "Draft board pricing memo"));
    }
}
