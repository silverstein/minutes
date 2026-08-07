//! Capability-bound ingestion and live-source verification for a local legal
//! text vault.
//!
//! This module is intentionally a narrow Gate 2 proof. It supports bounded
//! UTF-8 text and Markdown sources, keeps its FTS index in memory, retains
//! read-only file capabilities, and revalidates source membership, identity,
//! bytes, and revision before evidence leaves the vault.

use crate::retrieval::{
    interpret_legal_query, normalize_converted_document, normalize_text_document,
    normalize_transcribed_document, CurrentRevisionSet, DocumentId, LegalIndex, LegalQuery,
    LegalSearchResponse, NormalizedDocument, ProvisionBoundaries, RetrievalError, SourceRevision,
    VaultId, MAX_NORMALIZED_DOCUMENT_BYTES, MAX_QUERY_CHARS, MAX_SEMANTIC_PROVISIONS,
};
use crate::{
    cap_identity_matches, cap_metadata_identity_portable, cap_metadata_is_link_or_reparse,
    cap_metadata_is_multiply_linked, extension_for_name, open_approved_root, package_category,
    validate_approved_roots, ApprovedRoot, CensusError, FileIdentity,
};
use cap_std::fs::{Dir, File};
use minutes_archive_convert::{BoundedConverter, SourceFormat};
use minutes_archive_ocr::BoundedTranscriber;
use minutes_archive_semantic::{
    BoundedSemanticEngine, SemanticEmbedder, SemanticError, SemanticModelMetadata,
    MAX_SEMANTIC_INPUT_CHARS,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use thiserror::Error;

/// Raise this process's open-file ceiling before anything opens a document.
///
/// The vault holds one descriptor per indexed document, for the fence that
/// checks a source is still the file it was indexed from. macOS gives a GUI
/// application a soft `RLIMIT_NOFILE` of 256, so a build stopped at 237
/// documents and reported the rest as unreadable -- on an archive of 16,621.
///
/// It lives here rather than in the application's `main` because that is the
/// one place no test can reach. A terminal inherits a far higher soft limit
/// than launchd gives an app bundle, which is exactly why every local run
/// passed while the real thing failed; a harness has to be able to lower the
/// ceiling, call this, and check the build still completes.
#[cfg(unix)]
pub fn raise_open_file_ceiling() {
    // Enough for a very large archive plus the process's own descriptors,
    // without asking for an unbounded number.
    const WANTED: libc::rlim_t = 200_000;
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return;
    }
    if limit.rlim_cur >= WANTED {
        return;
    }
    let target = if limit.rlim_max == libc::RLIM_INFINITY {
        WANTED
    } else {
        WANTED.min(limit.rlim_max)
    };
    let raised = libc::rlimit {
        rlim_cur: target,
        rlim_max: limit.rlim_max,
    };
    // Best effort. A lower ceiling is reported honestly at the point it bites.
    unsafe {
        libc::setrlimit(libc::RLIMIT_NOFILE, &raised);
    }
}

#[cfg(not(unix))]
pub fn raise_open_file_ceiling() {}

pub const DOCUMENT_VAULT_SCHEMA: &str = "minutes.archive-document-vault.v1";

/// One folder the build must not enter, named within the root that contains it.
///
/// Bound to a single approved root on purpose. An exclusion used to be just a
/// relative path, which the walk compared against whichever root it happened to
/// be walking -- so skipping "attachments" in a notes vault also skipped an
/// unrelated `attachments` folder sitting at the top of a *different* approved
/// root, and the only trace was a count. Silently dropping documents an
/// attorney believes are indexed is the one failure this build cannot have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedFolder {
    /// Position of the approved root this exclusion belongs to.
    pub root_index: usize,
    /// Path of the folder relative to that root.
    pub relative_path: PathBuf,
}

// No longer `Copy`: the exclusion list owns its paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextVaultLimits {
    /// Folders the build must not enter, each within one approved root.
    pub excluded_paths: Vec<ExcludedFolder>,
    pub max_documents: usize,
    pub max_total_bytes: u64,
    pub max_directories: u64,
    pub max_depth: u32,
}

impl Default for TextVaultLimits {
    fn default() -> Self {
        Self {
            excluded_paths: Vec::new(),
            // A thirty-year practice is simply bigger than the numbers first
            // chosen here. These bound one session; nothing about correctness
            // or privacy rests on them, and the old 50,000 / 2 GiB stopped
            // short of a real archive of 16,620 artifacts across two folders.
            max_documents: 500_000,
            max_total_bytes: 32 * 1024 * 1024 * 1024,
            max_directories: 1_000_000,
            max_depth: 128,
        }
    }
}

impl TextVaultLimits {
    fn validate(self) -> Result<Self, VaultError> {
        if self.max_documents == 0
            || self.max_total_bytes == 0
            || self.max_directories == 0
            || self.max_depth == 0
        {
            return Err(VaultError::InvalidLimits);
        }
        Ok(self)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("the vault build limits must be greater than zero")]
    InvalidLimits,
    #[error("the approved archive authority is invalid")]
    InvalidAuthority,
    #[error("an approved archive location changed")]
    RootChanged,
    #[error("the vault build limits were exceeded; narrow the source set")]
    BuildBudgetExceeded,
    #[error("the vault build was cancelled; no partial vault was retained")]
    Cancelled,
    #[error("the private source index is unavailable")]
    IndexUnavailable,
    #[error("the bounded document converter is unavailable")]
    ConverterUnavailable,
    #[error("the lexical candidate budget was exceeded; narrow the query")]
    CandidateBudgetExceeded,
    #[error("the query could not be applied safely")]
    InvalidQuery,
    #[error("the in-memory semantic index exceeded its bounded pilot capacity")]
    SemanticBudgetExceeded,
    #[error("the bounded on-device semantic worker is unavailable")]
    SemanticUnavailable,
}

impl From<CensusError> for VaultError {
    fn from(error: CensusError) -> Self {
        match error {
            CensusError::RootChanged
            | CensusError::RootUnavailable { .. }
            | CensusError::RootNotDirectory { .. }
            | CensusError::RootIsLink { .. } => Self::RootChanged,
            _ => Self::InvalidAuthority,
        }
    }
}

impl From<RetrievalError> for VaultError {
    fn from(error: RetrievalError) -> Self {
        match error {
            RetrievalError::InvalidVaultScope
            | RetrievalError::InvalidDocumentIdentity
            | RetrievalError::InvalidTitle
            | RetrievalError::InvalidDocumentText
            | RetrievalError::TooManyProvisions
            | RetrievalError::InvalidQuery
            | RetrievalError::ScopeMismatch => Self::InvalidQuery,
            RetrievalError::CandidateBudgetExceeded => Self::CandidateBudgetExceeded,
            RetrievalError::IndexUnavailable => Self::IndexUnavailable,
            RetrievalError::InvalidSemanticVector => Self::IndexUnavailable,
            RetrievalError::SemanticBudgetExceeded => Self::SemanticBudgetExceeded,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TextVaultBuildReport {
    pub schema: &'static str,
    pub vault_id: VaultId,
    pub approved_locations: u64,
    pub indexed_documents: u64,
    pub indexed_bytes: u64,
    pub unsupported_files_skipped: u64,
    pub oversized_files_skipped: u64,
    pub malformed_text_files_skipped: u64,
    pub conversion_failures: u64,
    pub ocr_required_files: u64,
    pub searchable_pdf_documents: u64,
    pub inferred_boundary_documents: u64,
    pub docx_documents: u64,
    pub duplicate_files_skipped: u64,
    pub symlinks_skipped: u64,
    pub hard_links_skipped: u64,
    /// True when a size or count limit stopped the build before the end of
    /// the approved folders. The index is real but partial.
    pub budget_reached: bool,
    pub documents_left_unread: u64,
    /// The file exists and could not be opened. Almost always permissions.
    ///
    /// Split out of a single `metadata_errors` bucket after a real archive
    /// reported 10,429 of roughly 16,600 documents "could not be read" and the
    /// number was unanswerable: five different failures shared one counter, and
    /// permission denied reads very differently from a file changing mid-read.
    pub permission_denied: u64,
    /// Could not be opened for some reason that is NOT permissions.
    pub unopenable: u64,
    /// The process ran out of file descriptors before reaching these.
    pub open_file_limit_reached: u64,
    /// Page images the recognizer could not read, or that held no text.
    pub scans_unreadable: u64,
    /// The entry could not be stat'd at all.
    pub entries_unstattable: u64,
    /// No stable device/inode identity, so the file could not be pinned.
    pub identity_unavailable: u64,
    /// The file changed between being seen and being read, so it was refused.
    pub changed_while_reading: u64,
    /// Folders not descended into, because the tree was deeper than the limit.
    pub directories_left_unread: u64,
    /// Folders the operator chose not to index.
    pub excluded_directories: u64,
    /// Scans that were read. Searchable, but only ever as transcriptions.
    pub transcribed_documents: u64,
    pub metadata_errors: u64,
    pub directory_errors: u64,
    pub source_content_persisted: bool,
    pub retrieval_index_persisted: bool,
    pub converter_sandbox_verified: bool,
    pub semantic_worker_sandbox_verified: bool,
    pub semantic_retrieval_enabled: bool,
    pub semantic_model: Option<SemanticModelMetadata>,
    pub semantic_provisions_indexed: u64,
    pub semantic_provisions_skipped: u64,
    pub semantic_unavailable: bool,
    /// The semantic worker died partway through, so suggestions cover only the
    /// documents read before it went.
    ///
    /// Distinct from `semantic_unavailable`, which means there were never any
    /// suggestions. Partial coverage has to be said out loud: exact search is
    /// complete either way, but a reader who assumes suggestions swept the
    /// whole archive would be wrong, and this tool's job is to never let a
    /// result read as more complete than it is.
    pub semantic_coverage_partial: bool,
    pub semantic_derivatives_persisted: bool,
    pub semantic_model_download_requested: bool,
    pub supported_formats: Vec<&'static str>,
}

struct AuthorizedVaultRoot {
    approval: ApprovedRoot,
    directory: Dir,
}

impl std::fmt::Debug for AuthorizedVaultRoot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthorizedVaultRoot([redacted capability])")
    }
}

struct AuthorizedSource {
    root_index: usize,
    relative_path: PathBuf,
    identity: FileIdentity,
    file: File,
    indexed_revision: SourceRevision,
}

impl std::fmt::Debug for AuthorizedSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AuthorizedSource([redacted capability and path])")
    }
}

