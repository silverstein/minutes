use minutes_archive_convert::{BoundedConverter, SourceFormat};
use std::io::{Cursor, Write};
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
            .expect("document entry");
        writer
            .write_all(
                br#"<w:document xmlns:w="urn:test"><w:body>
                <w:p><w:r><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
                <w:p><w:r><w:t>Confidential Information includes affiliate data.</w:t></w:r></w:p>
                </w:body></w:document>"#,
            )
            .expect("xml");
        writer.finish().expect("zip");
    }
    cursor.into_inner()
}

#[cfg(target_os = "macos")]
#[test]
fn real_worker_snapshot_sandbox_and_pipe_conversion_are_enforced() {
    let worker = env!("CARGO_BIN_EXE_minutes-archive-convert-worker");
    let converter = BoundedConverter::bind(worker.as_ref()).expect("bind and sandbox self-test");
    let converted = converter
        .convert(SourceFormat::Docx, &synthetic_docx())
        .expect("bounded conversion");
    assert_eq!(converted.blocks.len(), 2);
    assert_eq!(converted.blocks[0].source_anchor, "paragraph:000001");
    assert!(converted.blocks[1]
        .text
        .contains("Confidential Information"));
}
