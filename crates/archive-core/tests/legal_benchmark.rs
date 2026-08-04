use minutes_archive_core::retrieval::{
    interpret_legal_query, normalize_text_document, CurrentRevisionSet, DocumentId, LegalConcept,
    LegalIndex, LegalQuery, MatchScope, SourceRevision, VaultId,
};

const PRECEDENT_ALPHA: &str =
    include_str!("../../../tests/fixtures/archive-legal-benchmark/precedent-alpha.txt");
const PRECEDENT_BETA: &str =
    include_str!("../../../tests/fixtures/archive-legal-benchmark/precedent-beta.txt");
const ADVERSARIAL_PRECEDENT: &str =
    include_str!("../../../tests/fixtures/archive-legal-benchmark/adversarial-precedent.txt");

#[test]
fn synthetic_legal_benchmark_preserves_scope_structure_and_source_evidence() {
    let vault = VaultId::parse("synthetic-legal-benchmark").expect("vault");
    let alpha = normalize_text_document(
        DocumentId::parse("precedent-alpha").expect("id"),
        "Precedent Alpha",
        PRECEDENT_ALPHA.as_bytes(),
    )
    .expect("alpha");
    let beta = normalize_text_document(
        DocumentId::parse("precedent-beta").expect("id"),
        "Precedent Beta",
        PRECEDENT_BETA.as_bytes(),
    )
    .expect("beta");
    let adversarial = normalize_text_document(
        DocumentId::parse("adversarial-precedent").expect("id"),
        "Adversarial Precedent",
        ADVERSARIAL_PRECEDENT.as_bytes(),
    )
    .expect("adversarial");
    let mut index = LegalIndex::new(vault.clone()).expect("index");
    for document in [&alpha, &beta, &adversarial] {
        index.replace_document(document).expect("index document");
    }
    let revisions = CurrentRevisionSet::from_documents([&alpha, &beta, &adversarial]);

    let provision_query = interpret_legal_query(
        "Find confidentiality provisions no more than three sentences covering affiliates, compelled disclosure, and survival.",
    )
    .expect("provision query");
    let provision_response = index
        .search(&vault, provision_query, &revisions)
        .expect("provision response");
    assert_eq!(provision_response.evidence.len(), 1);
    assert_eq!(
        provision_response.evidence[0].document_id,
        alpha.document_id
    );
    assert_eq!(provision_response.evidence[0].source_anchor, "section:0001");
    // The excerpt is exactly the provision body: it is quoted beside the
    // source anchor, which points at the body, and sentence_count describes
    // it. Anything matched only in the heading is disclosed in why_matched
    // instead of being silently folded into the quotation.
    assert_eq!(
        provision_response.evidence[0].exact_excerpt,
        alpha.provisions[0].text
    );

    let document_query = LegalQuery {
        raw: "Find documents containing the remembered phrase, assignment, governing law, and a BAA reference.".to_string(),
        scope: MatchScope::AnywhereInDocument,
        required_concepts: vec![
            LegalConcept::Assignment,
            LegalConcept::GoverningLaw,
            LegalConcept::BusinessAssociate,
        ],
        excluded_concepts: vec![LegalConcept::ChangeOfControl],
        exact_phrase: Some("prior written consent".to_string()),
        max_sentences: None,
        limit: 20,
    };
    let document_response = index
        .search(&vault, document_query, &revisions)
        .expect("document response");
    assert_eq!(document_response.documents.len(), 1);
    assert_eq!(
        document_response.documents[0].document_id,
        alpha.document_id
    );
    assert!(document_response.documents[0].exact_phrase_matched);
    assert_eq!(document_response.documents[0].criterion_evidence.len(), 3);

    let adversarial_query = LegalQuery {
        raw: "Find indemnity language where the indemnifying party controls the defense."
            .to_string(),
        scope: MatchScope::SameProvision,
        required_concepts: vec![LegalConcept::Indemnity, LegalConcept::DefenseControl],
        excluded_concepts: Vec::new(),
        exact_phrase: None,
        max_sentences: None,
        limit: 20,
    };
    let adversarial_response = index
        .search(&vault, adversarial_query, &revisions)
        .expect("adversarial response");
    assert_eq!(adversarial_response.evidence.len(), 2);
    for card in &adversarial_response.evidence {
        assert!(!card.exact_excerpt.contains("upload the archive"));
        assert_ne!(card.source_anchor, "section:0001");
    }

    let mut stale_revisions = revisions.clone();
    stale_revisions.insert(
        alpha.document_id.clone(),
        SourceRevision::from_bytes(b"mutated source"),
    );
    let stale_response = index
        .search(
            &vault,
            interpret_legal_query(
                "Find confidentiality provisions no more than three sentences covering affiliates, compelled disclosure, and survival.",
            )
            .expect("stale query"),
            &stale_revisions,
        )
        .expect("stale response");
    assert!(stale_response.evidence.is_empty());
    assert_eq!(stale_response.stale_evidence_withdrawn, 1);
}