/// A non-serializable, in-memory vault bound to folder-picker authorities.
pub struct AuthorizedTextVault {
    vault_id: VaultId,
    roots: Vec<AuthorizedVaultRoot>,
    sources: BTreeMap<DocumentId, AuthorizedSource>,
    index: LegalIndex,
    semantic_engine: Option<BoundedSemanticEngine>,
    build_report: TextVaultBuildReport,
}

pub type AuthorizedDocumentVault = AuthorizedTextVault;
pub type DocumentVaultBuildReport = TextVaultBuildReport;
pub type DocumentVaultLimits = TextVaultLimits;

impl std::fmt::Debug for AuthorizedTextVault {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AuthorizedTextVault")
            .field("vault_id", &self.vault_id)
            .field("roots", &self.roots.len())
            .field("sources", &self.sources.len())
            .field("index", &"[private in-memory sqlite]")
            .field(
                "semantic_engine",
                &self.semantic_engine.as_ref().map(|_| "[bounded worker]"),
            )
            .finish()
    }
}

impl AuthorizedTextVault {
    pub fn vault_id(&self) -> &VaultId {
        &self.vault_id
    }

    /// The real path of an indexed document, for revealing it in Finder.
    ///
    /// The interface never receives paths -- cards carry an opaque document id
    /// and the IPC layer strips filenames and paths from everything it sends.
    /// That invariant is kept here: the caller passes an id back, this resolves
    /// it against the roots the vault already holds, and only the Rust side
    /// ever sees the path. Nothing about it reaches the webview.
    ///
    /// Refuses unless the file is still the one that was indexed, checked the
    /// same way a quotation is: same device and inode, still a regular file,
    /// still inside its approved root. Revealing a path that now names
    /// something else would point counsel at the wrong document.
    pub fn source_path_for_reveal(&self, document_id: &DocumentId) -> Option<PathBuf> {
        let source = self.sources.get(document_id)?;
        let root = self.roots.get(source.root_index)?;
        let path = root.approval.canonical_path().join(&source.relative_path);
        let metadata = std::fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return None;
        }
        let current = crate::portable_identity_for(&metadata)?;
        if current != source.identity {
            return None;
        }
        Some(path)
    }

    pub fn build_report(&self) -> &TextVaultBuildReport {
        &self.build_report
    }

    pub fn interpret_and_search(
        &self,
        raw_query: impl Into<String>,
    ) -> Result<LegalSearchResponse, VaultError> {
        let query = interpret_legal_query(raw_query).map_err(VaultError::from)?;
        self.search(query)
    }

    pub fn search(&self, query: LegalQuery) -> Result<LegalSearchResponse, VaultError> {
        self.revalidate_roots()?;
        let has_lexical_constraints =
            !query.required_concepts.is_empty() || query.exact_phrase.is_some();

        let indexed_revisions = self
            .sources
            .iter()
            .map(|(document_id, source)| (document_id.clone(), source.indexed_revision.clone()))
            .collect::<Vec<_>>();
        let mut revisions = CurrentRevisionSet::default();
        for (document_id, revision) in indexed_revisions {
            revisions.insert(document_id, revision);
        }
        let mut response = if has_lexical_constraints {
            self.index
                .search(&self.vault_id, query.clone(), &revisions)
                .map_err(VaultError::from)?
        } else {
            if query.raw.trim().is_empty()
                || query.raw.chars().count() > MAX_QUERY_CHARS
                || query.limit == 0
            {
                return Err(VaultError::InvalidQuery);
            }
            LegalSearchResponse {
                query: query.clone(),
                evidence: Vec::new(),
                documents: Vec::new(),
                semantic_suggestions: Vec::new(),
                transcriptions: Vec::new(),
                lexical_candidates_considered: 0,
                semantic_candidates_considered: 0,
                semantic_query_applied: false,
                semantic_model: self.index.semantic_model().cloned(),
                stale_evidence_withdrawn: 0,
                inferred_boundary_evidence_withdrawn: 0,
                stale_document_ids: BTreeSet::new(),
            }
        };

        if let (Some(indexed_model), Some(semantic_engine)) =
            (self.index.semantic_model(), self.semantic_engine.as_ref())
        {
            let pinned_model = SemanticModelMetadata::apple_english_sentence_revision_one();
            if *indexed_model != pinned_model {
                return Err(VaultError::SemanticUnavailable);
            }
            match semantic_engine.embed_once(&query.raw) {
                Ok(query_vector) => {
                    let semantic = self
                        .index
                        .semantic_search(&self.vault_id, &query_vector, &revisions, query.limit)
                        .map_err(VaultError::from)?;
                    let verified = response
                        .evidence
                        .iter()
                        .map(|card| (card.document_id.clone(), card.source_anchor.clone()))
                        .collect::<BTreeSet<_>>();
                    response.semantic_suggestions = semantic
                        .suggestions
                        .into_iter()
                        .filter(|card| {
                            !verified
                                .contains(&(card.document_id.clone(), card.source_anchor.clone()))
                        })
                        .filter(|card| {
                            query
                                .max_sentences
                                .is_none_or(|maximum| card.sentence_count <= maximum)
                        })
                        .take(query.limit)
                        .collect();
                    response.semantic_candidates_considered = semantic.candidates_considered;
                    response.semantic_query_applied = true;
                    response.semantic_model = semantic.model;
                    response
                        .stale_document_ids
                        .extend(semantic.stale_document_ids);
                    response.stale_evidence_withdrawn = response.stale_document_ids.len() as u64;
                }
                Err(_) if !has_lexical_constraints => {
                    return Err(VaultError::SemanticUnavailable);
                }
                Err(_) => {}
            }
        }
        if !has_lexical_constraints && !response.semantic_query_applied {
            return Err(VaultError::InvalidQuery);
        }

        let result_documents = response
            .evidence
            .iter()
            .map(|card| card.document_id.clone())
            .chain(
                response
                    .documents
                    .iter()
                    .map(|card| card.document_id.clone()),
            )
            .chain(
                response
                    .semantic_suggestions
                    .iter()
                    .map(|card| card.document_id.clone()),
            )
            .collect::<BTreeSet<_>>();
        let mut withdrawn = BTreeSet::new();
        for document_id in result_documents {
            let Some(source) = self.sources.get(&document_id) else {
                withdrawn.insert(document_id);
                continue;
            };
            if !self.source_is_current(source) {
                withdrawn.insert(document_id);
            }
        }

        response
            .evidence
            .retain(|card| !withdrawn.contains(&card.document_id));
        response
            .documents
            .retain(|card| !withdrawn.contains(&card.document_id));
        response
            .semantic_suggestions
            .retain(|card| !withdrawn.contains(&card.document_id));
        response.stale_document_ids.extend(withdrawn);
        response.stale_evidence_withdrawn = response.stale_document_ids.len() as u64;
        Ok(response)
    }

    fn revalidate_roots(&self) -> Result<(), VaultError> {
        for root in &self.roots {
            open_approved_root(&root.approval).map_err(VaultError::from)?;
            let retained_metadata = root
                .directory
                .dir_metadata()
                .map_err(|_| VaultError::RootChanged)?;
            if !cap_identity_matches(&retained_metadata, root.approval.identity) {
                return Err(VaultError::RootChanged);
            }
        }
        Ok(())
    }

    fn source_is_current(&self, source: &AuthorizedSource) -> bool {
        let Some(root) = self.roots.get(source.root_index) else {
            return false;
        };
        if !relative_source_identity_matches(
            &root.directory,
            &source.relative_path,
            source.identity,
        ) {
            return false;
        }
        let Ok(mut file) = source.file.try_clone() else {
            return false;
        };
        let Ok(before) = file.metadata() else {
            return false;
        };
        // Link status is rechecked here, not only at indexing. A file that was
        // singly linked when indexed can be hard-linked from OUTSIDE the
        // approved root afterwards, and an independent reviewer confirmed the
        // evidence card kept being returned with nothing withdrawn. Indexing
        // refused multiply linked files; display did not, so the authority
        // rule held only at the moment of the build.
        if !cap_identity_matches(&before, source.identity)
            || cap_metadata_is_multiply_linked(&before)
            || before.len() > MAX_NORMALIZED_DOCUMENT_BYTES as u64
        {
            return false;
        }
        if file.seek(SeekFrom::Start(0)).is_err() {
            return false;
        }
        let Ok(bytes) = read_bounded(&mut file, MAX_NORMALIZED_DOCUMENT_BYTES) else {
            return false;
        };
        let Ok(after) = file.metadata() else {
            return false;
        };
        // ...and again after reading, so a link created during the read is
        // caught too.
        cap_identity_matches(&after, source.identity)
            && !cap_metadata_is_multiply_linked(&after)
            && after.len() == before.len()
            && SourceRevision::from_bytes(&bytes) == source.indexed_revision
            && relative_source_identity_matches(
                &root.directory,
                &source.relative_path,
                source.identity,
            )
    }
}

#[derive(Debug)]
struct PendingDirectory {
    root_index: usize,
    relative_path: PathBuf,
    directory: Dir,
    depth: u32,
}

