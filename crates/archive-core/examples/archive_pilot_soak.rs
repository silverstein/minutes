//! A build at archive scale, under the constraints the application really has.
//!
//! Every failure the owner of this pilot hit in person got past the existing
//! checks for the same two reasons, and this exists to close both.
//!
//! **Scale.** `document_vault_smoke` indexes three documents. A build that
//! stops at 237 of 16,621, or loses its semantic worker nine thousand
//! documents in, looks perfect at three.
//!
//! **Environment.** The tests run from a terminal, which inherits a soft
//! `RLIMIT_NOFILE` in the thousands. launchd gives a GUI application 256. The
//! vault holds one descriptor per indexed document, so the real application
//! stopped at 237 and called the rest unreadable while every local run passed.
//! This harness lowers its own ceiling to the GUI's 256 before building, which
//! is the only way a test can be in the same situation the application is in.
//!
//! Runs headless against the installed executable -- the same binary the
//! application spawns its workers from -- with no window, no folder picker and
//! nobody clicking anything.
//!
//! Usage: `archive_pilot_soak <exe> [documents] [--census-shape]`
//!
//! `--census-shape` mixes the formats in the proportions a real thirty-year
//! archive actually had -- 78% text and Markdown, 17% images and scans, 3% PDF,
//! 2% Word -- instead of text alone. Scans cost a sandboxed recognizer process
//! each, so this is the slow, pre-delivery form; the default stays text-only
//! and fast enough to run on every commit.

use minutes_archive_convert::BoundedConverter;
use minutes_archive_core::approve_roots;
use minutes_archive_core::retrieval::VaultId;
use minutes_archive_core::vault::{
    build_authorized_document_vault, raise_open_file_ceiling, DocumentVaultLimits, ExcludedFolder,
};
use minutes_archive_ocr::BoundedTranscriber;
use minutes_archive_semantic::BoundedSemanticEngine;
use std::fs;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;
use tempfile::TempDir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// The soft limit launchd hands a GUI application.
#[cfg(unix)]
const GUI_OPEN_FILE_SOFT_LIMIT: libc::rlim_t = 256;

/// Pin this process to the ceiling the application starts life with.
#[cfg(unix)]
fn adopt_gui_open_file_limit() {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
        0,
        "could not read the open-file limit"
    );
    let lowered = libc::rlimit {
        rlim_cur: GUI_OPEN_FILE_SOFT_LIMIT,
        rlim_max: limit.rlim_max,
    };
    assert_eq!(
        unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) },
        0,
        "could not lower the open-file limit to the GUI's"
    );
}

#[cfg(not(unix))]
fn adopt_gui_open_file_limit() {}

/// Peak resident memory of this process, in megabytes.
#[cfg(unix)]
fn peak_resident_megabytes() -> u64 {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return 0;
    }
    // Darwin reports maxrss in bytes; Linux in kilobytes.
    let raw = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        raw / (1024 * 1024)
    } else {
        raw / 1024
    }
}

#[cfg(not(unix))]
fn peak_resident_megabytes() -> u64 {
    0
}

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

/// A document with real provisions, so the semantic path does work per file
/// rather than being skipped for want of text.
fn synthetic_matter(index: usize) -> String {
    // A real practice does not put every clause in every document. Uniform
    // filler makes an intersection query look exactly like a single-term one,
    // which is the difference this corpus exists to show, so the clauses are
    // spread instead: roughly 55% carry confidentiality, 50% assignment, 30%
    // governing law, and one in two hundred an escrow clause that almost
    // nothing else mentions.
    let mut document = format!("MATTER {index:05}\n");
    if index % 20 < 11 {
        document.push_str(
            "\n7. CONFIDENTIALITY\n\
             Confidential Information includes affiliate data disclosed under this Agreement.\n",
        );
    }
    if index.is_multiple_of(2) {
        document.push_str(
            "\n8. ASSIGNMENT\n\
             Neither party may assign this Agreement without prior written consent.\n",
        );
    }
    if index % 10 < 3 {
        document.push_str(
            "\n9. GOVERNING LAW\n\
             This Agreement is governed by the laws of the State of New York.\n",
        );
    }
    if index.is_multiple_of(200) {
        document.push_str(
            "\n12. ESCROW\n\
             The escrow agent shall release the retained funds upon written notice.\n",
        );
    }
    // Never empty: a document with no provision is a different test.
    if document.lines().count() < 2 {
        document.push_str("\n5. NOTICES\nNotices shall be delivered to the address of record.\n");
    }
    document
}

