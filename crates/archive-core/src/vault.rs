//! Capability-bound ingestion and live-source verification for a local legal
//! text vault.
//!
//! This module is intentionally a narrow Gate 2 proof. It supports bounded
//! UTF-8 text and Markdown sources, keeps its FTS index in memory, retains
//! read-only file capabilities, and revalidates source membership, identity,
//! bytes, and revision before evidence leaves the vault.

use crate::retrieval::{
    interpret_legal_query, normalize_converted_document, normalize_text_document,
    CurrentRevisionSet, DocumentId, LegalIndex, LegalQuery, LegalSearchResponse,
    ProvisionBoundaries, RetrievalError, SourceRevision, VaultId, MAX_NORMALIZED_DOCUMENT_BYTES,
    MAX_QUERY_CHARS, MAX_SEMANTIC_PROVISIONS,
};
use crate::{
    cap_identity_matches, cap_metadata_identity_portable, cap_metadata_is_link_or_reparse,
    cap_metadata_is_multiply_linked, extension_for_name, open_approved_root, package_category,
    validate_approved_roots, ApprovedRoot, CensusError, FileIdentity,
};
use cap_std::fs::{Dir, File};
use minutes_archive_convert::{BoundedConverter, SourceFormat, WorkerError};
use minutes_archive_semantic::{
    BoundedSemanticEngine, SemanticError, SemanticModelMetadata, MAX_SEMANTIC_INPUT_CHARS,
};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use thiserror::Error;

pub const DOCUMENT_VAULT_SCHEMA: &str = "minutes.archive-document-vault.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextVaultLimits {
    pub max_documents: usize,
    pub max_total_bytes: u64,
    pub max_directories: u64,
    pub max_depth: u32,
}