/// A document that has passed every identity check and been read, waiting for
/// the one expensive step: conversion or transcription.
///
/// It carries the whole of what the cheap tail needs, because a batch is
/// drained after the walk has already moved on from the folder the document
/// came from.
struct PendingDocument {
    document_id: DocumentId,
    title: String,
    bytes: Vec<u8>,
    source_kind: SourceKind,
    identity: FileIdentity,
    file: File,
    /// The length seen after the read, which is what `indexed_bytes` counts.
    source_len: u64,
    root_index: usize,
    relative_path: PathBuf,
}

/// How many documents to extract at once.
///
/// Two cores are held back: one for the walk that keeps the batch fed, and one
/// so the Mac stays usable while a build of tens of thousands of documents
/// runs. Twelve is the ceiling because past that the sandboxed workers are
/// contending for memory bandwidth rather than adding throughput, and every
/// one of them is a process.
fn extraction_batch_size() -> usize {
    std::thread::available_parallelism()
        .map(|cores| cores.get().saturating_sub(2))
        .unwrap_or(1)
        .clamp(1, 12)
}

/// A batch also stops at a byte ceiling.
///
/// A single document may be up to `MAX_NORMALIZED_DOCUMENT_BYTES`, so a batch
/// counted only by document count could hold a multiple of that in memory at
/// once alongside the text each conversion returns. The sequential walk never
/// held more than one source's bytes; this bounds how far that goes.
const MAX_PENDING_EXTRACTION_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Default)]
struct BuildCounters {
    indexed_documents: u64,
    /// Set when a limit stopped the build short of the whole folder.
    budget_reached: bool,
    documents_left_unread: u64,
    permission_denied: u64,
    unopenable: u64,
    open_file_limit_reached: u64,
    scans_unreadable: u64,
    entries_unstattable: u64,
    identity_unavailable: u64,
    changed_while_reading: u64,
    directories_left_unread: u64,
    excluded_directories: u64,
    transcribed_documents: u64,
    indexed_bytes: u64,
    unsupported_files_skipped: u64,
    oversized_files_skipped: u64,
    malformed_text_files_skipped: u64,
    conversion_failures: u64,
    ocr_required_files: u64,
    searchable_pdf_documents: u64,
    inferred_boundary_documents: u64,
    docx_documents: u64,
    duplicate_files_skipped: u64,
    symlinks_skipped: u64,
    hard_links_skipped: u64,
    metadata_errors: u64,
    directory_errors: u64,
    directories_scanned: u64,
    semantic_provisions_indexed: u64,
    semantic_provisions_skipped: u64,
    semantic_unavailable: bool,
    semantic_coverage_partial: bool,
}

/// Live counts for a build in flight.
///
/// Lock-free because the build loop touches it once per file and must not pay
/// for the reporting. Read by the interface while the build runs, which is the
/// whole point: a build over tens of thousands of documents with no visible
/// progress is indistinguishable from a hung one, and someone waiting on it
/// cannot tell whether to keep waiting or give up.
#[derive(Debug, Default)]
pub struct BuildProgress {
    examined: AtomicU64,
    indexed: AtomicU64,
}

impl BuildProgress {
    pub fn examined(&self) -> u64 {
        self.examined.load(Ordering::Relaxed)
    }

    pub fn indexed(&self) -> u64 {
        self.indexed.load(Ordering::Relaxed)
    }

    fn note_examined(&self) {
        self.examined.fetch_add(1, Ordering::Relaxed);
    }