/// The format mix measured on the real archive this pilot is for: 16,621
/// artifacts, of which 13,029 text and Markdown, 2,774 images and scans, 447
/// PDF, 349 other. Held as proportions so any total keeps the same shape.
struct CensusShape {
    scans: usize,
    pdfs: usize,
    word: usize,
    text: usize,
}

impl CensusShape {
    fn text_only(total: usize) -> Self {
        Self {
            scans: 0,
            pdfs: 0,
            word: 0,
            text: total,
        }
    }

    fn from_real_archive(total: usize) -> Self {
        let scans = total * 2_774 / 16_621;
        let pdfs = total * 447 / 16_621;
        let word = total * 349 / 16_621;
        Self {
            scans,
            pdfs,
            word,
            text: total - scans - pdfs - word,
        }
    }
}

fn main() {
    let mut arguments = std::env::args().skip(1);
    let worker_path = arguments
        .next()
        .expect("usage: archive_pilot_soak <exe> [documents] [--census-shape]");
    // Well past the 256 the application is given, so a build that cannot raise
    // its own ceiling fails here instead of on the owner's archive.
    let documents: usize = arguments
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(700);
    assert!(
        documents > 300,
        "a soak below the 256-descriptor ceiling proves nothing"
    );
    let census_shape = std::env::args().any(|value| value == "--census-shape");
    let shape = if census_shape {
        CensusShape::from_real_archive(documents)
    } else {
        CensusShape::text_only(documents)
    };

    adopt_gui_open_file_limit();
    // Exactly what the application does at startup, from the same code.
    raise_open_file_ceiling();

    let temp = TempDir::new().expect("temporary fixture");
    let root = temp.path().join("approved");
    fs::create_dir(&root).expect("approved root");

    // Nested, because a flat folder never exercises the descent, and one
    // folder that must be skipped entirely.
    let matters = root.join("matters");
    let deep = matters.join("2019").join("q3");
    let skipped = root.join("attachments");
    fs::create_dir_all(&deep).expect("nested folders");
    fs::create_dir(&skipped).expect("skipped folder");

    let place = |index: usize| -> PathBuf {
        match index % 3 {
            0 => root.clone(),
            1 => matters.clone(),
            _ => deep.clone(),
        }
    };
    for index in 0..shape.text {
        fs::write(
            place(index).join(format!("matter-{index:05}.txt")),
            synthetic_matter(index),
        )
        .expect("write matter");
    }
    if census_shape {
        // A real rendered page, copied. Each one costs a sandboxed recognizer
        // process, which is the cost this shape exists to measure.
        let scan = fs::read("tests/fixtures/archive-ocr/scanned-nda.png")
            .expect("the OCR fixture must be run from the repository root");
        for index in 0..shape.scans {
            fs::write(place(index).join(format!("scan-{index:05}.png")), &scan)
                .expect("write scan");
        }
        let pdf = synthetic_pdf();
        for index in 0..shape.pdfs {
            fs::write(place(index).join(format!("filed-{index:05}.pdf")), &pdf).expect("write pdf");
        }
        let word = synthetic_docx();
        for index in 0..shape.word {
            fs::write(place(index).join(format!("draft-{index:05}.docx")), &word)
                .expect("write docx");
        }
    }
    // These must never be read, and must not be counted as indexed.
    let skipped_documents = 40;
    for index in 0..skipped_documents {
        fs::write(
            skipped.join(format!("screenshot-{index:03}.txt")),
            synthetic_matter(900_000 + index),
        )
        .expect("write skipped");
    }

    let converter =
        BoundedConverter::bind(Path::new(&worker_path)).expect("bind embedded converter worker");
    // Optional exactly as in the application: a machine without the on-device
    // model must still complete the build.
    let semantic_engine = BoundedSemanticEngine::bind(Path::new(&worker_path)).ok();
    let semantic_bound = semantic_engine.is_some();
    // Only bound for the census shape; there is nothing to recognise otherwise.
    let transcriber = census_shape
        .then(|| BoundedTranscriber::bind(Path::new(&worker_path)).ok())
        .flatten();

    let approved = approve_roots(&[root]).expect("approve root");
    let started = Instant::now();
    let vault = build_authorized_document_vault(
        VaultId::parse("archive-pilot-soak").expect("vault id"),
        &approved,
        DocumentVaultLimits {
            excluded_paths: vec![ExcludedFolder {
                root_index: 0,
                relative_path: PathBuf::from("attachments"),
            }],
            ..DocumentVaultLimits::default()
        },
        &AtomicBool::new(false),
        &converter,
        transcriber.as_ref(),
        None,
        semantic_engine,
    )
    .expect("build the document vault at archive scale");
    let elapsed = started.elapsed();
    let report = vault.build_report();

    // The failure that reached the owner: a build that stops partway and
    // reports the remainder as something else.
    assert_eq!(
        report.open_file_limit_reached, 0,
        "the build ran out of descriptors; the ceiling was not raised"
    );
    assert_eq!(
        report.indexed_documents, documents as u64,
        "the build indexed {} of {documents} documents",
        report.indexed_documents
    );
    assert!(
        !report.budget_reached,
        "a default-limit build hit a budget at {documents} documents"
    );
    assert_eq!(
        report.excluded_directories, 1,
        "the excluded folder was entered"
    );

    // Exact evidence is the product and must be complete regardless of what
    // the optional workers did. A query with a term that only a fraction of
    // the archive uses is the realistic case, and it must answer.
    // A quoted phrase, because that is the path that finds language the fixed
    // concept vocabulary does not know -- and finding exact language is what
    // this tool is for. Only every two-hundredth document carries it.
    let response = vault
        .interpret_and_search("Find documents containing \"escrow agent shall release\".")
        .expect("a selective exact search over the soaked index");
    // A document-scope query answers in `documents`; `evidence` is where
    // provision-scope answers land. Asserting on the wrong one reported a
    // healthy index as returning nothing.
    assert!(
        !response.documents.is_empty(),
        "an exact phrase present in the corpus was not found across {documents} documents"
    );
    assert!(
        response
            .documents
            .iter()
            .all(|card| card.exact_phrase_matched),
        "a document answered a phrase query without matching the phrase"
    );

    // A term most of the archive shares is the other realistic case, and at
    // this scale it exceeds the 2,000-candidate ceiling and fails closed
    // rather than ranking. Reported rather than asserted: refusing beats
    // silently incomplete evidence, but whether an attorney searching thirty
    // years of contracts for "confidentiality" should be told to narrow the
    // query is a product decision, not something this harness should settle.
    // What the raised ceiling was raised for: the ordinary query, timed, with
    // the peak memory of the process after it. A ceiling that answers by
    // exhausting the machine is not an answer.
    let broad_started = Instant::now();
    let broad = vault
        .interpret_and_search(
            // One common concept, which is the query an attorney is most
            // likely to type and the one a conjunction cannot narrow.
            "Find confidentiality provisions covering affiliate.",
        )
        .map(|response| {
            format!(
                "{}cards/{}considered",
                response.evidence.len(),
                response.lexical_candidates_considered
            )
        })
        .unwrap_or_else(|error| format!("refused ({error})"));
    let broad_seconds = broad_started.elapsed().as_secs_f64();
    let peak_megabytes = peak_resident_megabytes();
    // The guarantee the raised ceiling buys, pinned so it cannot quietly go
    // back. Measured at the edge: 44,000 documents produce 24,200 candidate
    // clauses for an ordinary one-term query, answered in 0.24s with the whole
    // process at 232 MB. Below that size an attorney must never be told to
    // narrow an ordinary query.
    assert!(
        documents > 40_000 || !broad.starts_with("refused"),
        "an ordinary one-term query refused on {documents} documents: {broad}"
    );

    println!(
        "archive_pilot_soak=passed shape={} documents={} indexed={} transcribed={} pdfs={} \
         word={} excluded_dirs={} open_file_limit_reached={} semantic_bound={} \
         semantic_partial={} broad_query={} broad_seconds={:.2} peak_rss_mb={} \
         seconds={:.1}",
        if census_shape { "census" } else { "text-only" },
        documents,
        report.indexed_documents,
        report.transcribed_documents,
        report.searchable_pdf_documents,
        report.docx_documents,
        report.excluded_directories,
        report.open_file_limit_reached,
        semantic_bound,
        report.semantic_coverage_partial,
        broad,
        broad_seconds,
        peak_megabytes,
        elapsed.as_secs_f64(),
    );
}
