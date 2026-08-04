//! Operator QA against a real synthetic fixture folder.
//!
//! `document_vault_smoke` builds its own fixtures inline, so it proves the
//! pipeline works but not that it behaves correctly on the folder an operator
//! actually reviews. This drives the same code paths against the tree produced
//! by `scripts/make-archive-qa-fixtures.sh`, and asserts the properties the
//! pilot runbook makes preconditions of delivery -- census export privacy,
//! exact evidence with anchors, and withdrawal of a mutated source.
//!
//! It does not replace the human GUI pass: the native folder picker,
//! cancellation, and the visible close-to-purge behaviour still need a person.
//! Everything below that surface is checked here.
//!
//! Usage:
//!   cargo run -p minutes-archive-core --example archive_qa_fixtures_smoke -- \
//!     "<minutes-archive-app executable>" "<fixture directory>"

use minutes_archive_convert::BoundedConverter;
use minutes_archive_core::retrieval::VaultId;
use minutes_archive_core::vault::{build_authorized_document_vault, DocumentVaultLimits};
use minutes_archive_core::{approve_roots, scan_approved_roots, CensusLimits};
use minutes_archive_semantic::BoundedSemanticEngine;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Canary written into every fixture by `make-archive-qa-fixtures.sh`.
const CANARY: &str = "ARCHIVE_QA_CANARY";

fn main() {
    let mut args = std::env::args_os().skip(1);
    let worker_path: PathBuf = args
        .next()
        .expect("usage: archive_qa_fixtures_smoke <executable> <fixture dir>")
        .into();
    let fixture_root: PathBuf = args
        .next()
        .expect("usage: archive_qa_fixtures_smoke <executable> <fixture dir>")
        .into();

    let approved =
        approve_roots(std::slice::from_ref(&fixture_root)).expect("approve fixture root");

    // --- Census, and the privacy properties of its exported form -----------
    let census = scan_approved_roots(&approved, CensusLimits::default(), &AtomicBool::new(false))
        .expect("census");
    let exported = serde_json::to_string(&census).expect("serialize census");

    assert!(
        !exported.contains(CANARY),
        "census export leaked document content: {exported}"
    );
    for entry in fs::read_dir(&fixture_root).expect("read fixture dir") {
        let entry = entry.expect("fixture entry");
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // The extension is legitimately reported; the rest of the name is not.
        let stem = name
            .rsplit_once('.')
            .map_or(name.as_ref(), |(stem, _)| stem);
        assert!(
            !exported.contains(stem),
            "census export leaked the filename {name:?}: {exported}"
        );
    }
    let root_component = fixture_root
        .file_name()
        .expect("fixture dir name")
        .to_string_lossy()
        .to_string();
    assert!(
        !exported.contains(&root_component),
        "census export leaked an approved path component: {exported}"
    );
    println!("archive_qa_census=exported_without_names_paths_or_content");
    println!(
        "archive_qa_census_artifacts={} formats={}",
        census.summary.artifacts,
        census.formats.len()
    );

    // --- Index and retrieve ------------------------------------------------
    let converter = BoundedConverter::bind(&worker_path).expect("bind converter");
    let semantic_engine = BoundedSemanticEngine::bind(&worker_path).expect("bind semantic worker");
    let vault = build_authorized_document_vault(
        VaultId::parse("archive-qa-fixtures").expect("vault id"),
        &approved,
        DocumentVaultLimits::default(),
        &AtomicBool::new(false),
        &converter,
        semantic_engine,
    )
    .expect("build vault");

    let report = vault.build_report();
    assert!(
        !report.source_content_persisted,
        "source content must never be persisted"
    );
    assert!(
        !report.retrieval_index_persisted,
        "the retrieval index must never be persisted"
    );
    assert!(
        !report.semantic_derivatives_persisted,
        "semantic vectors must never be persisted"
    );
    assert!(
        !report.semantic_model_download_requested,
        "no model may be downloaded"
    );
    println!(
        "archive_qa_index=built documents={} conversion_failures={}",
        report.indexed_documents, report.conversion_failures
    );

    // The question the runbook tells Peter to start with.
    let response = vault
        .interpret_and_search(
            "Find confidentiality provisions no more than three sentences covering affiliates, compelled disclosure, and survival.",
        )
        .expect("search");
    assert!(
        !response.evidence.is_empty(),
        "the known-good fixture must be found"
    );
    for card in &response.evidence {
        assert!(
            !card.source_anchor.is_empty(),
            "every result needs a source anchor counsel can verify"
        );
        assert!(
            !card.exact_excerpt.is_empty(),
            "every result needs citable text"
        );
        assert!(card.index_fresh, "stale evidence must not be displayed");
    }
    println!(
        "archive_qa_search=passed evidence={} suggestions={}",
        response.evidence.len(),
        response.semantic_suggestions.len()
    );

    // --- Withdrawal of a mutated source ------------------------------------
    let mutated = response
        .evidence
        .first()
        .map(|card| card.document_title.clone())
        .expect("at least one evidence card");
    let target = find_by_title(&fixture_root, &mutated).expect("locate the indexed source on disk");
    let original = fs::read(&target).expect("read source");
    fs::write(&target, b"replaced after indexing\n").expect("mutate source");

    let after = vault
        .interpret_and_search(
            "Find confidentiality provisions no more than three sentences covering affiliates, compelled disclosure, and survival.",
        )
        .expect("search after mutation");
    assert!(
        after
            .evidence
            .iter()
            .all(|card| card.document_title != mutated),
        "evidence from a mutated source must be withdrawn"
    );
    fs::write(&target, original).expect("restore source");
    println!("archive_qa_withdrawal=passed mutated_source_withdrawn");

    println!("archive_qa_fixtures_smoke=passed");
}

fn find_by_title(root: &Path, title: &str) -> Option<PathBuf> {
    for entry in fs::read_dir(root).ok()? {
        let path = entry.ok()?.path();
        let name = path.file_name()?.to_string_lossy().to_string();
        if name == title || name.starts_with(title) || title.starts_with(&name) {
            return Some(path);
        }
    }
    None
}