    fn note_indexed(&self) {
        self.indexed.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn build_authorized_text_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
) -> Result<AuthorizedTextVault, VaultError> {
    build_authorized_vault(
        vault_id,
        approved_roots,
        limits,
        cancelled,
        None,
        None,
        None,
        None,
        None,
    )
}

// Eight arguments, each a distinct capability the caller decides on:
// identity, roots, limits, cancellation, conversion, transcription, progress
// and semantics. Bundling them into a struct would hide which are optional.
#[allow(clippy::too_many_arguments)]
pub fn build_authorized_document_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
    converter: &BoundedConverter,
    // Optional: a Mac without the recogniser still indexes everything else,
    // and scans stay counted rather than blocking the build.
    transcriber: Option<&BoundedTranscriber>,
    progress: Option<&BuildProgress>,
    // Optional by design. The inner builder already accepted None; only this
    // wrapper insisted on an engine, which is what made a Mac without Apple's
    // linguistic asset unable to build any index at all.
    semantic_engine: Option<BoundedSemanticEngine>,
) -> Result<AuthorizedTextVault, VaultError> {
    build_authorized_vault(
        vault_id,
        approved_roots,
        limits,
        cancelled,
        Some(converter),
        transcriber,
        progress,
        semantic_engine,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn build_authorized_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
    converter: Option<&BoundedConverter>,
    transcriber: Option<&BoundedTranscriber>,
    progress: Option<&BuildProgress>,
    semantic_engine: Option<BoundedSemanticEngine>,
    // Stands in for the worker session, for tests only.
    //
    // The failure worth testing is a worker that answers for a while and then
    // stops -- it discarded a complete index in the field. With only a real
    // worker to test against, every run has either a healthy worker or none,
    // and the case in between is unreachable. Production passes `None` and
    // opens a real sandboxed session below.
    injected_embedder: Option<&mut dyn SemanticEmbedder>,
) -> Result<AuthorizedTextVault, VaultError> {
    let limits = limits.validate()?;
    validate_approved_roots(approved_roots).map_err(VaultError::from)?;
    let mut roots = Vec::with_capacity(approved_roots.len());
    let mut pending = Vec::with_capacity(approved_roots.len());

    for (root_index, approval) in approved_roots.iter().enumerate() {
        let directory = open_approved_root(approval).map_err(VaultError::from)?;
        let traversal = directory.try_clone().map_err(|_| VaultError::RootChanged)?;
        roots.push(AuthorizedVaultRoot {
            approval: approval.clone(),
            directory,
        });
        pending.push(PendingDirectory {
            root_index,
            relative_path: PathBuf::new(),
            directory: traversal,
            depth: 0,
        });
    }

    let mut index = LegalIndex::new(vault_id.clone()).map_err(VaultError::from)?;
    let mut sources = BTreeMap::new();
    let mut identities = HashSet::new();
    let mut counters = BuildCounters::default();
    let batch_size = extraction_batch_size();
    let mut pending_documents: Vec<PendingDocument> = Vec::with_capacity(batch_size);
    let mut pending_bytes: u64 = 0;
    // Document ids used to be `indexed_documents + 1`, which tied an opaque
    // identifier to a counter that now moves in a different place. They are
    // handed out here instead, monotonically, and a document that fails
    // extraction simply leaves a gap. Nothing reads meaning out of them.
    let mut next_document_number: u64 = 0;
    // Semantic suggestions are an optional aid, explicitly labelled in the UI
    // as review-not-verified, while exact evidence is the product. Failing the
    // whole build when Apple's on-device model is unavailable left the
    // operator with NO index at all -- no lexical search either -- on a Mac
    // that simply lacks the linguistic asset. The interface already says
    // "Semantic suggestions are unavailable on this Mac"; this makes the code
    // agree with it. Withhold the capability, not the product.
    let mut semantic_session = match semantic_engine
        .as_ref()
        .map(BoundedSemanticEngine::open_session)
        .transpose()
    {
        Ok(session) => session,
        Err(_) => {
            counters.semantic_unavailable = true;
            None
        }
    };
    let mut semantic_session: Option<&mut dyn SemanticEmbedder> = match injected_embedder {
        Some(embedder) => Some(embedder),
        None => semantic_session
            .as_mut()
            .map(|session| session as &mut dyn SemanticEmbedder),
    };
    let semantic_model = semantic_session
        .as_ref()
        .map(|_| SemanticModelMetadata::apple_english_sentence_revision_one());

    while let Some(current) = pending.pop() {
        if cancelled.load(Ordering::Acquire) {
            return Err(VaultError::Cancelled);
        }
        // Partial, like the count and byte limits. No limit in this build
        // discards what has already been read.
        if counters.directories_scanned >= limits.max_directories {
            counters.budget_reached = true;
            continue;
        }
        let entries = match current.directory.entries() {
            Ok(entries) => entries,
            Err(_) => {
                counters.directory_errors = counters.directory_errors.saturating_add(1);
                continue;
            }
        };
        counters.directories_scanned = counters.directories_scanned.saturating_add(1);

        for entry_result in entries {
            if cancelled.load(Ordering::Acquire) {
                return Err(VaultError::Cancelled);
            }
            let entry = match entry_result {
                Ok(entry) => entry,
                Err(_) => {
                    counters.directory_errors = counters.directory_errors.saturating_add(1);
                    continue;
                }
            };
            let name = entry.file_name();
            let metadata = match current.directory.symlink_metadata(&name) {
                Ok(metadata) => metadata,
                Err(_) => {
                    counters.entries_unstattable = counters.entries_unstattable.saturating_add(1);
                    continue;
                }
            };
            if cap_metadata_is_link_or_reparse(&metadata) {
                counters.symlinks_skipped = counters.symlinks_skipped.saturating_add(1);
                continue;
            }
            // A hard link is not a symlink, so the check above passes it
            // through. Indexing one pulls an inode from outside the approved
            // root into the vault, where it is returned as evidence under a
            // title that looks local.
            if cap_metadata_is_multiply_linked(&metadata) {
                counters.hard_links_skipped = counters.hard_links_skipped.saturating_add(1);
                continue;
            }
            if metadata.is_dir() {
                if package_category(&extension_for_name(&name)).is_some() {
                    counters.unsupported_files_skipped =
                        counters.unsupported_files_skipped.saturating_add(1);
                    continue;
                }
                // Folders the operator excluded are not entered at all.
                //
                // The only unit of choice used to be a whole approved folder,
                // so a notes vault with 2,873 screenshots and recordings under
                // one subfolder had to be indexed whole or not at all. An
                // exclusion names one folder inside one root: it covers that
                // folder and everything beneath it, and a folder of the same
                // name in another approved root is untouched.
                let relative = current.relative_path.join(&name);
                if limits.excluded_paths.iter().any(|excluded| {
                    excluded.root_index == current.root_index
                        && (relative == excluded.relative_path
                            || relative.starts_with(&excluded.relative_path))
                }) {
                    counters.excluded_directories = counters.excluded_directories.saturating_add(1);
                    continue;
                }
                // A tree deeper than the limit is not descended into, and the
                // build carries on rather than failing over one branch.
                if current.depth >= limits.max_depth {
                    counters.budget_reached = true;
                    counters.directories_left_unread =
                        counters.directories_left_unread.saturating_add(1);
                    continue;
                }
                let expected = cap_metadata_identity_portable(&metadata);
                let child = match entry.open_dir() {
                    Ok(child) => child,
                    Err(_) => {
                        counters.directory_errors = counters.directory_errors.saturating_add(1);
                        continue;
                    }
                };
                let opened_metadata = match child.dir_metadata() {
                    Ok(metadata) => metadata,
                    Err(_) => {
                        counters.entries_unstattable =
                            counters.entries_unstattable.saturating_add(1);
                        continue;
                    }
                };
                if cap_metadata_is_link_or_reparse(&opened_metadata)
                    || expected
                        .zip(cap_metadata_identity_portable(&opened_metadata))
                        .is_some_and(|(before, after)| before != after)
                {
                    counters.symlinks_skipped = counters.symlinks_skipped.saturating_add(1);
                    continue;
                }
                pending.push(PendingDirectory {
                    root_index: current.root_index,
                    relative_path: current.relative_path.join(&name),
                    directory: child,
                    depth: current.depth + 1,
                });
                continue;
            }
            if !metadata.is_file() {
                counters.unsupported_files_skipped =
                    counters.unsupported_files_skipped.saturating_add(1);
                continue;
            }
            if let Some(progress) = progress {
                progress.note_examined();
            }
            let Some(source_kind) = source_kind(&name, converter.is_some(), transcriber.is_some())
            else {
                counters.unsupported_files_skipped =
                    counters.unsupported_files_skipped.saturating_add(1);
                continue;
            };
            if metadata.len() == 0 || metadata.len() > MAX_NORMALIZED_DOCUMENT_BYTES as u64 {
                counters.oversized_files_skipped =
                    counters.oversized_files_skipped.saturating_add(1);
                continue;
            }
            // Reaching a budget stops the build; it does not throw it away.
            //
            // This used to return an error, so a folder one document over the
            // limit produced nothing at all after however long it had already
            // spent reading. An index of the first 50,000 documents, clearly
            // labelled as partial, is worth having; an error after an hour is
            // worth nothing. The counters below are what the summary uses to
            // say so.
            //
            // A budget is decided by the counters, and the counters only move
            // when a batch reaches the tail below. So when the documents
            // already waiting could carry the build over a limit, drain them
            // first: the check that follows then sees exactly the numbers a
            // sequential walk would have put in front of it.
            if counters
                .indexed_documents
                .saturating_add(pending_documents.len() as u64) as usize
                >= limits.max_documents
                || counters
                    .indexed_bytes
                    .saturating_add(pending_bytes)
                    .saturating_add(metadata.len())
                    > limits.max_total_bytes
            {
                index_extracted_batch(
                    &mut pending_documents,
                    &mut pending_bytes,
                    converter,
                    transcriber,
                    &mut index,
                    &mut sources,
                    &mut counters,
                    &mut semantic_session,
                    semantic_model.as_ref(),
                    progress,
                )?;
            }
            if counters.indexed_documents as usize >= limits.max_documents
                || counters.indexed_bytes.saturating_add(metadata.len()) > limits.max_total_bytes
            {
                counters.budget_reached = true;
                counters.documents_left_unread = counters.documents_left_unread.saturating_add(1);
                continue;
            }
            let Some(identity) = cap_metadata_identity_portable(&metadata) else {
                counters.identity_unavailable = counters.identity_unavailable.saturating_add(1);
                continue;
            };
            if !identities.insert(identity) {
                counters.duplicate_files_skipped =
                    counters.duplicate_files_skipped.saturating_add(1);
                continue;
            }

            let mut file = match current.directory.open(&name) {
                Ok(file) => file,
                Err(error) => {
                    // Ask the error what happened rather than assuming. The
                    // first version of this counter was labelled "permission
                    // denied" on the strength of a guess, and a real archive
                    // then reported 10,418 of them -- a number stated as a
                    // cause without ever measuring one.
                    match error.kind() {
                        std::io::ErrorKind::PermissionDenied => {
                            counters.permission_denied =
                                counters.permission_denied.saturating_add(1);
                        }
                        // Named exactly, because it was twice reported as a
                        // permissions problem and sent the owner looking at
                        // Full Disk Access for files that were never
                        // unreadable. The vault holds one descriptor per
                        // indexed document for the live-source fence, so a low
                        // ceiling stops a build dead: on a real archive of
                        // 16,620 files it indexed 237 -- the GUI soft limit is
                        // 256 -- and called the other 10,418 a permissions
                        // failure.
                        _ if matches!(
                            error.raw_os_error(),
                            Some(libc::EMFILE) | Some(libc::ENFILE)
                        ) =>
                        {
                            counters.open_file_limit_reached =
                                counters.open_file_limit_reached.saturating_add(1);
                        }
                        _ => {
                            counters.unopenable = counters.unopenable.saturating_add(1);
                        }
                    }
                    continue;
                }
            };
            let opened_metadata = match file.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    counters.entries_unstattable = counters.entries_unstattable.saturating_add(1);
                    continue;
                }
            };
            if !cap_identity_matches(&opened_metadata, identity) {
                counters.changed_while_reading = counters.changed_while_reading.saturating_add(1);
                continue;
            }
            let bytes = match read_bounded(&mut file, MAX_NORMALIZED_DOCUMENT_BYTES) {
                Ok(bytes) => bytes,
                Err(_) => {
                    counters.oversized_files_skipped =
                        counters.oversized_files_skipped.saturating_add(1);
                    continue;
                }
            };
            let post_read_metadata = match file.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    counters.entries_unstattable = counters.entries_unstattable.saturating_add(1);
                    continue;
                }
            };
            if !cap_identity_matches(&post_read_metadata, identity)
                || post_read_metadata.len() != opened_metadata.len()
            {
                counters.changed_while_reading = counters.changed_while_reading.saturating_add(1);
                continue;
            }

            next_document_number = next_document_number.saturating_add(1);
            let document_id = DocumentId::parse(format!("document-{next_document_number:016x}"))
                .map_err(VaultError::from)?;
            let Some(title) = source_title(&name) else {
                counters.malformed_text_files_skipped =
                    counters.malformed_text_files_skipped.saturating_add(1);
                continue;
            };
            // Everything above is cheap and touches shared state; the one
            // expensive step is deferred to a batch so it can run on more than
            // one core.
            pending_bytes = pending_bytes.saturating_add(post_read_metadata.len());
            pending_documents.push(PendingDocument {
                document_id,
                title,
                bytes,
                source_kind,
                identity,
                file,
                source_len: post_read_metadata.len(),
                root_index: current.root_index,
                relative_path: current.relative_path.join(&name),
            });
            if pending_documents.len() >= batch_size
                || pending_bytes >= MAX_PENDING_EXTRACTION_BYTES
            {
                index_extracted_batch(
                    &mut pending_documents,
                    &mut pending_bytes,
                    converter,
                    transcriber,
                    &mut index,
                    &mut sources,
                    &mut counters,
                    &mut semantic_session,
                    semantic_model.as_ref(),
                    progress,
                )?;
            }
        }
    }

    // Whatever the walk ended holding. Cancellation is deliberately not
    // rechecked here: the entry loop is where a stop is answered, and a build
    // that reached this line was not cancelled.
    index_extracted_batch(
        &mut pending_documents,
        &mut pending_bytes,
        converter,
        transcriber,
        &mut index,
        &mut sources,
        &mut counters,
        &mut semantic_session,
        semantic_model.as_ref(),
        progress,
    )?;

    let build_report = TextVaultBuildReport {
        schema: DOCUMENT_VAULT_SCHEMA,
        vault_id: vault_id.clone(),
        approved_locations: roots.len() as u64,
        indexed_documents: counters.indexed_documents,
        indexed_bytes: counters.indexed_bytes,
        unsupported_files_skipped: counters.unsupported_files_skipped,
        oversized_files_skipped: counters.oversized_files_skipped,
        malformed_text_files_skipped: counters.malformed_text_files_skipped,
        conversion_failures: counters.conversion_failures,
        ocr_required_files: counters.ocr_required_files,
        searchable_pdf_documents: counters.searchable_pdf_documents,
        inferred_boundary_documents: counters.inferred_boundary_documents,
        docx_documents: counters.docx_documents,
        duplicate_files_skipped: counters.duplicate_files_skipped,
        symlinks_skipped: counters.symlinks_skipped,
        hard_links_skipped: counters.hard_links_skipped,
        budget_reached: counters.budget_reached,
        documents_left_unread: counters.documents_left_unread,
        permission_denied: counters.permission_denied,
        unopenable: counters.unopenable,
        open_file_limit_reached: counters.open_file_limit_reached,
        scans_unreadable: counters.scans_unreadable,
        entries_unstattable: counters.entries_unstattable,
        identity_unavailable: counters.identity_unavailable,
        changed_while_reading: counters.changed_while_reading,
        directories_left_unread: counters.directories_left_unread,
        excluded_directories: counters.excluded_directories,
        transcribed_documents: counters.transcribed_documents,
        metadata_errors: counters.metadata_errors,
        directory_errors: counters.directory_errors,
        source_content_persisted: false,
        retrieval_index_persisted: false,
        converter_sandbox_verified: converter.is_some(),
        semantic_worker_sandbox_verified: semantic_engine.is_some(),
        semantic_retrieval_enabled: semantic_session.is_some()
            && counters.semantic_provisions_indexed > 0,
        semantic_model,
        semantic_provisions_indexed: counters.semantic_provisions_indexed,
        semantic_provisions_skipped: counters.semantic_provisions_skipped,
        semantic_unavailable: counters.semantic_unavailable,
        semantic_coverage_partial: counters.semantic_coverage_partial,
        semantic_derivatives_persisted: false,
        semantic_model_download_requested: false,
        supported_formats: if converter.is_some() {
            vec![
                ".bmp", ".doc", ".docx", ".gif", ".heic", ".heif", ".jpeg", ".jpg", ".md", ".odt",
                ".pdf", ".png", ".rtf", ".text", ".tif", ".tiff", ".txt",
            ]
        } else {
            vec![".md", ".text", ".txt"]
        },
    };
    Ok(AuthorizedTextVault {
        vault_id,
        roots,
        sources,
        index,
        semantic_engine,
        build_report,
    })
}

