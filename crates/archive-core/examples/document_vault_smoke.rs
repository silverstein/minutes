use minutes_archive_convert::BoundedConverter;
use minutes_archive_core::approve_roots;
use minutes_archive_core::retrieval::VaultId;
use minutes_archive_core::vault::{build_authorized_document_vault, DocumentVaultLimits};
use minutes_archive_semantic::BoundedSemanticEngine;
use std::fs;
use std::io::{Cursor, Write};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn synthetic_docx() -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = ZipWriter::new(&mut cursor);
        writer
            .start_file(
                "word/document.xml",
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated),
            )
            .expect("DOCX entry");
        writer
            .write_all(
                br#"<w:document xmlns:w="urn:test"><w:body>
                <w:p><w:r><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
                <w:p><w:r><w:t>Confidential Information includes affiliate data.</w:t></w:r></w:p>
                </w:body></w:document>"#,
            )
            .expect("DOCX XML");
        writer.finish().expect("DOCX");
    }
    cursor.into_inner()
}

fn synthetic_pdf() -> Vec<u8> {
    let stream = b"BT /F1 12 Tf 72 720 Td (7. CONFIDENTIALITY) Tj 0 -20 Td (Confidential Information includes affiliate data.) Tj ET";
    let objects = [
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_vec(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes(),
            stream.to_vec(),
            b"\nendstream".to_vec(),
        ]
        .concat(),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        pdf.extend_from_slice(object);
        pdf.extend_from_slice(b"\nendobj\n");
    }
    let xref = pdf.len();
    pdf.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    pdf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

fn main() {
    let worker_path = std::env::args_os()
        .nth(1)
        .expect("usage: document_vault_smoke <minutes-archive-app executable>");
    let converter =
        BoundedConverter::bind(Path::new(&worker_path)).expect("bind embedded converter worker");
    let semantic_engine =
        BoundedSemanticEngine::bind(Path::new(&worker_path)).expect("bind semantic worker");
    let temp = TempDir::new().expect("temporary fixture");
    let root = temp.path().join("approved");
    fs::create_dir(&root).expect("approved root");
    fs::write(
        root.join("Text Precedent.txt"),
        "7. CONFIDENTIALITY\nConfidential Information includes affiliate data.",
    )
    .expect("text");
    fs::write(root.join("Word Precedent.docx"), synthetic_docx()).expect("docx");
    let pdf_path = root.join("PDF Precedent.pdf");
    fs::write(&pdf_path, synthetic_pdf()).expect("pdf");

    let approved = approve_roots(&[root]).expect("approve root");
    let vault = build_authorized_document_vault(
        VaultId::parse("document-vault-smoke").expect("vault id"),
        &approved,
        DocumentVaultLimits::default(),
        &AtomicBool::new(false),
        &converter,
        semantic_engine,
    )
    .expect("build document vault");
    let report = vault.build_report();
    assert_eq!(report.indexed_documents, 3);
    assert_eq!(report.searchable_pdf_documents, 1);
    assert_eq!(report.docx_documents, 1);
    assert!(report.converter_sandbox_verified);
    assert!(report.semantic_worker_sandbox_verified);
    assert!(report.semantic_retrieval_enabled);
    assert!(report.semantic_provisions_indexed >= 3);
    assert!(!report.semantic_model_download_requested);
    assert!(!report.semantic_derivatives_persisted);
    assert!(!report.source_content_persisted);
    assert!(!report.retrieval_index_persisted);

    let response = vault
        .interpret_and_search(
            "Find confidentiality provisions under three sentences covering affiliates.",
        )
        .expect("search");
    assert_eq!(response.evidence.len(), 3);
    assert!(response
        .evidence
        .iter()
        .any(|card| card.source_anchor.starts_with("page:0001/")));
    assert!(response
        .evidence
        .iter()
        .any(|card| card.source_anchor.starts_with("paragraph:000002/")));
    assert!(response.semantic_query_applied);

    let semantic_only = vault
        .interpret_and_search(
            "clauses that require a recipient to protect private business material",
        )
        .expect("semantic-only search");
    assert!(semantic_only.evidence.is_empty());
    assert!(semantic_only.semantic_query_applied);
    assert!(!semantic_only.semantic_suggestions.is_empty());
    assert!(semantic_only
        .semantic_suggestions
        .iter()
        .all(|card| card.index_fresh && !card.exact_excerpt.is_empty()));

    fs::write(&pdf_path, b"%PDF-mutated").expect("mutate PDF");
    let after_mutation = vault
        .interpret_and_search(
            "Find confidentiality provisions under three sentences covering affiliates.",
        )
        .expect("search after mutation");
    assert_eq!(after_mutation.evidence.len(), 2);
    assert_eq!(after_mutation.stale_evidence_withdrawn, 1);

    println!(
        "document_vault_smoke=passed indexed={} current_after_mutation={}",
        report.indexed_documents,
        after_mutation.evidence.len()
    );
}