impl Default for TextVaultLimits {
    fn default() -> Self {
        Self {
            max_documents: 50_000,
            max_total_bytes: 2 * 1024 * 1024 * 1024,
            max_directories: 100_000,
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
        if !cap_identity_matches(&before, source.identity)
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
        cap_identity_matches(&after, source.identity)
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

#[derive(Debug, Default)]
struct BuildCounters {
    indexed_documents: u64,
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
}

pub fn build_authorized_text_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
) -> Result<AuthorizedTextVault, VaultError> {
    build_authorized_vault(vault_id, approved_roots, limits, cancelled, None, None)
}

pub fn build_authorized_document_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
    converter: &BoundedConverter,
    semantic_engine: BoundedSemanticEngine,
) -> Result<AuthorizedTextVault, VaultError> {
    build_authorized_vault(
        vault_id,
        approved_roots,
        limits,
        cancelled,
        Some(converter),
        Some(semantic_engine),
    )
}

fn build_authorized_vault(
    vault_id: VaultId,
    approved_roots: &[ApprovedRoot],
    limits: TextVaultLimits,
    cancelled: &AtomicBool,
    converter: Option<&BoundedConverter>,
    semantic_engine: Option<BoundedSemanticEngine>,
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
    let mut semantic_session = semantic_engine
        .as_ref()
        .map(BoundedSemanticEngine::open_session)
        .transpose()
        .map_err(|_| VaultError::SemanticUnavailable)?;
    let semantic_model = semantic_session
        .as_ref()
        .map(|_| SemanticModelMetadata::apple_english_sentence_revision_one());

    while let Some(current) = pending.pop() {
        if cancelled.load(Ordering::Acquire) {
            return Err(VaultError::Cancelled);
        }
        if counters.directories_scanned >= limits.max_directories {
            return Err(VaultError::BuildBudgetExceeded);
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
                    counters.metadata_errors = counters.metadata_errors.saturating_add(1);
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
                if current.depth >= limits.max_depth {
                    return Err(VaultError::BuildBudgetExceeded);
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
                        counters.metadata_errors = counters.metadata_errors.saturating_add(1);
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
            let Some(source_kind) = source_kind(&name, converter.is_some()) else {
                counters.unsupported_files_skipped =
                    counters.unsupported_files_skipped.saturating_add(1);
                continue;
            };
            if metadata.len() == 0 || metadata.len() > MAX_NORMALIZED_DOCUMENT_BYTES as u64 {
                counters.oversized_files_skipped =
                    counters.oversized_files_skipped.saturating_add(1);
                continue;
            }
            if counters.indexed_documents as usize >= limits.max_documents
                || counters.indexed_bytes.saturating_add(metadata.len()) > limits.max_total_bytes
            {
                return Err(VaultError::BuildBudgetExceeded);
            }
            let Some(identity) = cap_metadata_identity_portable(&metadata) else {
                counters.metadata_errors = counters.metadata_errors.saturating_add(1);
                continue;
            };
            if !identities.insert(identity) {
                counters.duplicate_files_skipped =
                    counters.duplicate_files_skipped.saturating_add(1);
                continue;
            }

            let mut file = match current.directory.open(&name) {
                Ok(file) => file,
                Err(_) => {
                    counters.metadata_errors = counters.metadata_errors.saturating_add(1);
                    continue;
                }
            };
            let opened_metadata = match file.metadata() {
                Ok(metadata) => metadata,
                Err(_) => {
                    counters.metadata_errors = counters.metadata_errors.saturating_add(1);
                    continue;
                }
            };
            if !cap_identity_matches(&opened_metadata, identity) {
                counters.metadata_errors = counters.metadata_errors.saturating_add(1);
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
                    counters.metadata_errors = counters.metadata_errors.saturating_add(1);
                    continue;
                }
            };
            if !cap_identity_matches(&post_read_metadata, identity)
                || post_read_metadata.len() != opened_metadata.len()
            {
                counters.metadata_errors = counters.metadata_errors.saturating_add(1);
                continue;
            }

            let document_number = counters.indexed_documents.saturating_add(1);
            let document_id = DocumentId::parse(format!("document-{document_number:016x}"))
                .map_err(VaultError::from)?;
            let Some(title) = source_title(&name) else {
                counters.malformed_text_files_skipped =
                    counters.malformed_text_files_skipped.saturating_add(1);
                continue;
            };
            let normalized = match source_kind {
                SourceKind::Text => normalize_text_document(document_id.clone(), title, &bytes),
                SourceKind::Converted(format) => {
                    let Some(converter) = converter else {
                        return Err(VaultError::ConverterUnavailable);
                    };
                    let converted = match converter.convert(format, &bytes) {
                        Ok(converted) => converted,
                        Err(WorkerError::SourceRefused | WorkerError::WorkerBudgetExceeded) => {
                            counters.conversion_failures =
                                counters.conversion_failures.saturating_add(1);
                            continue;
                        }
                        Err(_) => return Err(VaultError::ConverterUnavailable),
                    };
                    if converted.blocks.is_empty()
                        && converted
                            .warnings
                            .iter()
                            .any(|warning| warning == "ocr_required_or_no_extractable_text")
                    {
                        counters.ocr_required_files = counters.ocr_required_files.saturating_add(1);
                        continue;
                    }
                    normalize_converted_document(document_id.clone(), title, &bytes, &converted)
                }
            };
            let normalized = match normalized {
                Ok(document) => document,
                Err(_) => {
                    match source_kind {
                        SourceKind::Text => {
                            counters.malformed_text_files_skipped =
                                counters.malformed_text_files_skipped.saturating_add(1);
                        }
                        SourceKind::Converted(_) => {
                            counters.conversion_failures =
                                counters.conversion_failures.saturating_add(1);
                        }
                    }
                    continue;
                }
            };
            if let Some(session) = semantic_session.as_mut() {
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
                        Some(Err(
                            SemanticError::PlatformUnavailable | SemanticError::ModelUnavailable,
                        )) => {
                            return Err(VaultError::SemanticUnavailable);
                        }
                        Some(Err(
                            SemanticError::ExecutableUnavailable
                            | SemanticError::SecurityBoundaryUnavailable
                            | SemanticError::WorkerBudgetExceeded
                            | SemanticError::WorkerFailed,
                        )) => return Err(VaultError::SemanticUnavailable),
                    }
                }
                let indexed = index
                    .replace_document_with_semantics(
                        &normalized,
                        semantic_model
                            .clone()
                            .ok_or(VaultError::SemanticUnavailable)?,
                        &embeddings,
                    )
                    .map_err(VaultError::from)?;
                counters.semantic_provisions_indexed = counters
                    .semantic_provisions_indexed
                    .saturating_add(indexed as u64);
            } else {
                index
                    .replace_document(&normalized)
                    .map_err(VaultError::from)?;
            }
            sources.insert(
                document_id,
                AuthorizedSource {
                    root_index: current.root_index,
                    relative_path: current.relative_path.join(&name),
                    identity,
                    file,
                    indexed_revision: normalized.revision,
                },
            );
            counters.indexed_documents = document_number;
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
                SourceKind::Text => {}
            }
            counters.indexed_bytes = counters
                .indexed_bytes
                .saturating_add(post_read_metadata.len());
        }
    }

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
        semantic_derivatives_persisted: false,
        semantic_model_download_requested: false,
        supported_formats: if converter.is_some() {
            vec![".docx", ".md", ".pdf", ".text", ".txt"]
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

#[derive(Debug, Clone, Copy)]
enum SourceKind {
    Text,
    Converted(SourceFormat),
}

fn source_kind(name: &OsStr, converter_available: bool) -> Option<SourceKind> {
    match extension_for_name(name).as_str() {
        ".md" | ".text" | ".txt" => Some(SourceKind::Text),
        ".pdf" if converter_available => Some(SourceKind::Converted(SourceFormat::Pdf)),
        ".docx" if converter_available => Some(SourceKind::Converted(SourceFormat::Docx)),
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
    use std::fs;
    use tempfile::TempDir;

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

    #[test]
    fn build_budget_and_cancellation_discard_partial_vaults() {
        let temp = TempDir::new().expect("temp");
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("root");
        fs::write(root.join("one.txt"), "ASSIGNMENT\nNo assignment.").expect("one");
        fs::write(root.join("two.txt"), "ASSIGNMENT\nNo assignment.").expect("two");
        let approved = approve_roots(&[root]).expect("approve");
        assert!(matches!(
            build_authorized_text_vault(
                VaultId::parse("bounded").expect("vault"),
                &approved,
                TextVaultLimits {
                    max_documents: 1,
                    ..TextVaultLimits::default()
                },
                &AtomicBool::new(false),
            ),
            Err(VaultError::BuildBudgetExceeded)
        ));
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
}