/// Why a document produced no text.
///
/// One variant per counter the build keeps, so extraction can run away from
/// the counters and still leave the report a sequential build produced.
#[derive(Debug)]
enum ExtractionOutcome {
    MalformedText,
    ConversionFailed,
    /// A PDF that carries no text layer: a picture of a page.
    OcrRequired,
    /// A page image the recognizer could not read, or that held no text.
    ///
    /// Split from `OcrRequired` because sharing it told a reader "2,565 PDFs
    /// require OCR" for an archive holding 447 PDFs -- the number was almost
    /// entirely scans that failed, reported as something they are not.
    ScanUnreadable,
    /// Not a coverage gap and not counted: `source_kind` only classifies a
    /// convertible format when a converter exists, so reaching this means the
    /// build was misconfigured. Carried back rather than counted because that
    /// is what the sequential build did with it -- fail, do not quietly skip.
    ConverterUnavailable,
}

/// The one expensive step, with no access to the index, the counters, or
/// anything else the build shares.
///
/// Pulled out so a batch of documents can go through it at once. Each
/// conversion and each transcription already spawns its own fresh sandboxed
/// worker process, which is why running several at a time does not weaken the
/// isolation: it is the same guarantee several times over, no worker
/// outliving its document and none able to see another's bytes.
fn extract_document(
    document_id: &DocumentId,
    title: &str,
    bytes: &[u8],
    source_kind: SourceKind,
    converter: Option<&BoundedConverter>,
    transcriber: Option<&BoundedTranscriber>,
) -> Result<NormalizedDocument, ExtractionOutcome> {
    let normalized = match source_kind {
        SourceKind::Text => normalize_text_document(document_id.clone(), title, bytes),
        SourceKind::Converted(format) => {
            let Some(converter) = converter else {
                return Err(ExtractionOutcome::ConverterUnavailable);
            };
            let converted = match converter.convert(format, bytes) {
                Ok(converted) => converted,
                // Every conversion failure is a coverage gap, never a reason
                // to lose the build.
                //
                // This used to abort on any error that was not a refusal or a
                // timeout, so one document the worker could not handle
                // discarded everything indexed before it. A folder of 32,605
                // artifacts spent six minutes reading and returned "the
                // bounded document converter is unavailable" and nothing else.
                // That is the opposite of how the rest of this build behaves:
                // unreadable, malformed, oversized and duplicate files are all
                // counted and reported so the gap is visible. A worker that
                // fell over on one file is the same kind of fact.
                Err(_) => return Err(ExtractionOutcome::ConversionFailed),
            };
            if converted.blocks.is_empty()
                && converted
                    .warnings
                    .iter()
                    .any(|warning| warning == "ocr_required_or_no_extractable_text")
            {
                return Err(ExtractionOutcome::OcrRequired);
            }
            normalize_converted_document(document_id.clone(), title, bytes, &converted)
        }
        SourceKind::Scanned => {
            let Some(transcriber) = transcriber else {
                // Unreachable: a scan is only classified as one when a
                // recogniser exists. Counted rather than failing the build, so
                // a bad classification cannot lose a folder.
                return Err(ExtractionOutcome::ScanUnreadable);
            };
            let page = match transcriber.transcribe(bytes) {
                Ok(page) => page,
                // A scan that cannot be read is still a coverage gap the
                // reader should see, and it is the same gap as a PDF with no
                // text layer.
                Err(_) => return Err(ExtractionOutcome::ScanUnreadable),
            };
            let text = page
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if text.trim().is_empty() {
                // A photograph, a blank scan, a separator sheet.
                return Err(ExtractionOutcome::ScanUnreadable);
            }
            normalize_transcribed_document(
                document_id.clone(),
                title,
                bytes,
                minutes_archive_ocr::TRANSCRIBER,
                &[(1, text, page.lowest_confidence())],
            )
        }
    };
    normalized.map_err(|_| match source_kind {
        SourceKind::Text => ExtractionOutcome::MalformedText,
        SourceKind::Converted(_) => ExtractionOutcome::ConversionFailed,
        SourceKind::Scanned => ExtractionOutcome::ScanUnreadable,
    })
}

/// Extract a batch at once, then index the results one at a time, in the order
/// the walk found them.
///
/// The split is the whole point. Extraction is a sandboxed process per
/// document -- about half a second for a converted format, longer for a scan
/// -- and a real archive of 16,620 artifacts spent twenty-five minutes doing
/// that on one core while eleven sat idle. Everything after it is cheap and
/// touches state that is not shared safely: the index, the source map, the
/// counters, and a semantic session that is one worker.
#[allow(clippy::too_many_arguments)]
fn index_extracted_batch(
    pending_documents: &mut Vec<PendingDocument>,
    pending_bytes: &mut u64,
    converter: Option<&BoundedConverter>,
    transcriber: Option<&BoundedTranscriber>,
    index: &mut LegalIndex,
    sources: &mut BTreeMap<DocumentId, AuthorizedSource>,
    counters: &mut BuildCounters,
    semantic_session: &mut Option<&mut dyn SemanticEmbedder>,
    semantic_model: Option<&SemanticModelMetadata>,
    progress: Option<&BuildProgress>,
) -> Result<(), VaultError> {
    *pending_bytes = 0;
    let batch = std::mem::take(pending_documents);
    if batch.is_empty() {
        return Ok(());
    }
    let extracted: Vec<Result<NormalizedDocument, ExtractionOutcome>> =
        std::thread::scope(|scope| {
            let workers = batch
                .iter()
                .map(|pending| {
                    scope.spawn(move || {
                        extract_document(
                            &pending.document_id,
                            &pending.title,
                            &pending.bytes,
                            pending.source_kind,
                            converter,
                            transcriber,
                        )
                    })
                })
                .collect::<Vec<_>>();
            workers
                .into_iter()
                .map(|worker| match worker.join() {
                    Ok(outcome) => outcome,
                    // A worker thread does nothing but extract, so a panic
                    // there is the same bug it would have been inline. Let it
                    // out rather than record it as a coverage gap.
                    Err(panic) => std::panic::resume_unwind(panic),
                })
                .collect()
        });

    for (pending, extracted) in batch.into_iter().zip(extracted) {
        let normalized = match extracted {
            Ok(normalized) => normalized,
            Err(ExtractionOutcome::MalformedText) => {
                counters.malformed_text_files_skipped =
                    counters.malformed_text_files_skipped.saturating_add(1);
                continue;
            }
            Err(ExtractionOutcome::ConversionFailed) => {
                counters.conversion_failures = counters.conversion_failures.saturating_add(1);
                continue;
            }
            Err(ExtractionOutcome::ScanUnreadable) => {
                counters.scans_unreadable = counters.scans_unreadable.saturating_add(1);
                continue;
            }
            Err(ExtractionOutcome::OcrRequired) => {
                counters.ocr_required_files = counters.ocr_required_files.saturating_add(1);
                continue;
            }
            Err(ExtractionOutcome::ConverterUnavailable) => {
                return Err(VaultError::ConverterUnavailable)
            }
        };
        // A semantic worker that dies partway through must not cost the
        // operator the index.
        //
        // Binding the engine was already optional -- a Mac without Apple's
        // linguistic asset still gets exact search -- but the engine failing
        // *during* a build returned `Err`, which discards everything. On a real
        // archive that is sixteen thousand documents and twenty-two minutes of
        // reading thrown away because an optional aid stopped working, and the
        // operator is told only "the bounded on-device semantic worker is
        // unavailable". Exact evidence is the product; suggestions are the aid.
        if counters.semantic_coverage_partial {
            *semantic_session = None;
        }
        if let Some(session) = semantic_session.as_deref_mut() {
            let remaining = MAX_SEMANTIC_PROVISIONS
                .saturating_sub(counters.semantic_provisions_indexed as usize);
            let mut embeddings = Vec::with_capacity(normalized.provisions.len());
            for (position, provision) in normalized.provisions.iter().enumerate() {
                if position >= remaining {
                    counters.semantic_provisions_skipped =
                        counters.semantic_provisions_skipped.saturating_add(1);
                    embeddings.push(None);
                    continue;
                }
                let text = semantic_provision_text(
                    &normalized.title,
                    provision.heading.as_deref(),
                    &provision.text,
                );
                match text.as_deref().map(|text| session.embed(text)) {
                    Some(Ok(vector)) => embeddings.push(Some(vector)),
                    Some(Err(
                        SemanticError::InputBudgetExceeded | SemanticError::InvalidVector,
                    ))
                    | None => {
                        counters.semantic_provisions_skipped =
                            counters.semantic_provisions_skipped.saturating_add(1);
                        embeddings.push(None);
                    }
                    // The worker itself is gone, not just unhappy with one
                    // provision. Stop asking it -- retrying a dead worker once
                    // per provision for the rest of the archive is its own
                    // stall -- and index everything from here without
                    // suggestions.
                    Some(Err(
                        SemanticError::PlatformUnavailable
                        | SemanticError::ModelUnavailable
                        | SemanticError::ExecutableUnavailable
                        | SemanticError::SecurityBoundaryUnavailable
                        | SemanticError::WorkerBudgetExceeded
                        | SemanticError::WorkerFailed,
                    )) => {
                        counters.semantic_coverage_partial = true;
                        counters.semantic_provisions_skipped =
                            counters.semantic_provisions_skipped.saturating_add(1);
                        embeddings.push(None);
                    }
                }
                if counters.semantic_coverage_partial {
                    // Stop asking a dead worker, but finish the vector first.
                    // `replace_document_with_semantics` requires one entry per
                    // provision and rejects a short one -- so simply breaking
                    // here turned a graceful degradation into a *different*
                    // fatal error, and discarded the build anyway. It survived
                    // review because the first test's documents happened to
                    // have an even number of provisions, so the worker always
                    // died between documents rather than inside one.
                    while embeddings.len() < normalized.provisions.len() {
                        counters.semantic_provisions_skipped =
                            counters.semantic_provisions_skipped.saturating_add(1);
                        embeddings.push(None);
                    }
                    break;
                }
            }
            match semantic_model.cloned() {
                Some(model) => {
                    match index.replace_document_with_semantics(&normalized, model, &embeddings) {
                        Ok(indexed) => {
                            counters.semantic_provisions_indexed = counters
                                .semantic_provisions_indexed
                                .saturating_add(indexed as u64);
                        }
                        // The index refused the suggestion vectors -- too many
                        // of them, or a shape it will not store. That is a
                        // reason to index this document without suggestions,
                        // never to throw away every document already read.
                        Err(
                            RetrievalError::InvalidSemanticVector
                            | RetrievalError::SemanticBudgetExceeded,
                        ) => {
                            counters.semantic_coverage_partial = true;
                            index
                                .replace_document(&normalized)
                                .map_err(VaultError::from)?;
                        }
                        // A genuinely broken index is fatal, and only this is.
                        Err(error) => return Err(VaultError::from(error)),
                    }
                }
                // No pinned model means nothing can be stored as a suggestion,
                // which is a reason to index without them, not to refuse.
                None => {
                    counters.semantic_coverage_partial = true;
                    index
                        .replace_document(&normalized)
                        .map_err(VaultError::from)?;
                }
            }
        } else {
            index
                .replace_document(&normalized)
                .map_err(VaultError::from)?;
        }
        let source_kind = pending.source_kind;
        let source_len = pending.source_len;
        sources.insert(
            pending.document_id,
            AuthorizedSource {
                root_index: pending.root_index,
                relative_path: pending.relative_path,
                identity: pending.identity,
                file: pending.file,
                indexed_revision: normalized.revision,
            },
        );
        counters.indexed_documents = counters.indexed_documents.saturating_add(1);
        if let Some(progress) = progress {
            progress.note_indexed();
        }
        if normalized.provision_boundaries == ProvisionBoundaries::Inferred {
            counters.inferred_boundary_documents =
                counters.inferred_boundary_documents.saturating_add(1);
        }
        match source_kind {
            SourceKind::Converted(SourceFormat::Pdf) => {
                counters.searchable_pdf_documents =
                    counters.searchable_pdf_documents.saturating_add(1);
            }
            SourceKind::Converted(SourceFormat::Docx) => {
                counters.docx_documents = counters.docx_documents.saturating_add(1);
            }
            // Counted with DOCX rather than in a category of their own. The
            // number exists so counsel can see how much of a folder became
            // searchable, and "word-processor documents" is the distinction
            // that carries meaning there; splitting out the container format
            // would not change any decision.
            SourceKind::Converted(SourceFormat::Doc)
            | SourceKind::Converted(SourceFormat::Odt)
            | SourceKind::Converted(SourceFormat::Rtf) => {
                counters.docx_documents = counters.docx_documents.saturating_add(1);
            }
            // Counted separately from the word-processor formats: a scan that
            // was read is searchable, but only as a transcription, and lumping
            // it in with documents that carry their own text would overstate
            // what the archive can actually quote.
            SourceKind::Scanned => {
                counters.transcribed_documents = counters.transcribed_documents.saturating_add(1);
            }
            SourceKind::Text => {}
        }
        counters.indexed_bytes = counters.indexed_bytes.saturating_add(source_len);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum SourceKind {
    Text,
    Converted(SourceFormat),
    /// A page image, read by the recogniser rather than parsed.
    Scanned,
}

fn source_kind(
    name: &OsStr,
    converter_available: bool,
    transcriber_available: bool,
) -> Option<SourceKind> {
    match extension_for_name(name).as_str() {
        ".md" | ".text" | ".txt" => Some(SourceKind::Text),
        ".pdf" if converter_available => Some(SourceKind::Converted(SourceFormat::Pdf)),
        ".docx" if converter_available => Some(SourceKind::Converted(SourceFormat::Docx)),
        // Word 97-2003, OpenDocument Text and RTF. A thirty-year practice
        // keeps most of its older agreements in these, and until now they were
        // counted as unsupported and never read. `.docm` is deliberately
        // absent: it is a macro-enabled container, and nothing here needs to
        // open one to answer a question about a clause.
        ".doc" if converter_available => Some(SourceKind::Converted(SourceFormat::Doc)),
        ".odt" if converter_available => Some(SourceKind::Converted(SourceFormat::Odt)),
        ".rtf" if converter_available => Some(SourceKind::Converted(SourceFormat::Rtf)),
        // Page images. Their text is a reading, never a quotation, which the
        // retrieval side enforces by type; here they are simply a different
        // road into the index.
        ".bmp" | ".gif" | ".heic" | ".heif" | ".jpeg" | ".jpg" | ".png" | ".tif" | ".tiff"
            if transcriber_available =>
        {
            Some(SourceKind::Scanned)
        }
        _ => None,
    }
}

fn source_title(name: &OsStr) -> Option<String> {
    let path = Path::new(name);
    let title = path.file_stem()?.to_string_lossy().trim().to_string();
    (!title.is_empty()).then_some(title)
}

fn semantic_provision_text(title: &str, heading: Option<&str>, provision: &str) -> Option<String> {
    let text = match heading {
        Some(heading) => format!("Title: {title}\nHeading: {heading}\nText: {provision}"),
        None => format!("Title: {title}\nText: {provision}"),
    };
    (text.chars().count() <= MAX_SEMANTIC_INPUT_CHARS).then_some(text)
}

fn read_bounded(file: &mut File, maximum: usize) -> Result<Vec<u8>, std::io::Error> {
    let mut bytes = Vec::new();
    file.take((maximum as u64).saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "source exceeded the byte budget",
        ));
    }
    Ok(bytes)
}

fn relative_source_identity_matches(
    root: &Dir,
    relative_path: &Path,
    expected_file: FileIdentity,
) -> bool {
    let components = relative_path.components().collect::<Vec<_>>();
    if components.is_empty()
        || components
            .iter()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return false;
    }
    let Ok(mut directory) = root.try_clone() else {
        return false;
    };
    for (index, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            return false;
        };
        let Ok(metadata) = directory.symlink_metadata(name) else {
            return false;
        };
        if cap_metadata_is_link_or_reparse(&metadata) {
            return false;
        }
        let is_last = index + 1 == components.len();
        if is_last {
            return metadata.is_file() && cap_identity_matches(&metadata, expected_file);
        }
        if !metadata.is_dir() {
            return false;
        }
        let before = cap_metadata_identity_portable(&metadata);
        let Ok(child) = directory.open_dir(name) else {
            return false;
        };
        let Ok(after_metadata) = child.dir_metadata() else {
            return false;
        };
        if cap_metadata_is_link_or_reparse(&after_metadata)
            || before
                .zip(cap_metadata_identity_portable(&after_metadata))
                .is_some_and(|(before, after)| before != after)
        {
            return false;
        }
        directory = child;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approve_roots;
    use crate::retrieval::MatchScope;
    use std::fs;
    use tempfile::TempDir;

    /// A semantic worker that answers a few times and then stops for good.
    ///
    /// This is the shape of the failure that reached the owner: the engine
    /// bound, the sandbox verified, embeddings came back for a while, and then
    /// the worker went away partway through a sixteen-thousand-document build.
    struct FailingEmbedder {
        answers_before_failing: usize,
        calls: usize,
        failure: SemanticError,
    }

    impl SemanticEmbedder for FailingEmbedder {
        fn embed(&mut self, _text: &str) -> Result<Vec<f32>, SemanticError> {
            self.calls += 1;
            if self.calls > self.answers_before_failing {
                return Err(self.failure.clone());
            }
            // Unit-length and the right shape; the values do not matter here.
            let mut vector = vec![0.0f32; 512];
            vector[0] = 1.0;
            Ok(vector)
        }
    }

    fn build(temp: &TempDir) -> (AuthorizedTextVault, PathBuf) {
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        let source = root.join("Confidential Precedent.txt");
        fs::write(
            &source,
            "7. CONFIDENTIALITY\nConfidential Information includes affiliate data. Disclosure is allowed when required by law. These duties survive termination.",
        )
        .expect("source");
        fs::write(root.join("unsupported.pdf"), b"%PDF-synthetic").expect("pdf");
        let approved = approve_roots(&[root]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("authorized-text").expect("vault"),
            &approved,
            TextVaultLimits::default(),
            &AtomicBool::new(false),
        )
        .expect("build");
        (vault, source)
    }

    #[test]
    fn a_mac_without_the_on_device_model_still_gets_a_searchable_index() {
        // An independent reviewer found that a failure to open the semantic
        // session aborted the WHOLE build, so a Mac lacking Apple's linguistic
        // asset got no index at all -- not even lexical. Exact evidence is the
        // product; semantic suggestions are an optional aid the interface
        // already labels review-not-verified, and already has copy for being
        // unavailable. Withhold the capability, not the product.
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        fs::write(
            root.join("precedent.txt"),
            "7. CONFIDENTIALITY\nRecipient shall protect Confidential Information and its affiliates.",
        )
        .expect("source");
        let approved = approve_roots(&[root]).expect("approve");

        // No engine at all: the shape a Mac without the asset now produces.
        let vault = build_authorized_text_vault(
            VaultId::parse("no-semantic").expect("vault"),
            &approved,
            TextVaultLimits::default(),
            &AtomicBool::new(false),
        )
        .expect("a vault must still build without the on-device model");

        let report = vault.build_report();
        assert_eq!(report.indexed_documents, 1);
        assert!(
            !report.semantic_retrieval_enabled,
            "semantic must report itself unavailable rather than being claimed"
        );

        // ...and exact evidence, the primary feature, still works.
        let response = vault
            .interpret_and_search("Find confidentiality provisions covering affiliates.")
            .expect("exact search must work without the model");
        assert!(
            !response.evidence.is_empty(),
            "no exact evidence returned without the on-device model"
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_hard_linked_document_is_never_indexed_or_returned_as_evidence() {
        // The confirmed escape: an independent reviewer built a root whose only
        // entry was a hard link to a file outside it, and the vault reported
        // indexed_documents=1 and returned an evidence card for privileged text
        // the operator never approved, titled as though it were local.
        let outside = TempDir::new().expect("outside");
        let outside_file = outside.path().join("never-approved.txt");
        fs::write(
            &outside_file,
            "7. CONFIDENTIALITY\nPrivileged text from a directory the operator never approved.",
        )
        .expect("outside source");

        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        fs::write(
            root.join("genuine.txt"),
            "7. CONFIDENTIALITY\nConfidential Information includes affiliate data.",
        )
        .expect("in-root source");
        fs::hard_link(&outside_file, root.join("looks-local.txt")).expect("hard link");

        let approved = approve_roots(&[root]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("hard-link-probe").expect("vault"),
            &approved,
            TextVaultLimits::default(),
            &AtomicBool::new(false),
        )
        .expect("build");

        let report = vault.build_report();
        assert_eq!(
            report.hard_links_skipped, 1,
            "the hard link was not refused"
        );
        assert_eq!(
            report.indexed_documents, 1,
            "an out-of-root inode was indexed as an approved document"
        );
    }

    #[test]
    fn build_report_is_aggregate_and_derivatives_are_not_persisted() {
        let temp = TempDir::new().expect("temp");
        let (vault, _) = build(&temp);
        let report = vault.build_report();
        assert_eq!(report.indexed_documents, 1);
        assert_eq!(report.unsupported_files_skipped, 1);
        assert!(!report.source_content_persisted);
        assert!(!report.retrieval_index_persisted);
        let serialized = serde_json::to_string(report).expect("serialize");
        assert!(!serialized.contains("Confidential Precedent"));
        assert!(!serialized.contains(temp.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn authorized_text_search_returns_current_exact_evidence() {
        let temp = TempDir::new().expect("temp");
        let (vault, _) = build(&temp);
        let response = vault
            .interpret_and_search(
                "Find confidentiality provisions within three sentences covering affiliates, compelled disclosure, and survival.",
            )
            .expect("search");
        assert_eq!(response.evidence.len(), 1);
        assert_eq!(
            response.evidence[0].document_title,
            "Confidential Precedent"
        );
        assert_eq!(response.evidence[0].source_anchor, "section:0001");
        assert!(response.evidence[0]
            .exact_excerpt
            .starts_with("Confidential Information"));
    }

    #[test]
    fn source_mutation_withdraws_previously_indexed_evidence() {
        let temp = TempDir::new().expect("temp");
        let (vault, source) = build(&temp);
        fs::write(&source, "7. PUBLICITY\nPress releases require approval.").expect("mutate");
        let response = vault
            .interpret_and_search(
                "Find confidentiality provisions within three sentences covering affiliates, compelled disclosure, and survival.",
            )
            .expect("search");
        assert!(response.evidence.is_empty());
        assert_eq!(response.stale_evidence_withdrawn, 1);
    }

    #[test]
    fn source_path_replacement_is_not_reauthorized_by_name() {
        let temp = TempDir::new().expect("temp");
        let (vault, source) = build(&temp);
        let moved = source.with_extension("moved");
        fs::rename(&source, &moved).expect("move original");
        fs::write(
            &source,
            "7. CONFIDENTIALITY\nConfidential Information includes affiliate data. Disclosure is allowed when required by law. These duties survive termination.",
        )
        .expect("replacement");
        let response = vault
            .interpret_and_search(
                "Find confidentiality provisions within three sentences covering affiliates, compelled disclosure, and survival.",
            )
            .expect("search");
        assert!(response.evidence.is_empty());
        assert_eq!(response.stale_evidence_withdrawn, 1);
    }

    // Indexing refuses multiply linked files, but a file singly linked at build
    // time can be hard-linked from outside the approved root afterwards. An
    // independent reviewer confirmed the evidence card kept being returned and
    // nothing was withdrawn, so the root-authority rule held only at build time.
    #[cfg(unix)]
    #[test]
    fn hard_link_created_outside_the_root_after_indexing_withdraws_evidence() {
        let temp = TempDir::new().expect("temp");
        let (vault, source) = build(&temp);
        let same_provision = "Find confidentiality provisions within three sentences covering affiliates, compelled disclosure, and survival.";
        let anywhere = "Find confidentiality provisions covering affiliates, compelled disclosure, and survival anywhere in the document.";
        // The two scopes are separate retrieval code paths, so both are
        // exercised: a fence binding only one leaves the other quoting the
        // file. Asserted rather than assumed, so a change to query
        // interpretation cannot quietly collapse this to a single lane.
        assert_eq!(
            interpret_legal_query(same_provision).expect("query").scope,
            MatchScope::SameProvision
        );
        assert_eq!(
            interpret_legal_query(anywhere).expect("query").scope,
            MatchScope::AnywhereInDocument
        );
        // The scopes return through different fields -- provision scope fills
        // `evidence`, document scope fills `documents` -- so counting only one
        // silently skips the other lane.
        let returned =
            |response: &LegalSearchResponse| response.evidence.len() + response.documents.len();
        for raw in [same_provision, anywhere] {
            assert_eq!(
                returned(&vault.interpret_and_search(raw).expect("search")),
                1,
                "the singly linked source must be evidence before the link is created"
            );
        }

        let outside = temp.path().join("outside-the-root.txt");
        fs::hard_link(&source, &outside).expect("hard link");

        for raw in [same_provision, anywhere] {
            let response = vault.interpret_and_search(raw).expect("search");
            assert_eq!(returned(&response), 0, "{raw} still quoted the file");
            assert_eq!(response.stale_evidence_withdrawn, 1, "{raw}");
        }

        // Withdrawal must track the current link count rather than latching.
        // A latched refusal would blank a document permanently the first time
        // anything on the Mac transiently linked it, which is a worse failure
        // for counsel than the hole this fence closes.
        fs::remove_file(&outside).expect("remove the outside link");
        for raw in [same_provision, anywhere] {
            assert_eq!(
                returned(&vault.interpret_and_search(raw).expect("search")),
                1,
                "the singly linked document did not recover once the link was gone"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symbolic_links_are_skipped_and_never_become_sources() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("temp");
        let outside = temp.path().join("outside.txt");
        fs::write(
            &outside,
            "CONFIDENTIALITY\nConfidential Information survives termination.",
        )
        .expect("outside");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        symlink(&outside, root.join("linked.txt")).expect("link");
        let approved = approve_roots(&[root]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("link-test").expect("vault"),
            &approved,
            TextVaultLimits::default(),
            &AtomicBool::new(false),
        )
        .expect("build");
        assert_eq!(vault.build_report().indexed_documents, 0);
        assert_eq!(vault.build_report().symlinks_skipped, 1);
    }

    /// A budget stops the build; cancellation discards it. Those are different
    /// answers to different questions.
    ///
    /// Hitting a limit used to return an error, so a folder one document over
    /// produced nothing at all -- after however long it had already spent
    /// reading. An index of what fits, labelled as partial, is worth having.
    /// Cancellation still discards, because there the operator asked to stop
    /// and half an index they did not ask for is not a favour.
    /// A semantic worker that dies partway through must cost suggestions, not
    /// the index.
    ///
    /// This is the failure the owner hit twice. `BoundedSemanticEngine::bind`
    /// succeeding was already treated as optional, so a Mac with no linguistic
    /// asset still got exact search -- but the worker failing *during* a build
    /// returned `Err`, and the whole index was discarded. On the real archive
    /// that was 16,621 documents and twenty-two minutes of reading thrown away
    /// for an optional aid, reported as "the bounded on-device semantic worker
    /// is unavailable".
    ///
    /// Nothing in the suite could reach it: every test had either a healthy
    /// worker or no worker at all, and the interesting case is the one in
    /// between. That is what the injected embedder exists for.
    #[test]
    fn a_semantic_worker_that_dies_midway_costs_suggestions_not_the_index() {
        // Every point at which the worker can die, not just a convenient
        // one. The first version of this test used documents with an even
        // number of provisions and only ever failed *between* documents, so it
        // passed while a build that lost its worker in the middle of a
        // three-provision document still died -- with `IndexUnavailable`,
        // because the embeddings vector came up short. `answers` sweeps the
        // boundary instead of assuming one.
        for (failure, answers) in [
            SemanticError::WorkerFailed,
            SemanticError::PlatformUnavailable,
            SemanticError::ModelUnavailable,
            SemanticError::SecurityBoundaryUnavailable,
            SemanticError::WorkerBudgetExceeded,
        ]
        .into_iter()
        .flat_map(|failure| (0..7).map(move |answers| (failure.clone(), answers)))
        {
            let temp = TempDir::new().expect("temp");
            let root = temp.path().join("approved");
            fs::create_dir(&root).expect("root");
            for index in 0..12 {
                // Three provisions, deliberately an odd number: the worker
                // dies partway through a document, not neatly between two.
                fs::write(
                    root.join(format!("matter-{index:02}.txt")),
                    "ASSIGNMENT\nNeither party may assign this Agreement.\n\
                     CONFIDENTIALITY\nEach party shall protect Confidential Information.\n\
                     GOVERNING LAW\nThis Agreement is governed by the laws of New York.",
                )
                .expect("document");
            }
            let approved = approve_roots(&[root]).expect("approve");

            // Answers for the first document, then never again.
            let mut embedder = FailingEmbedder {
                answers_before_failing: answers,
                calls: 0,
                failure: failure.clone(),
            };
            let vault = build_authorized_vault(
                VaultId::parse("half-semantic").expect("vault"),
                &approved,
                TextVaultLimits::default(),
                &AtomicBool::new(false),
                None,
                None,
                None,
                None,
                Some(&mut embedder),
            )
            .unwrap_or_else(|error| {
                panic!(
                    "a semantic worker that answered {answers} times then failed with \
                     {failure:?} discarded the whole index: {error:?}"
                )
            });

            let report = vault.build_report();
            assert_eq!(
                report.indexed_documents, 12,
                "{failure:?} after {answers} answers cost documents that have nothing to \
                 do with semantics"
            );
            // Whatever the worker did answer before it died must survive. The
            // fallback that indexes a document without suggestions is the
            // safety net, not the intended path: if the vector is left short
            // of the provision count the index rejects the whole document's
            // suggestions, and every embedding already paid for is discarded.
            if answers > 0 {
                assert!(
                    report.semantic_provisions_indexed > 0,
                    "{failure:?} after {answers} answers threw away embeddings the worker had \
                     already produced"
                );
            }
            assert!(
                report.semantic_coverage_partial,
                "{failure:?} after {answers} answers degraded silently; partial suggestion \
                 coverage must be reported"
            );

            // The product is exact evidence, and it must be untouched.
            let response = vault
                .interpret_and_search(
                    "Find assignment provisions within three sentences covering assign.",
                )
                .expect("exact search still works without the semantic worker");
            assert!(
                !response.evidence.is_empty(),
                "{failure:?} after {answers} answers left exact search unable to answer"
            );
        }
    }

    /// An excluded folder is not entered, and every namesake is kept.
    ///
    /// The only unit of choice was a whole approved root, so a notes vault
    /// with thousands of screenshots under one subfolder had to be taken whole
    /// or not at all.
    ///
    /// Two namesakes are checked, because they failed differently. A nested
    /// `matters/attachments` was always safe: the comparison is against the
    /// path from the root, not the folder name. `attachments` at the top of a
    /// *second* approved root was not -- the exclusion carried no root, so the
    /// walk compared it against whichever root it was in, and the second
    /// folder disappeared with nothing but a count to say so.
    #[test]
    fn an_excluded_folder_is_not_entered_and_its_namesakes_are_kept() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        let other = temp.path().join("other-approved");
        fs::create_dir_all(root.join("attachments")).expect("attachments");
        fs::create_dir_all(root.join("matters/attachments")).expect("nested namesake");
        fs::create_dir_all(other.join("attachments")).expect("second-root namesake");
        fs::write(root.join("keep.txt"), "ASSIGNMENT\nNo assignment.").expect("keep");
        fs::write(
            root.join("attachments/skip.txt"),
            "ASSIGNMENT\nNo assignment.",
        )
        .expect("skip");
        fs::write(
            root.join("matters/attachments/keep-too.txt"),
            "ASSIGNMENT\nNo assignment.",
        )
        .expect("namesake");
        fs::write(
            other.join("attachments/keep-as-well.txt"),
            "ASSIGNMENT\nNo assignment.",
        )
        .expect("second-root namesake file");

        let approved = approve_roots(&[root, other]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("excluding").expect("vault"),
            &approved,
            TextVaultLimits {
                excluded_paths: vec![ExcludedFolder {
                    root_index: 0,
                    relative_path: PathBuf::from("attachments"),
                }],
                ..TextVaultLimits::default()
            },
            &AtomicBool::new(false),
        )
        .expect("build");

        let report = vault.build_report();
        assert_eq!(
            report.excluded_directories, 1,
            "exactly one folder should have been excluded"
        );
        assert_eq!(
            report.indexed_documents, 3,
            "the excluded folder was entered, or a namesake was wrongly skipped"
        );
    }

    #[test]
    fn a_budget_yields_a_partial_index_while_cancellation_discards() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        fs::write(root.join("one.txt"), "ASSIGNMENT\nNo assignment.").expect("one");
        fs::write(root.join("two.txt"), "ASSIGNMENT\nNo assignment.").expect("two");
        let approved = approve_roots(&[root]).expect("approve");
        let bounded = build_authorized_text_vault(
            VaultId::parse("bounded").expect("vault"),
            &approved,
            TextVaultLimits {
                max_documents: 1,
                ..TextVaultLimits::default()
            },
            &AtomicBool::new(false),
        )
        .expect("a budget must not throw away what was already read");
        let report = bounded.build_report();
        assert_eq!(report.indexed_documents, 1);
        assert!(
            report.budget_reached,
            "a partial index must say that it is partial"
        );
        assert_eq!(report.documents_left_unread, 1);
        // And it is a working index, not a husk.
        assert!(!bounded
            .interpret_and_search("Find documents containing assignment.")
            .expect("search")
            .documents
            .is_empty());
        assert!(matches!(
            build_authorized_text_vault(
                VaultId::parse("cancelled").expect("vault"),
                &approved,
                TextVaultLimits::default(),
                &AtomicBool::new(true),
            ),
            Err(VaultError::Cancelled)
        ));
    }

    #[test]
    fn root_replacement_fails_the_query_instead_of_using_retained_authority() {
        let temp = TempDir::new().expect("temp");
        let (vault, source) = build(&temp);
        let root = source.parent().expect("root").to_path_buf();
        let moved_root = temp.path().join("moved-root");
        fs::rename(&root, &moved_root).expect("move root");
        fs::create_dir(&root).expect("replacement root");
        assert_eq!(
            vault.interpret_and_search("Find confidentiality provisions."),
            Err(VaultError::RootChanged)
        );
    }

    /// A folder bigger than one extraction batch counts every document once.
    ///
    /// Extraction now happens away from the walk, in batches, so the counters
    /// move somewhere other than where the file was read. Every other fixture
    /// here is smaller than a batch and so never crosses that seam. This one
    /// does, and it mixes in sources that cannot be normalized so a failure
    /// lands on the right counter after the reordering too.
    #[test]
    fn a_folder_larger_than_one_extraction_batch_counts_every_document_once() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        for index in 0..64u32 {
            fs::write(
                root.join(format!("precedent-{index:03}.txt")),
                "7. CONFIDENTIALITY\nConfidential Information includes affiliate data.",
            )
            .expect("source");
        }
        for index in 0..3u32 {
            // Read without complaint, then refused by normalization: not UTF-8.
            fs::write(
                root.join(format!("mojibake-{index}.txt")),
                [0xff, 0xfe, 0x9f, 0x0a],
            )
            .expect("malformed source");
        }

        let approved = approve_roots(&[root]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("batched").expect("vault"),
            &approved,
            TextVaultLimits::default(),
            &AtomicBool::new(false),
        )
        .expect("build");

        let report = vault.build_report();
        assert_eq!(report.indexed_documents, 64);
        assert_eq!(report.malformed_text_files_skipped, 3);
        assert!(!report.budget_reached);
        assert_eq!(report.documents_left_unread, 0);
        assert!(!vault
            .interpret_and_search("Find documents containing confidentiality.")
            .expect("search")
            .documents
            .is_empty());
    }

    /// A limit smaller than one batch still stops on the document it names.
    ///
    /// Documents wait in a batch before they reach the counters a budget is
    /// measured against, so the build drains the batch before deciding. Without
    /// that, a limit of five would let a whole batch of twelve through.
    #[test]
    fn a_budget_smaller_than_one_extraction_batch_stops_at_the_limit() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        for index in 0..40u32 {
            fs::write(
                root.join(format!("precedent-{index:03}.txt")),
                "7. CONFIDENTIALITY\nConfidential Information includes affiliate data.",
            )
            .expect("source");
        }

        let approved = approve_roots(&[root]).expect("approve");
        let vault = build_authorized_text_vault(
            VaultId::parse("batched-budget").expect("vault"),
            &approved,
            TextVaultLimits {
                max_documents: 5,
                ..TextVaultLimits::default()
            },
            &AtomicBool::new(false),
        )
        .expect("a budget must not throw away what was already read");

        let report = vault.build_report();
        assert_eq!(report.indexed_documents, 5);
        assert!(report.budget_reached);
        assert_eq!(report.documents_left_unread, 35);
    }
}
