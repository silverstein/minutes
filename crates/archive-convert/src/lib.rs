//! Bounded byte-to-text conversion for untrusted legal documents.
//!
//! Parsing functions are public for deterministic fixture tests. Production
//! callers must use the worker entry point so PDF and ZIP/XML parsing never
//! occurs in the Tauri process.

use minutes_archive_worker_control::{RegisteredChild, WorkerProcessControl};
use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::borrow::Cow;
use std::collections::BTreeSet;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use thiserror::Error;
use zip::ZipArchive;

pub const WORKER_MARKER: &str = "--minutes-archive-convert-worker-v1";
pub const PDF_UNSUPPORTED_STRUCTURE_WARNING: &str = "pdf_unsupported_structure_signal";
pub const MAX_SOURCE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OUTPUT_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_BLOCKS: usize = 10_000;
pub const MAX_DOCX_ENTRIES: usize = 2_000;
pub const MAX_DOCX_XML_BYTES: usize = 24 * 1024 * 1024;
const WORKER_CPU_SECONDS: u64 = 15;
#[cfg(target_os = "macos")]
const WORKER_MEMORY_GROWTH_BYTES: u64 = 1024 * 1024 * 1024;
const WORKER_DEADLINE: Duration = Duration::from_secs(20);
const MAX_WORKER_STDERR_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Pdf,
    Docx,
    /// Binary Word 97-2003.
    Doc,
    /// OpenDocument Text.
    Odt,
    /// Rich Text Format.
    Rtf,
}

impl SourceFormat {
    pub fn parse(value: &str) -> Result<Self, ConversionError> {
        match value {
            "pdf" => Ok(Self::Pdf),
            "docx" => Ok(Self::Docx),
            "doc" => Ok(Self::Doc),
            "odt" => Ok(Self::Odt),
            "rtf" => Ok(Self::Rtf),
            _ => Err(ConversionError::UnsupportedFormat),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
            Self::Doc => "doc",
            Self::Odt => "odt",
            Self::Rtf => "rtf",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorFlow {
    HardBoundary,
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedBlock {
    pub source_anchor: String,
    pub text: String,
    pub flow: AnchorFlow,
    /// Whether the source marked this block as a heading.
    ///
    /// Documents record their own structure and retrieval should read it
    /// rather than guess from the text. DOCX carries `w:pStyle` when Word
    /// styles are used and, when they are not, run properties: a caption set
    /// in 24pt bold over 12pt body is unambiguous in the file and invisible
    /// to any lexical rule. Guessing produced five successive regressions --
    /// promoting cross-references onto unrelated clauses, and demoting real
    /// captions until genuine provisions returned nothing.
    ///
    /// `None` means the format carried no structural signal, not that the
    /// block is body text.
    #[serde(default)]
    pub is_heading: Option<bool>,
}

/// Where a converted document's characters came from.
///
/// A typed verdict rather than a warning string, for the same reason
/// provision boundaries stopped being one: a hard rule that depends on a
/// value a caller can omit is not a rule. This field has no serde default on
/// purpose -- a payload that does not state its origin fails to parse instead
/// of quietly parsing as quotable.
///
/// `MachineReadLayer` summarizes a document whose pages are all machine
/// readings of page scans. Mixed PDFs remain author-written at document level
/// while `machine_read_anchors` identifies the individual transcribed pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextOrigin {
    /// The characters were written into the file by its author's software.
    AuthorWritten,
    /// The characters are an embedded machine reading of a page image.
    MachineReadLayer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedDocument {
    pub format: SourceFormat,
    pub blocks: Vec<ConvertedBlock>,
    pub warnings: Vec<String>,
    /// See [`TextOrigin`]. Required in the worker payload: no default.
    pub text_origin: TextOrigin,
    /// Page anchors whose text must be treated as machine-read.
    ///
    /// Required in the worker payload: page provenance is a quoting boundary,
    /// not optional metadata that an older producer may silently omit. The PDF
    /// converter records only pages whose raster coverage passes the scan rule;
    /// the OCR path likewise records individual pages.
    pub machine_read_anchors: BTreeSet<String>,
}

impl ConvertedDocument {
    pub fn validate(&self) -> Result<(), ConversionError> {
        if self.blocks.len() > MAX_BLOCKS {
            return Err(ConversionError::OutputBudgetExceeded);
        }
        let mut output_bytes = 0usize;
        for block in &self.blocks {
            if block.source_anchor.is_empty()
                || block.source_anchor.len() > 128
                || block
                    .source_anchor
                    .bytes()
                    .any(|byte| byte.is_ascii_control())
                || block.text.contains('\0')
            {
                return Err(ConversionError::MalformedOutput);
            }
            output_bytes = output_bytes
                .checked_add(block.text.len())
                .ok_or(ConversionError::OutputBudgetExceeded)?;
            if output_bytes > MAX_OUTPUT_BYTES {
                return Err(ConversionError::OutputBudgetExceeded);
            }
        }
        if self.warnings.len() > 32
            || self
                .warnings
                .iter()
                .any(|warning| warning.len() > 256 || warning.chars().any(char::is_control))
        {
            return Err(ConversionError::MalformedOutput);
        }
        if self.machine_read_anchors.iter().any(|anchor| {
            anchor.len() > 128
                || anchor.chars().any(char::is_control)
                || anchor
                    .strip_prefix("page:")
                    .and_then(|digits| digits.parse::<u32>().ok())
                    .is_none_or(|page| page == 0)
        }) {
            return Err(ConversionError::MalformedOutput);
        }
        Ok(())
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConversionError {
    #[error("the source format is not supported")]
    UnsupportedFormat,
    #[error("the source is empty or exceeds the input budget")]
    InputBudgetExceeded,
    #[error("the source could not be converted")]
    MalformedSource,
    #[error("the converted document exceeded its output budget")]
    OutputBudgetExceeded,
    #[error("the converter emitted malformed output")]
    MalformedOutput,
    #[error("the conversion worker could not install its security boundary")]
    SecurityBoundaryUnavailable,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WorkerError {
    #[error("the conversion worker executable is unavailable or mutable")]
    ExecutableUnavailable,
    #[error("the conversion worker security self-test failed")]
    SecuritySelfTestFailed,
    #[error("the conversion worker exceeded its deadline or output budget")]
    WorkerBudgetExceeded,
    #[error("the conversion worker stopped without a valid result")]
    WorkerFailed,
    #[error("the source was refused by the bounded converter")]
    SourceRefused,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResponse {
    document: Option<ConvertedDocument>,
    error: Option<String>,
}

pub struct BoundedConverter {
    executable_path: PathBuf,
    /// Held open with `O_NOFOLLOW` for the object's lifetime. The descriptor
    /// pins one inode, so the digest below is re-read from the same file no
    /// matter what happens to the path.
    executable: fs::File,
    executable_identity: FileIdentity,
    executable_bytes: u64,
    executable_digest: [u8; 32],
    process_control: Option<WorkerProcessControl>,
}

/// Device and inode of the pinned worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
}

#[cfg(unix)]
fn file_identity(metadata: &fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
fn file_identity(_metadata: &fs::Metadata) -> FileIdentity {
    FileIdentity {
        device: 0,
        inode: 0,
    }
}

impl std::fmt::Debug for BoundedConverter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BoundedConverter([pinned worker executable])")
    }
}

impl BoundedConverter {
    pub fn bind(worker_executable: &Path) -> Result<Self, WorkerError> {
        Self::bind_inner(worker_executable, None)
    }

    pub fn bind_with_process_control(
        worker_executable: &Path,
        process_control: WorkerProcessControl,
    ) -> Result<Self, WorkerError> {
        Self::bind_inner(worker_executable, Some(process_control))
    }

    fn bind_inner(
        worker_executable: &Path,
        process_control: Option<WorkerProcessControl>,
    ) -> Result<Self, WorkerError> {
        let canonical =
            fs::canonicalize(worker_executable).map_err(|_| WorkerError::ExecutableUnavailable)?;
        let lexical =
            fs::symlink_metadata(&canonical).map_err(|_| WorkerError::ExecutableUnavailable)?;
        if lexical.file_type().is_symlink() || !lexical.is_file() {
            return Err(WorkerError::ExecutableUnavailable);
        }
        let mut source_options = fs::OpenOptions::new();
        source_options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            source_options.custom_flags(libc::O_NOFOLLOW);
        }
        let source = source_options
            .open(&canonical)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        let source_metadata = source
            .metadata()
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        if !source_metadata.is_file() {
            return Err(WorkerError::ExecutableUnavailable);
        }

        // The worker executes in place, from inside the application bundle.
        //
        // It used to be copied to a private temp directory and run from there,
        // so the bytes could not change between verification and use. That is
        // sound reasoning and it produced an app that could not work at all: a
        // Developer ID signature with the hardened runtime is bound to its
        // bundle, so the copy fails validation -- `codesign` reports "invalid
        // Info.plist (plist or signature have been modified)" -- and the kernel
        // SIGKILLs it the moment it is exec'd. Every notarized build was
        // therefore unable to build an index, while every test passed, because
        // local testing used an ad-hoc-signed app (whose copy runs fine) and CI
        // exercised the unsigned build.
        //
        // The property the copy provided is kept without it. This descriptor
        // pins one inode for the object's lifetime, and `verify_executable`
        // re-reads the digest through it and confirms the path still resolves
        // to that same inode before every launch. A swapped file changes the
        // inode and is refused; a rewritten one changes the digest and is
        // refused.
        let executable_identity = file_identity(&source_metadata);
        let (executable_bytes, executable_digest) =
            digest_file(&source).map_err(|_| WorkerError::ExecutableUnavailable)?;
        if executable_bytes != source_metadata.len() {
            return Err(WorkerError::ExecutableUnavailable);
        }
        let converter = Self {
            executable_path: canonical,
            executable: source,
            executable_identity,
            executable_bytes,
            executable_digest,
            process_control,
        };
        converter.verify_sandbox()?;
        Ok(converter)
    }

    pub fn convert(
        &self,
        format: SourceFormat,
        source: &[u8],
    ) -> Result<ConvertedDocument, WorkerError> {
        if source.is_empty() || source.len() > MAX_SOURCE_BYTES {
            return Err(WorkerError::SourceRefused);
        }
        self.verify_executable()?;
        let mut input = Vec::with_capacity(8 + source.len());
        input.extend_from_slice(&(source.len() as u64).to_le_bytes());
        input.extend_from_slice(source);
        let output = self.launch(format.as_str(), input)?;
        if !output.success {
            return Err(WorkerError::SourceRefused);
        }
        let response: WorkerResponse =
            serde_json::from_slice(&output.stdout).map_err(|_| WorkerError::WorkerFailed)?;
        let document = response.document.ok_or(WorkerError::SourceRefused)?;
        if response.error.is_some() || document.format != format {
            return Err(WorkerError::WorkerFailed);
        }
        document.validate().map_err(|_| WorkerError::WorkerFailed)?;
        Ok(document)
    }

    fn verify_sandbox(&self) -> Result<(), WorkerError> {
        self.verify_executable()?;
        let output = self.launch("sandbox-self-test", Vec::new())?;
        if output.success {
            Ok(())
        } else {
            Err(WorkerError::SecuritySelfTestFailed)
        }
    }

    /// Confirm the worker is still the exact file this object bound to.
    ///
    /// Runs immediately before every launch. The write-bit refusal that used
    /// to live here was possible only because the worker was a private copy
    /// the code had just created at mode 0500; an application bundle's
    /// executable is 0755 like every other installed binary, so demanding no
    /// write bits would refuse every real installation. Identity and content
    /// are what actually matter, and both are checked against the descriptor
    /// opened at bind time rather than against the path.
    fn verify_executable(&self) -> Result<(), WorkerError> {
        let metadata = fs::symlink_metadata(&self.executable_path)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(WorkerError::ExecutableUnavailable);
        }
        // The path must still lead to the inode this object pinned. A binary
        // swapped in beneath us lands on a different one.
        if file_identity(&metadata) != self.executable_identity {
            return Err(WorkerError::ExecutableUnavailable);
        }
        let pinned = self
            .executable
            .metadata()
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        if file_identity(&pinned) != self.executable_identity {
            return Err(WorkerError::ExecutableUnavailable);
        }
        // Re-read through the descriptor, so this measures the pinned inode
        // and not whatever the path happens to name now.
        let (bytes, digest) =
            digest_file(&self.executable).map_err(|_| WorkerError::ExecutableUnavailable)?;
        if bytes != self.executable_bytes || digest != self.executable_digest {
            return Err(WorkerError::ExecutableUnavailable);
        }
        Ok(())
    }

    fn launch(&self, operation: &str, input: Vec<u8>) -> Result<WorkerOutput, WorkerError> {
        let mut command = Command::new(&self.executable_path);
        command
            .arg(WORKER_MARKER)
            .arg(operation)
            .env_clear()
            .current_dir("/")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            unsafe {
                command.pre_exec(|| {
                    if libc::setpgid(0, 0) != 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
        }
        let mut child = RegisteredChild::spawn(&mut command, self.process_control.as_ref())
            .map_err(|_| WorkerError::WorkerFailed)?;
        let mut stdin = child
            .child_mut()
            .stdin
            .take()
            .ok_or(WorkerError::WorkerFailed)?;
        let stdout = child
            .child_mut()
            .stdout
            .take()
            .ok_or(WorkerError::WorkerFailed)?;
        let stderr = child
            .child_mut()
            .stderr
            .take()
            .ok_or(WorkerError::WorkerFailed)?;
        let input_writer = thread::spawn(move || {
            let result = stdin.write_all(&input).and_then(|_| stdin.flush());
            drop(stdin);
            result
        });
        let stdout_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stdout
                .take((MAX_OUTPUT_BYTES as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });
        let stderr_reader = thread::spawn(move || {
            let mut bytes = Vec::new();
            stderr
                .take((MAX_WORKER_STDERR_BYTES as u64).saturating_add(1))
                .read_to_end(&mut bytes)
                .map(|_| bytes)
        });

        let deadline = Instant::now() + WORKER_DEADLINE;
        let exit_status = loop {
            if self
                .process_control
                .as_ref()
                .is_some_and(WorkerProcessControl::is_cancelled)
            {
                child.terminate();
                let _ = input_writer.join();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(WorkerError::WorkerBudgetExceeded);
            }
            let status = match child.try_wait() {
                Ok(status) => status,
                Err(_) => {
                    child.terminate();
                    let _ = input_writer.join();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(WorkerError::WorkerFailed);
                }
            };
            match status {
                Some(exit_status) => break exit_status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => {
                    child.terminate();
                    let _ = input_writer.join();
                    let _ = stdout_reader.join();
                    let _ = stderr_reader.join();
                    return Err(WorkerError::WorkerBudgetExceeded);
                }
            }
        };
        input_writer
            .join()
            .map_err(|_| WorkerError::WorkerFailed)?
            .map_err(|_| WorkerError::WorkerFailed)?;
        let stdout = stdout_reader
            .join()
            .map_err(|_| WorkerError::WorkerFailed)?
            .map_err(|_| WorkerError::WorkerFailed)?;
        let stderr = stderr_reader
            .join()
            .map_err(|_| WorkerError::WorkerFailed)?
            .map_err(|_| WorkerError::WorkerFailed)?;
        if stdout.len() > MAX_OUTPUT_BYTES || stderr.len() > MAX_WORKER_STDERR_BYTES {
            return Err(WorkerError::WorkerBudgetExceeded);
        }
        Ok(WorkerOutput {
            success: exit_status.success(),
            stdout,
        })
    }
}

#[derive(Debug)]
struct WorkerOutput {
    success: bool,
    stdout: Vec<u8>,
}

/// Measure the pinned worker without moving anyone's file offset.
///
/// This used to rewind a `try_clone` of the descriptor. `try_clone` is `dup`,
/// and duplicated descriptors share one open file description and therefore one
/// offset: rewinding the clone rewinds the original. Two threads verifying the
/// worker at the same moment then read each other's bytes and both conclude the
/// executable had been swapped underneath them. That is precisely what happened
/// the first time document extraction ran on more than one thread -- every
/// concurrent conversion failed as "unavailable or mutable" while the file on
/// disk had not changed at all. Positional reads have no shared cursor to race
/// over, and the check still measures the pinned inode rather than the path.
#[cfg(unix)]
fn digest_file(file: &fs::File) -> Result<(u64, [u8; 32]), std::io::Error> {
    use std::os::unix::fs::FileExt;

    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut offset = 0u64;
    loop {
        match file.read_at(&mut buffer, offset) {
            Ok(0) => break,
            Ok(read) => {
                hasher.update(&buffer[..read]);
                offset = offset.saturating_add(read as u64);
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok((offset, hasher.finalize().into()))
}

#[cfg(not(unix))]
fn digest_file(file: &fs::File) -> Result<(u64, [u8; 32]), std::io::Error> {
    use std::io::{Seek, SeekFrom};

    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher)?;
    Ok((bytes, hasher.finalize().into()))
}

pub fn convert_bytes(
    format: SourceFormat,
    bytes: &[u8],
) -> Result<ConvertedDocument, ConversionError> {
    if bytes.is_empty() || bytes.len() > MAX_SOURCE_BYTES {
        return Err(ConversionError::InputBudgetExceeded);
    }
    let document = match format {
        SourceFormat::Pdf => convert_pdf(bytes)?,
        SourceFormat::Docx => convert_docx(bytes)?,
        SourceFormat::Doc => convert_via_anydoc(bytes, anydoc::Format::Doc, format)?,
        SourceFormat::Odt => convert_via_anydoc(bytes, anydoc::Format::Odt, format)?,
        SourceFormat::Rtf => convert_via_anydoc(bytes, anydoc::Format::Rtf, format)?,
    };
    document.validate()?;
    Ok(document)
}

/// Convert the word-processor formats that carry legal prose.
///
/// Scope is deliberately narrower than the library's. Spreadsheets,
/// presentations, EPUB and CSV are not routed here: this app segments prose
/// into clauses and quotes them as evidence, and a worksheet has no clauses to
/// find. Feeding one through would produce confident-looking cards built from
/// cell text. PDF is not routed here either -- `to_document` does not support
/// it, and the existing path keeps glyph geometry, which is the only structure
/// a PDF has and which Markdown would discard.
///
/// The parser is treated as hostile, as every parser here is: it runs in the
/// converter worker under `(deny default)` seatbelt with `RLIMIT_AS` and
/// `RLIMIT_CPU` already bound, and only bytes cross that boundary. That matters
/// more than usual for these formats, whose containers are classic exploit
/// carriers, and for a dependency this new.
fn convert_via_anydoc(
    bytes: &[u8],
    parser_format: anydoc::Format,
    format: SourceFormat,
) -> Result<ConvertedDocument, ConversionError> {
    let document =
        anydoc::to_document(bytes, parser_format).map_err(|_| ConversionError::MalformedSource)?;

    let mut blocks = Vec::new();
    let mut output_bytes = 0usize;
    let mut ordinal = 0usize;
    for block in &document.blocks {
        let (text, is_heading) = match block {
            anydoc::model::Block::Heading { content, .. } => (inline_text(content), true),
            anydoc::model::Block::Paragraph(content) => (inline_text(content), false),
            // Everything else -- tables, lists, code, rules, images -- is
            // skipped rather than flattened. A table rendered as a run of
            // sentences reads like prose and would be quoted as a clause.
            _ => continue,
        };
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        output_bytes = output_bytes
            .checked_add(text.len())
            .and_then(|value| value.checked_add(1))
            .ok_or(ConversionError::OutputBudgetExceeded)?;
        if output_bytes > MAX_OUTPUT_BYTES || blocks.len() >= MAX_BLOCKS {
            return Err(ConversionError::OutputBudgetExceeded);
        }
        ordinal += 1;
        blocks.push(ConvertedBlock {
            source_anchor: format!("paragraph:{ordinal:06}"),
            text,
            flow: AnchorFlow::Continue,
            is_heading: Some(is_heading),
        });
    }

    let warnings = if blocks.is_empty() {
        vec!["ocr_required_or_no_extractable_text".to_string()]
    } else if format == SourceFormat::Rtf {
        // RTF is withheld from same-clause answers outright.
        //
        // A first attempt keyed this on whether the parsed document contained
        // a heading, which is the same document-level reasoning that produced
        // the PDF defect: one outline level anywhere marked the whole file
        // trustworthy. A reviewer built the RTF that turns that into a wrong
        // answer, and `\outlinelevel` is in fact recognised, so "RTF declares
        // no headings" was not true either.
        //
        // Word and OpenDocument carry a style system that states which
        // paragraphs are captions. RTF outline levels are an afterthought most
        // producers never write, so finding one proves little about the
        // paragraphs around it.
        vec![PDF_UNSUPPORTED_STRUCTURE_WARNING.to_string()]
    } else {
        Vec::new()
    };
    Ok(ConvertedDocument {
        format,
        blocks,
        warnings,
        // Word-processor formats carry the author's characters directly; a
        // scan wrapped in one of these containers has no text to extract at
        // all, so there is no embedded-reading case to detect here.
        text_origin: TextOrigin::AuthorWritten,
        machine_read_anchors: BTreeSet::new(),
    })
}

/// Flatten inline runs to their text, following links into their content.
fn inline_text(inlines: &[anydoc::model::Inline]) -> String {
    let mut out = String::new();
    for inline in inlines {
        match inline {
            anydoc::model::Inline::Text { text, .. } => out.push_str(text),
            anydoc::model::Inline::Link { content, .. } => out.push_str(&inline_text(content)),
            _ => {}
        }
    }
    out
}

/// Raster coverage, rather than any individual image's shape, identifies a
/// picture of a page. A 32x32 grid over the visible page box makes unions of
/// strips, tiles, overlaps, cropped scans, and rectangular clips bounded and
/// deterministic.
const PDF_COVERAGE_GRID_SIDE: usize = 32;
const PDF_COVERAGE_GRID_CELLS: usize = PDF_COVERAGE_GRID_SIDE * PDF_COVERAGE_GRID_SIDE;
const PDF_SCAN_MIN_COVERED_CELLS: usize = PDF_COVERAGE_GRID_CELLS / 2;
const PDF_SCAN_MIN_CONTRIBUTING_PIXELS: u64 = 250_000;
const MAX_PDF_CONTENT_DEPTH: usize = 32;
const MAX_PDF_CONTENT_VISITS: usize = 16_384;

#[derive(Debug, Clone, Copy)]
struct PdfRect {
    x_min: f64,
    y_min: f64,
    x_max: f64,
    y_max: f64,
}

impl PdfRect {
    fn from_object(doc: &lopdf::Document, object: &lopdf::Object) -> Result<Self, ()> {
        let coordinates = doc
            .dereference(object)
            .map_err(|_| ())?
            .1
            .as_array()
            .map_err(|_| ())?;
        let [x0, y0, x1, y1] = coordinates.as_slice() else {
            return Err(());
        };
        let x0 = resolved_number(doc, x0)?;
        let y0 = resolved_number(doc, y0)?;
        let x1 = resolved_number(doc, x1)?;
        let y1 = resolved_number(doc, y1)?;
        let rect = Self {
            x_min: x0.min(x1),
            y_min: y0.min(y1),
            x_max: x0.max(x1),
            y_max: y0.max(y1),
        };
        if !rect.x_min.is_finite()
            || !rect.y_min.is_finite()
            || !rect.x_max.is_finite()
            || !rect.y_max.is_finite()
            || rect.x_max <= rect.x_min
            || rect.y_max <= rect.y_min
        {
            return Err(());
        }
        Ok(rect)
    }

    fn width(self) -> f64 {
        self.x_max - self.x_min
    }

    fn height(self) -> f64 {
        self.y_max - self.y_min
    }

    fn intersection(self, other: Self) -> Result<Self, ()> {
        let intersection = Self {
            x_min: self.x_min.max(other.x_min),
            y_min: self.y_min.max(other.y_min),
            x_max: self.x_max.min(other.x_max),
            y_max: self.y_max.min(other.y_max),
        };
        (intersection.x_max > intersection.x_min && intersection.y_max > intersection.y_min)
            .then_some(intersection)
            .ok_or(())
    }
}

#[derive(Debug, Clone, Copy)]
struct PdfMatrix {
    a: f64,
    b: f64,
    c: f64,
    d: f64,
    e: f64,
    f: f64,
}

impl PdfMatrix {
    const IDENTITY: Self = Self {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn from_objects(doc: &lopdf::Document, operands: &[lopdf::Object]) -> Result<Self, ()> {
        let [a, b, c, d, e, f] = operands else {
            return Err(());
        };
        let matrix = Self {
            a: resolved_number(doc, a)?,
            b: resolved_number(doc, b)?,
            c: resolved_number(doc, c)?,
            d: resolved_number(doc, d)?,
            e: resolved_number(doc, e)?,
            f: resolved_number(doc, f)?,
        };
        matrix.is_finite().then_some(matrix).ok_or(())
    }

    fn from_array(doc: &lopdf::Document, object: &lopdf::Object) -> Result<Self, ()> {
        let array = doc
            .dereference(object)
            .map_err(|_| ())?
            .1
            .as_array()
            .map_err(|_| ())?;
        Self::from_objects(doc, array)
    }

    /// Compose a local transform after the current local-to-page transform.
    fn concat(self, local: Self) -> Self {
        Self {
            a: self.a * local.a + self.c * local.b,
            b: self.b * local.a + self.d * local.b,
            c: self.a * local.c + self.c * local.d,
            d: self.b * local.c + self.d * local.d,
            e: self.a * local.e + self.c * local.f + self.e,
            f: self.b * local.e + self.d * local.f + self.f,
        }
    }

    fn transform(self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    fn transform_rect(self, rect: PdfRect) -> Result<PdfRect, ()> {
        let corners = [
            self.transform(rect.x_min, rect.y_min),
            self.transform(rect.x_min, rect.y_max),
            self.transform(rect.x_max, rect.y_min),
            self.transform(rect.x_max, rect.y_max),
        ];
        let mut x_min = f64::INFINITY;
        let mut y_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut y_max = f64::NEG_INFINITY;
        for (x, y) in corners {
            x_min = x_min.min(x);
            y_min = y_min.min(y);
            x_max = x_max.max(x);
            y_max = y_max.max(y);
        }
        let transformed = PdfRect {
            x_min,
            y_min,
            x_max,
            y_max,
        };
        transformed
            .x_min
            .is_finite()
            .then_some(())
            .filter(|_| {
                transformed.y_min.is_finite()
                    && transformed.x_max.is_finite()
                    && transformed.y_max.is_finite()
            })
            .map(|()| transformed)
            .ok_or(())
    }

    fn unit_square_bbox(self) -> Result<PdfRect, ()> {
        self.transform_rect(PdfRect {
            x_min: 0.0,
            y_min: 0.0,
            x_max: 1.0,
            y_max: 1.0,
        })
    }

    fn is_finite(self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f]
            .into_iter()
            .all(f64::is_finite)
    }
}

#[derive(Debug)]
struct PdfCoverage {
    page: PdfRect,
    covered: [bool; PDF_COVERAGE_GRID_CELLS],
    contributing_pixels: u64,
}

#[derive(Debug, Clone)]
struct PdfCellMask {
    cells: [bool; PDF_COVERAGE_GRID_CELLS],
}

impl PdfCellMask {
    fn empty() -> Self {
        Self {
            cells: [false; PDF_COVERAGE_GRID_CELLS],
        }
    }

    fn full() -> Self {
        Self {
            cells: [true; PDF_COVERAGE_GRID_CELLS],
        }
    }

    fn mark_rect(&mut self, page: PdfRect, rect: PdfRect) {
        for row in 0..PDF_COVERAGE_GRID_SIDE {
            let cell_y_min =
                page.y_min + page.height() * (row as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
            let cell_y_max =
                page.y_min + page.height() * ((row + 1) as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
            for column in 0..PDF_COVERAGE_GRID_SIDE {
                let cell_x_min =
                    page.x_min + page.width() * (column as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
                let cell_x_max = page.x_min
                    + page.width() * ((column + 1) as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
                if rect.x_max > cell_x_min
                    && rect.x_min < cell_x_max
                    && rect.y_max > cell_y_min
                    && rect.y_min < cell_y_max
                {
                    self.cells[row * PDF_COVERAGE_GRID_SIDE + column] = true;
                }
            }
        }
    }

    fn intersect(&mut self, other: &Self) {
        for (cell, other_cell) in self.cells.iter_mut().zip(&other.cells) {
            *cell &= *other_cell;
        }
    }
}

impl PdfCoverage {
    fn new(page: PdfRect) -> Self {
        Self {
            page,
            covered: [false; PDF_COVERAGE_GRID_CELLS],
            contributing_pixels: 0,
        }
    }

    fn mark_image(
        &mut self,
        width: i64,
        height: i64,
        ctm: PdfMatrix,
        clip: &PdfCellMask,
    ) -> Result<(), ()> {
        let width = u64::try_from(width).map_err(|_| ())?;
        let height = u64::try_from(height).map_err(|_| ())?;
        if width == 0 || height == 0 {
            return Err(());
        }
        let source_pixels = width.checked_mul(height).ok_or(())?;
        let image = ctm.unit_square_bbox()?;
        let mut contributes = false;
        for row in 0..PDF_COVERAGE_GRID_SIDE {
            let cell_y_min = self.page.y_min
                + self.page.height() * (row as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
            let cell_y_max = self.page.y_min
                + self.page.height() * ((row + 1) as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
            for column in 0..PDF_COVERAGE_GRID_SIDE {
                let cell_x_min = self.page.x_min
                    + self.page.width() * (column as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
                let cell_x_max = self.page.x_min
                    + self.page.width() * ((column + 1) as f64) / (PDF_COVERAGE_GRID_SIDE as f64);
                let cell_index = row * PDF_COVERAGE_GRID_SIDE + column;
                if clip.cells[cell_index]
                    && image.x_max > cell_x_min
                    && image.x_min < cell_x_max
                    && image.y_max > cell_y_min
                    && image.y_min < cell_y_max
                {
                    self.covered[cell_index] = true;
                    contributes = true;
                }
            }
        }
        if contributes {
            self.contributing_pixels = self
                .contributing_pixels
                .checked_add(source_pixels)
                .ok_or(())?;
        }
        Ok(())
    }

    fn is_page_scan(&self) -> bool {
        self.covered.iter().filter(|covered| **covered).count() >= PDF_SCAN_MIN_COVERED_CELLS
            && self.contributing_pixels >= PDF_SCAN_MIN_CONTRIBUTING_PIXELS
    }
}

#[derive(Debug)]
struct PdfCurrentPath {
    cells: PdfCellMask,
    supported_rectangles_only: bool,
    clip_pending: bool,
}

impl PdfCurrentPath {
    fn new() -> Self {
        Self {
            cells: PdfCellMask::empty(),
            supported_rectangles_only: true,
            clip_pending: false,
        }
    }

    fn add_rectangle(&mut self, page: PdfRect, rect: PdfRect) {
        self.cells.mark_rect(page, rect);
    }

    fn mark_unsupported_geometry(&mut self) {
        self.supported_rectangles_only = false;
    }

    fn clear(&mut self) {
        *self = Self::new();
    }
}

#[derive(Debug, Clone)]
struct PdfSavedGraphicsState {
    ctm: PdfMatrix,
    clip: PdfCellMask,
}

#[derive(Debug)]
struct PdfGraphicsState {
    ctm: PdfMatrix,
    clip: PdfCellMask,
    path: PdfCurrentPath,
    stack: Vec<PdfSavedGraphicsState>,
}

impl PdfGraphicsState {
    fn new(ctm: PdfMatrix) -> Self {
        Self::new_with_clip(ctm, PdfCellMask::full())
    }

    fn new_with_clip(ctm: PdfMatrix, clip: PdfCellMask) -> Self {
        Self {
            ctm,
            clip,
            path: PdfCurrentPath::new(),
            stack: Vec::new(),
        }
    }

    fn apply_pending_clip(&mut self) {
        if self.path.clip_pending && self.path.supported_rectangles_only {
            self.clip.intersect(&self.path.cells);
        }
        // Unsupported path geometry is deliberately not used to narrow the
        // clip. That can conservatively withhold a quote, but it cannot hide a
        // visible page scan and turn OCR into an exact quotation.
        self.path.clear();
    }

    fn is_balanced(&self) -> bool {
        self.stack.is_empty() && !self.path.clip_pending
    }
}

#[derive(Debug)]
struct PdfPageAnalyzer<'doc, 'budget> {
    doc: &'doc lopdf::Document,
    budget: &'budget mut PdfStreamBudget,
    coverage: PdfCoverage,
    active_streams: BTreeSet<lopdf::ObjectId>,
    visits: usize,
}

impl<'doc, 'budget> PdfPageAnalyzer<'doc, 'budget> {
    fn new(
        doc: &'doc lopdf::Document,
        budget: &'budget mut PdfStreamBudget,
        page: PdfRect,
    ) -> Self {
        Self {
            doc,
            budget,
            coverage: PdfCoverage::new(page),
            active_streams: BTreeSet::new(),
            visits: 0,
        }
    }

    fn analyze_page(&mut self, page_id: lopdf::ObjectId) -> Result<bool, ()> {
        let page = object_dictionary(self.doc, page_id)?;
        let resources = inherited_page_dictionary(self.doc, page_id, b"Resources")?;
        let mut state = PdfGraphicsState::new(PdfMatrix::IDENTITY);
        if let Ok(contents) = page.get(b"Contents") {
            self.process_content_object(contents, resources, &mut state, 0)?;
        }
        if !state.is_balanced() {
            return Err(());
        }
        if let Ok(annotations) = page.get(b"Annots") {
            self.process_annotations(annotations, resources, 0)?;
        }
        Ok(self.coverage.is_page_scan())
    }

    fn process_content_object(
        &mut self,
        object: &'doc lopdf::Object,
        resources: Option<&'doc lopdf::Dictionary>,
        state: &mut PdfGraphicsState,
        depth: usize,
    ) -> Result<(), ()> {
        if depth > MAX_PDF_CONTENT_DEPTH {
            return Err(());
        }
        match object {
            lopdf::Object::Reference(object_id) => {
                let referenced = self.doc.objects.get(object_id).ok_or(())?;
                match referenced {
                    lopdf::Object::Stream(stream) => {
                        self.process_stream(Some(*object_id), stream, resources, state, depth)
                    }
                    _ => self.process_content_object(referenced, resources, state, depth + 1),
                }
            }
            lopdf::Object::Array(objects) => {
                for object in objects {
                    self.process_content_object(object, resources, state, depth + 1)?;
                }
                Ok(())
            }
            // Content streams are indirect in a valid PDF. A direct stream is
            // not silently accepted as complete provenance evidence.
            lopdf::Object::Stream(_) => Err(()),
            _ => Err(()),
        }
    }

    fn process_stream(
        &mut self,
        object_id: Option<lopdf::ObjectId>,
        stream: &'doc lopdf::Stream,
        resources: Option<&'doc lopdf::Dictionary>,
        state: &mut PdfGraphicsState,
        depth: usize,
    ) -> Result<(), ()> {
        if depth > MAX_PDF_CONTENT_DEPTH {
            return Err(());
        }
        self.visits = self.visits.checked_add(1).ok_or(())?;
        if self.visits > MAX_PDF_CONTENT_VISITS {
            return Err(());
        }
        if object_id.is_some_and(|id| !self.active_streams.insert(id)) {
            return Err(());
        }
        let result = (|| {
            let bytes = decode_stream_for_sweep(stream, self.budget)?;
            let content = lopdf::content::Content::decode_strict(&bytes).map_err(|_| ())?;
            for operation in &content.operations {
                match operation.operator.as_str() {
                    "q" => {
                        if !operation.operands.is_empty() {
                            return Err(());
                        }
                        state.stack.push(PdfSavedGraphicsState {
                            ctm: state.ctm,
                            clip: state.clip.clone(),
                        });
                    }
                    "Q" => {
                        if !operation.operands.is_empty() {
                            return Err(());
                        }
                        let saved = state.stack.pop().ok_or(())?;
                        state.ctm = saved.ctm;
                        state.clip = saved.clip;
                    }
                    "cm" => {
                        let matrix = PdfMatrix::from_objects(self.doc, &operation.operands)?;
                        state.ctm = state.ctm.concat(matrix);
                    }
                    "re" => {
                        let [x, y, width, height] = operation.operands.as_slice() else {
                            return Err(());
                        };
                        let x = resolved_number(self.doc, x)?;
                        let y = resolved_number(self.doc, y)?;
                        let width = resolved_number(self.doc, width)?;
                        let height = resolved_number(self.doc, height)?;
                        let rect = state.ctm.transform_rect(PdfRect {
                            x_min: x.min(x + width),
                            y_min: y.min(y + height),
                            x_max: x.max(x + width),
                            y_max: y.max(y + height),
                        })?;
                        state.path.add_rectangle(self.coverage.page, rect);
                    }
                    "m" | "l" => {
                        if operation.operands.len() != 2 {
                            return Err(());
                        }
                        for operand in &operation.operands {
                            resolved_number(self.doc, operand)?;
                        }
                        state.path.mark_unsupported_geometry();
                    }
                    "c" => {
                        if operation.operands.len() != 6 {
                            return Err(());
                        }
                        for operand in &operation.operands {
                            resolved_number(self.doc, operand)?;
                        }
                        state.path.mark_unsupported_geometry();
                    }
                    "v" | "y" => {
                        if operation.operands.len() != 4 {
                            return Err(());
                        }
                        for operand in &operation.operands {
                            resolved_number(self.doc, operand)?;
                        }
                        state.path.mark_unsupported_geometry();
                    }
                    "h" => {
                        if !operation.operands.is_empty() {
                            return Err(());
                        }
                        state.path.mark_unsupported_geometry();
                    }
                    "W" | "W*" => {
                        if !operation.operands.is_empty() {
                            return Err(());
                        }
                        state.path.clip_pending = true;
                    }
                    "n" | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                        if !operation.operands.is_empty() {
                            return Err(());
                        }
                        state.apply_pending_clip();
                    }
                    "Do" => self.process_xobject(
                        &operation.operands,
                        resources,
                        state.ctm,
                        &state.clip,
                        depth + 1,
                    )?,
                    "BI" => {
                        self.process_inline_image(&operation.operands, state.ctm, &state.clip)?
                    }
                    "gs" => self.process_extgstate(
                        &operation.operands,
                        resources,
                        state.ctm,
                        &state.clip,
                        depth + 1,
                    )?,
                    "scn" | "SCN" => self.process_pattern(
                        &operation.operands,
                        resources,
                        state.ctm,
                        &state.clip,
                        depth + 1,
                    )?,
                    "Tf" => self.process_type3_font(
                        &operation.operands,
                        resources,
                        state.ctm,
                        &state.clip,
                        depth + 1,
                    )?,
                    _ => {}
                }
            }
            Ok(())
        })();
        if let Some(object_id) = object_id {
            self.active_streams.remove(&object_id);
        }
        result
    }

    fn process_xobject(
        &mut self,
        operands: &[lopdf::Object],
        resources: Option<&'doc lopdf::Dictionary>,
        ctm: PdfMatrix,
        clip: &PdfCellMask,
        depth: usize,
    ) -> Result<(), ()> {
        let [name] = operands else {
            return Err(());
        };
        let name = name.as_name().map_err(|_| ())?;
        let object = resource_entry(self.doc, resources, b"XObject", name)?;
        let (object_id, stream) = resolved_stream(self.doc, object)?;
        if stream_is_image_xobject(self.doc, stream)? {
            return self.mark_image_dictionary(&stream.dict, b"Width", b"Height", ctm, clip);
        }
        if !stream_is_form_xobject(self.doc, stream)? {
            return Err(());
        }
        self.process_form(object_id, stream, resources, ctm, clip, depth)
    }

    fn process_inline_image(
        &mut self,
        operands: &[lopdf::Object],
        ctm: PdfMatrix,
        clip: &PdfCellMask,
    ) -> Result<(), ()> {
        let [lopdf::Object::Stream(image)] = operands else {
            return Err(());
        };
        let width_key = if image.dict.get(b"W").is_ok() {
            b"W".as_slice()
        } else {
            b"Width".as_slice()
        };
        let height_key = if image.dict.get(b"H").is_ok() {
            b"H".as_slice()
        } else {
            b"Height".as_slice()
        };
        self.mark_image_dictionary(&image.dict, width_key, height_key, ctm, clip)
    }

    fn mark_image_dictionary(
        &mut self,
        dictionary: &lopdf::Dictionary,
        width_key: &[u8],
        height_key: &[u8],
        ctm: PdfMatrix,
        clip: &PdfCellMask,
    ) -> Result<(), ()> {
        let width = resolved_dict_value(self.doc, dictionary, width_key)?
            .as_i64()
            .map_err(|_| ())?;
        let height = resolved_dict_value(self.doc, dictionary, height_key)?
            .as_i64()
            .map_err(|_| ())?;
        self.coverage.mark_image(width, height, ctm, clip)
    }

    fn process_form(
        &mut self,
        object_id: Option<lopdf::ObjectId>,
        stream: &'doc lopdf::Stream,
        parent_resources: Option<&'doc lopdf::Dictionary>,
        ctm: PdfMatrix,
        parent_clip: &PdfCellMask,
        depth: usize,
    ) -> Result<(), ()> {
        let matrix = optional_matrix(self.doc, &stream.dict, b"Matrix")?;
        let bbox = PdfRect::from_object(self.doc, stream.dict.get(b"BBox").map_err(|_| ())?)?;
        let resources =
            optional_dictionary(self.doc, &stream.dict, b"Resources")?.or(parent_resources);
        let form_ctm = ctm.concat(matrix);
        let mut clip = parent_clip.clone();
        let mut bbox_mask = PdfCellMask::empty();
        bbox_mask.mark_rect(self.coverage.page, form_ctm.transform_rect(bbox)?);
        clip.intersect(&bbox_mask);
        let mut state = PdfGraphicsState::new_with_clip(form_ctm, clip);
        self.process_stream(object_id, stream, resources, &mut state, depth)?;
        state.is_balanced().then_some(()).ok_or(())
    }

    fn process_extgstate(
        &mut self,
        operands: &[lopdf::Object],
        resources: Option<&'doc lopdf::Dictionary>,
        ctm: PdfMatrix,
        clip: &PdfCellMask,
        depth: usize,
    ) -> Result<(), ()> {
        let [name] = operands else {
            return Err(());
        };
        let name = name.as_name().map_err(|_| ())?;
        let extgstate = resource_entry(self.doc, resources, b"ExtGState", name)?;
        let extgstate = resolved_dictionary(self.doc, extgstate)?;
        let Ok(soft_mask) = extgstate.get(b"SMask") else {
            return Ok(());
        };
        let soft_mask = self.doc.dereference(soft_mask).map_err(|_| ())?.1;
        if soft_mask.as_name().is_ok_and(|name| name == b"None") {
            return Ok(());
        }
        let soft_mask = soft_mask.as_dict().map_err(|_| ())?;
        let group = soft_mask.get(b"G").map_err(|_| ())?;
        let (object_id, stream) = resolved_stream(self.doc, group)?;
        if !stream_is_form_xobject(self.doc, stream)? {
            return Err(());
        }
        self.process_form(object_id, stream, resources, ctm, clip, depth)
    }

    fn process_pattern(
        &mut self,
        operands: &[lopdf::Object],
        resources: Option<&'doc lopdf::Dictionary>,
        ctm: PdfMatrix,
        clip: &PdfCellMask,
        depth: usize,
    ) -> Result<(), ()> {
        let Some(name) = operands
            .iter()
            .rev()
            .find_map(|operand| operand.as_name().ok())
        else {
            return Ok(());
        };
        let pattern = resource_entry(self.doc, resources, b"Pattern", name)?;
        let (object_id, stream) = resolved_stream(self.doc, pattern)?;
        if !stream_is_tiling_pattern(self.doc, stream)? {
            return Ok(());
        }
        let matrix = optional_matrix(self.doc, &stream.dict, b"Matrix")?;
        let pattern_resources =
            optional_dictionary(self.doc, &stream.dict, b"Resources")?.or(resources);
        let mut state = PdfGraphicsState::new_with_clip(ctm.concat(matrix), clip.clone());
        self.process_stream(object_id, stream, pattern_resources, &mut state, depth)?;
        state.is_balanced().then_some(()).ok_or(())
    }

    fn process_type3_font(
        &mut self,
        operands: &[lopdf::Object],
        resources: Option<&'doc lopdf::Dictionary>,
        ctm: PdfMatrix,
        clip: &PdfCellMask,
        depth: usize,
    ) -> Result<(), ()> {
        let [name, _size] = operands else {
            return Err(());
        };
        let name = name.as_name().map_err(|_| ())?;
        let font = resource_entry(self.doc, resources, b"Font", name)?;
        let font = resolved_dictionary(self.doc, font)?;
        if !dictionary_name_is(self.doc, font, b"Subtype", b"Type3")? {
            return Ok(());
        }
        let font_matrix = optional_matrix(self.doc, font, b"FontMatrix")?;
        let char_procs = resolved_dict_value(self.doc, font, b"CharProcs")?
            .as_dict()
            .map_err(|_| ())?;
        let font_resources = optional_dictionary(self.doc, font, b"Resources")?.or(resources);
        for (_, char_proc) in char_procs.iter() {
            let (object_id, stream) = resolved_stream(self.doc, char_proc)?;
            let mut state = PdfGraphicsState::new_with_clip(ctm.concat(font_matrix), clip.clone());
            self.process_stream(object_id, stream, font_resources, &mut state, depth)?;
            if !state.is_balanced() {
                return Err(());
            }
        }
        Ok(())
    }

    fn process_annotations(
        &mut self,
        object: &'doc lopdf::Object,
        page_resources: Option<&'doc lopdf::Dictionary>,
        depth: usize,
    ) -> Result<(), ()> {
        if depth > MAX_PDF_CONTENT_DEPTH {
            return Err(());
        }
        let object = self.doc.dereference(object).map_err(|_| ())?.1;
        match object {
            lopdf::Object::Array(annotations) => {
                for annotation in annotations {
                    self.process_annotations(annotation, page_resources, depth + 1)?;
                }
                Ok(())
            }
            lopdf::Object::Dictionary(annotation) => {
                let Ok(appearance) = annotation.get(b"AP") else {
                    return Ok(());
                };
                let rect =
                    PdfRect::from_object(self.doc, annotation.get(b"Rect").map_err(|_| ())?)?;
                self.process_appearance(appearance, rect, page_resources, depth + 1)
            }
            _ => Err(()),
        }
    }

    fn process_appearance(
        &mut self,
        object: &'doc lopdf::Object,
        annotation_rect: PdfRect,
        page_resources: Option<&'doc lopdf::Dictionary>,
        depth: usize,
    ) -> Result<(), ()> {
        if depth > MAX_PDF_CONTENT_DEPTH {
            return Err(());
        }
        match object {
            lopdf::Object::Reference(object_id) => {
                let referenced = self.doc.objects.get(object_id).ok_or(())?;
                if let lopdf::Object::Stream(stream) = referenced {
                    return self.process_appearance_stream(
                        Some(*object_id),
                        stream,
                        annotation_rect,
                        page_resources,
                        depth,
                    );
                }
                self.process_appearance(referenced, annotation_rect, page_resources, depth + 1)
            }
            lopdf::Object::Dictionary(dictionary) => {
                for (_, appearance) in dictionary.iter() {
                    self.process_appearance(
                        appearance,
                        annotation_rect,
                        page_resources,
                        depth + 1,
                    )?;
                }
                Ok(())
            }
            lopdf::Object::Stream(_) => Err(()),
            _ => Err(()),
        }
    }

    fn process_appearance_stream(
        &mut self,
        object_id: Option<lopdf::ObjectId>,
        stream: &'doc lopdf::Stream,
        annotation_rect: PdfRect,
        page_resources: Option<&'doc lopdf::Dictionary>,
        depth: usize,
    ) -> Result<(), ()> {
        let bbox = PdfRect::from_object(self.doc, stream.dict.get(b"BBox").map_err(|_| ())?)?;
        let matrix = optional_matrix(self.doc, &stream.dict, b"Matrix")?;
        let transformed_bbox = matrix.transform_rect(bbox)?;
        if transformed_bbox.width() <= 0.0 || transformed_bbox.height() <= 0.0 {
            return Err(());
        }
        let scale_x = annotation_rect.width() / transformed_bbox.width();
        let scale_y = annotation_rect.height() / transformed_bbox.height();
        let fit = PdfMatrix {
            a: scale_x,
            b: 0.0,
            c: 0.0,
            d: scale_y,
            e: annotation_rect.x_min - transformed_bbox.x_min * scale_x,
            f: annotation_rect.y_min - transformed_bbox.y_min * scale_y,
        };
        let resources =
            optional_dictionary(self.doc, &stream.dict, b"Resources")?.or(page_resources);
        let mut clip = PdfCellMask::empty();
        clip.mark_rect(self.coverage.page, annotation_rect);
        let mut state = PdfGraphicsState::new_with_clip(fit.concat(matrix), clip);
        self.process_stream(object_id, stream, resources, &mut state, depth)?;
        state.is_balanced().then_some(()).ok_or(())
    }
}

/// Maximum total decoded stream bytes inspected by the provenance sweep.
///
/// PDF conversion already runs in a memory- and time-bounded worker, while
/// this lower format-specific ceiling prevents a document with many compressed
/// streams from consuming the worker's entire allowance. Crossing the limit is
/// an unknown provenance result and therefore fails closed.
const MAX_PDF_SWEEP_DECOMPRESSED_BYTES: usize = 64 * 1024 * 1024;

type PdfScanCheck = Result<bool, ()>;

#[derive(Debug)]
struct PdfStreamBudget {
    used: usize,
    limit: usize,
}

impl PdfStreamBudget {
    fn new(limit: usize) -> Self {
        Self { used: 0, limit }
    }

    fn remaining(&self) -> usize {
        self.limit.saturating_sub(self.used)
    }

    fn charge(&mut self, bytes: usize) -> Result<(), ()> {
        self.used = self.used.checked_add(bytes).ok_or(())?;
        (self.used <= self.limit).then_some(()).ok_or(())
    }
}

#[derive(Debug)]
struct PdfPageScanVerdict {
    machine_read_anchors: BTreeSet<String>,
    page_count: usize,
    had_page_error: bool,
}

fn pdf_page_scan_verdict_with_budget(
    doc: &lopdf::Document,
    decompressed_byte_limit: usize,
) -> Result<PdfPageScanVerdict, ()> {
    object_table_is_complete(doc)?;
    let pages = doc.get_pages();
    if pages.is_empty() {
        return Err(());
    }
    let mut budget = PdfStreamBudget::new(decompressed_byte_limit);
    let mut machine_read_anchors = BTreeSet::new();
    let mut had_page_error = false;
    for (page_number, page_id) in &pages {
        let page_verdict = (|| {
            let media_box = inherited_page_value(doc, *page_id, b"MediaBox")?.ok_or(())?;
            let media_box = PdfRect::from_object(doc, media_box)?;
            let visible_box = match inherited_page_value(doc, *page_id, b"CropBox")? {
                Some(crop_box) => {
                    let crop_box = PdfRect::from_object(doc, crop_box)?;
                    media_box.intersection(crop_box)?
                }
                None => media_box,
            };
            PdfPageAnalyzer::new(doc, &mut budget, visible_box).analyze_page(*page_id)
        })();
        match page_verdict {
            Ok(true) => {
                machine_read_anchors.insert(format!("page:{page_number:04}"));
            }
            Ok(false) => {}
            Err(()) => {
                had_page_error = true;
                machine_read_anchors.insert(format!("page:{page_number:04}"));
            }
        }
    }
    Ok(PdfPageScanVerdict {
        machine_read_anchors,
        page_count: pages.len(),
        had_page_error,
    })
}

fn pdf_page_scan_verdict(doc: &lopdf::Document) -> PdfPageScanVerdict {
    match pdf_page_scan_verdict_with_budget(doc, MAX_PDF_SWEEP_DECOMPRESSED_BYTES) {
        Ok(verdict) if verdict.had_page_error && verdict.machine_read_anchors.is_empty() => {
            let pages = doc.get_pages();
            PdfPageScanVerdict {
                machine_read_anchors: pages
                    .keys()
                    .map(|page_number| format!("page:{page_number:04}"))
                    .collect(),
                page_count: pages.len(),
                had_page_error: true,
            }
        }
        Ok(verdict) => verdict,
        Err(()) => {
            let pages = doc.get_pages();
            PdfPageScanVerdict {
                machine_read_anchors: pages
                    .keys()
                    .map(|page_number| format!("page:{page_number:04}"))
                    .collect(),
                page_count: pages.len(),
                had_page_error: true,
            }
        }
    }
}

/// Walk each page's content operators with its own graphics state and raster
/// coverage grid. Image bytes are never decoded; only content-bearing streams
/// are decoded so their bounded q/Q/cm and image invocation operators can be
/// interpreted. A malformed page is recorded as machine-read without flattening
/// the verdict onto independently readable pages.
#[cfg(test)]
fn pdf_has_page_scan_image(doc: &lopdf::Document) -> PdfScanCheck {
    pdf_has_page_scan_image_with_budget(doc, MAX_PDF_SWEEP_DECOMPRESSED_BYTES)
}

#[cfg(test)]
fn pdf_has_page_scan_image_with_budget(
    doc: &lopdf::Document,
    decompressed_byte_limit: usize,
) -> PdfScanCheck {
    let verdict = pdf_page_scan_verdict_with_budget(doc, decompressed_byte_limit)?;
    if verdict.had_page_error {
        Err(())
    } else {
        Ok(!verdict.machine_read_anchors.is_empty())
    }
}

fn resolved_number(doc: &lopdf::Document, object: &lopdf::Object) -> Result<f64, ()> {
    match doc.dereference(object).map_err(|_| ())?.1 {
        lopdf::Object::Integer(value) => Ok(*value as f64),
        lopdf::Object::Real(value) => Ok(f64::from(*value)),
        _ => Err(()),
    }
}

/// lopdf deliberately skips malformed indirect objects while loading the rest
/// of a PDF. Compare the parsed map to every live xref entry so a skipped object
/// cannot disappear from an allegedly complete flat sweep.
fn object_table_is_complete(doc: &lopdf::Document) -> Result<(), ()> {
    for (&object_number, entry) in &doc.reference_table.entries {
        let expected_id = match entry {
            lopdf::xref::XrefEntry::Normal { generation, .. } => Some((object_number, *generation)),
            lopdf::xref::XrefEntry::Compressed { .. } => Some((object_number, 0)),
            lopdf::xref::XrefEntry::Free | lopdf::xref::XrefEntry::UnusableFree => None,
        };
        if expected_id.is_some_and(|object_id| !doc.objects.contains_key(&object_id)) {
            return Err(());
        }
    }
    Ok(())
}

fn object_dictionary(
    doc: &lopdf::Document,
    object_id: lopdf::ObjectId,
) -> Result<&lopdf::Dictionary, ()> {
    doc.objects
        .get(&object_id)
        .ok_or(())?
        .as_dict()
        .map_err(|_| ())
}

fn inherited_page_value<'a>(
    doc: &'a lopdf::Document,
    page_id: lopdf::ObjectId,
    key: &[u8],
) -> Result<Option<&'a lopdf::Object>, ()> {
    let mut current = page_id;
    let mut visited = BTreeSet::new();
    for _ in 0..=MAX_PDF_CONTENT_DEPTH {
        if !visited.insert(current) {
            return Err(());
        }
        let dictionary = object_dictionary(doc, current)?;
        if let Ok(value) = dictionary.get(key) {
            return doc
                .dereference(value)
                .map(|(_, value)| Some(value))
                .map_err(|_| ());
        }
        let Ok(parent) = dictionary.get(b"Parent") else {
            return Ok(None);
        };
        current = parent.as_reference().map_err(|_| ())?;
    }
    Err(())
}

fn inherited_page_dictionary<'a>(
    doc: &'a lopdf::Document,
    page_id: lopdf::ObjectId,
    key: &[u8],
) -> Result<Option<&'a lopdf::Dictionary>, ()> {
    inherited_page_value(doc, page_id, key)?
        .map(|object| object.as_dict().map_err(|_| ()))
        .transpose()
}

fn stream_is_image_xobject(doc: &lopdf::Document, stream: &lopdf::Stream) -> PdfScanCheck {
    dictionary_name_is(doc, &stream.dict, b"Subtype", b"Image")
}

fn stream_is_form_xobject(doc: &lopdf::Document, stream: &lopdf::Stream) -> PdfScanCheck {
    dictionary_name_is(doc, &stream.dict, b"Subtype", b"Form")
}

fn stream_is_tiling_pattern(doc: &lopdf::Document, stream: &lopdf::Stream) -> PdfScanCheck {
    if !dictionary_name_is(doc, &stream.dict, b"Type", b"Pattern")? {
        return Ok(false);
    }
    let pattern_type = resolved_dict_value(doc, &stream.dict, b"PatternType")?
        .as_i64()
        .map_err(|_| ())?;
    Ok(pattern_type == 1)
}

fn dictionary_name_is(
    doc: &lopdf::Document,
    dictionary: &lopdf::Dictionary,
    key: &[u8],
    expected: &[u8],
) -> PdfScanCheck {
    let value = match dictionary.get(key) {
        Ok(value) => doc.dereference(value).map_err(|_| ())?.1,
        Err(_) => return Ok(false),
    };
    Ok(value.as_name().map_err(|_| ())? == expected)
}

fn resolved_dictionary<'a>(
    doc: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Result<&'a lopdf::Dictionary, ()> {
    doc.dereference(object)
        .map_err(|_| ())?
        .1
        .as_dict()
        .map_err(|_| ())
}

fn optional_dictionary<'a>(
    doc: &'a lopdf::Document,
    dictionary: &'a lopdf::Dictionary,
    key: &[u8],
) -> Result<Option<&'a lopdf::Dictionary>, ()> {
    match dictionary.get(key) {
        Ok(object) => resolved_dictionary(doc, object).map(Some),
        Err(_) => Ok(None),
    }
}

fn optional_matrix(
    doc: &lopdf::Document,
    dictionary: &lopdf::Dictionary,
    key: &[u8],
) -> Result<PdfMatrix, ()> {
    match dictionary.get(key) {
        Ok(object) => PdfMatrix::from_array(doc, object),
        Err(_) => Ok(PdfMatrix::IDENTITY),
    }
}

fn resource_entry<'a>(
    doc: &'a lopdf::Document,
    resources: Option<&'a lopdf::Dictionary>,
    category: &[u8],
    name: &[u8],
) -> Result<&'a lopdf::Object, ()> {
    let resources = resources.ok_or(())?;
    let category = resolved_dict_value(doc, resources, category)?
        .as_dict()
        .map_err(|_| ())?;
    category.get(name).map_err(|_| ())
}

fn resolved_stream<'a>(
    doc: &'a lopdf::Document,
    object: &'a lopdf::Object,
) -> Result<(Option<lopdf::ObjectId>, &'a lopdf::Stream), ()> {
    match object {
        lopdf::Object::Reference(object_id) => {
            let object = doc.objects.get(object_id).ok_or(())?;
            let stream = object.as_stream().map_err(|_| ())?;
            Ok((Some(*object_id), stream))
        }
        // Resource streams are required to be indirect.
        lopdf::Object::Stream(_) => Err(()),
        _ => Err(()),
    }
}

fn decode_stream_for_sweep<'a>(
    stream: &'a lopdf::Stream,
    budget: &mut PdfStreamBudget,
) -> Result<Cow<'a, [u8]>, ()> {
    let remaining = budget.remaining();
    if stream.content.len() > remaining {
        return Err(());
    }
    let filters = match stream.dict.get(b"Filter") {
        Ok(_) => stream.filters().map_err(|_| ())?,
        Err(_) => Vec::new(),
    };
    if stream.dict.get(b"DecodeParms").is_ok() {
        // Predictor transforms need their own bounded implementation. Until
        // then an encoded stream that requests one has unknown provenance.
        return Err(());
    }

    let mut bytes = Cow::Borrowed(stream.content.as_slice());
    for filter in filters {
        bytes = Cow::Owned(match filter {
            b"FlateDecode" | b"Fl" => bounded_zlib_decode(&bytes, remaining)?,
            b"ASCII85Decode" | b"A85" => bounded_ascii85_decode(&bytes, remaining)?,
            // Do not fall back to lopdf's unbounded allocation for uncommon or
            // image-specific codecs. Unknown decoding is a fail-closed verdict.
            _ => return Err(()),
        });
    }
    budget.charge(bytes.len())?;
    Ok(bytes)
}

fn bounded_zlib_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, ()> {
    use flate2::read::ZlibDecoder;

    let maximum = u64::try_from(limit)
        .map_err(|_| ())?
        .checked_add(1)
        .ok_or(())?;
    let mut decoder = ZlibDecoder::new(input).take(maximum);
    let mut output = Vec::new();
    decoder.read_to_end(&mut output).map_err(|_| ())?;
    (output.len() <= limit).then_some(output).ok_or(())
}

fn bounded_ascii85_decode(input: &[u8], limit: usize) -> Result<Vec<u8>, ()> {
    let input = input.strip_suffix(b"~>").unwrap_or(input);
    let mut output = Vec::with_capacity(input.len().min(limit));
    let mut buffer = 0u32;
    let mut count = 0usize;

    for &byte in input {
        if byte.is_ascii_whitespace() {
            continue;
        }
        if byte == b'z' {
            if count != 0 || output.len().checked_add(4).is_none_or(|size| size > limit) {
                return Err(());
            }
            output.extend_from_slice(&[0; 4]);
            continue;
        }
        if !(b'!'..=b'u').contains(&byte) {
            return Err(());
        }
        buffer = buffer.checked_mul(85).ok_or(())?;
        buffer = buffer.checked_add(u32::from(byte - b'!')).ok_or(())?;
        count += 1;
        if count == 5 {
            if output.len().checked_add(4).is_none_or(|size| size > limit) {
                return Err(());
            }
            output.extend_from_slice(&buffer.to_be_bytes());
            buffer = 0;
            count = 0;
        }
    }

    if count == 1 {
        return Err(());
    }
    if count > 1 {
        for _ in count..5 {
            buffer = buffer.checked_mul(85).ok_or(())?;
            buffer = buffer.checked_add(84).ok_or(())?;
        }
        let bytes = buffer.to_be_bytes();
        let final_bytes = count - 1;
        if output
            .len()
            .checked_add(final_bytes)
            .is_none_or(|size| size > limit)
        {
            return Err(());
        }
        output.extend_from_slice(&bytes[..final_bytes]);
    }
    Ok(output)
}

#[cfg(test)]
fn pdf_text_origin(doc: &lopdf::Document) -> TextOrigin {
    let verdict = pdf_page_scan_verdict(doc);
    if verdict.page_count > 0 && verdict.machine_read_anchors.len() == verdict.page_count {
        TextOrigin::MachineReadLayer
    } else {
        TextOrigin::AuthorWritten
    }
}

fn resolved_dict_value<'a>(
    doc: &'a lopdf::Document,
    dictionary: &'a lopdf::Dictionary,
    key: &[u8],
) -> Result<&'a lopdf::Object, ()> {
    let value = dictionary.get(key).map_err(|_| ())?;
    doc.dereference(value)
        .map(|(_, value)| value)
        .map_err(|_| ())
}

/// The share of characters a readable text layer can lose to encoding damage.
///
/// Well above any legitimate document -- operative legal text does not run
/// twenty percent replacement characters, controls, and private-use glyphs --
/// and well below the output of a broken `ToUnicode` CMap, which damages most
/// of what it touches. Only the detectable classes are counted: a CMap that
/// maps to the *wrong valid letters* is invisible to any ratio, and that
/// limitation is documented rather than papered over.
const GARBLED_TEXT_RATIO: f64 = 0.2;

/// Below this much text a ratio is noise, and a short document is cheap for
/// the reader to check against the source anyway.
const GARBLED_TEXT_MIN_CHARS: usize = 200;

fn text_layer_is_garbled(blocks: &[ConvertedBlock]) -> bool {
    let mut total = 0usize;
    let mut damaged = 0usize;
    for block in blocks {
        for character in block.text.chars() {
            total += 1;
            let private_use = ('\u{E000}'..='\u{F8FF}').contains(&character)
                || ('\u{F0000}'..='\u{FFFFD}').contains(&character)
                || ('\u{100000}'..='\u{10FFFD}').contains(&character);
            if character == char::REPLACEMENT_CHARACTER
                || (character.is_control() && character != '\n' && character != '\t')
                || private_use
            {
                damaged += 1;
            }
        }
    }
    total >= GARBLED_TEXT_MIN_CHARS && (damaged as f64) / (total as f64) > GARBLED_TEXT_RATIO
}

fn convert_pdf(bytes: &[u8]) -> Result<ConvertedDocument, ConversionError> {
    let doc =
        pdf_extract::Document::load_mem(bytes).map_err(|_| ConversionError::MalformedSource)?;
    let mut output = LayoutOutput::default();
    pdf_extract::output_doc(&doc, &mut output).map_err(|_| ConversionError::MalformedSource)?;
    let mut blocks = Vec::new();
    let mut output_bytes = 0usize;
    for page in output.pages {
        let lines = page.lines();
        for (index, line) in lines.iter().enumerate() {
            output_bytes = output_bytes
                .checked_add(line.text.len())
                .and_then(|value| value.checked_add(1))
                .ok_or(ConversionError::OutputBudgetExceeded)?;
            if output_bytes > MAX_OUTPUT_BYTES || blocks.len() >= MAX_BLOCKS {
                return Err(ConversionError::OutputBudgetExceeded);
            }
            blocks.push(ConvertedBlock {
                source_anchor: format!("page:{:04}", page.number),
                text: line.text.clone(),
                flow: if index + 1 == lines.len() {
                    AnchorFlow::HardBoundary
                } else {
                    AnchorFlow::Continue
                },
                is_heading: line.is_heading.then_some(true),
            });
        }
    }
    let scan_verdict = pdf_page_scan_verdict(&doc);
    let text_origin = if scan_verdict.page_count > 0
        && scan_verdict.machine_read_anchors.len() == scan_verdict.page_count
    {
        TextOrigin::MachineReadLayer
    } else {
        TextOrigin::AuthorWritten
    };
    let machine_read_anchors = scan_verdict.machine_read_anchors;
    // A text layer damaged past reading is not a text layer. Handing the
    // blocks over anyway would quote mojibake as the exact language of the
    // source; reporting the file as needing OCR is true -- the pages are
    // there, the recogniser can read them -- and keeps the gap counted.
    if !blocks.is_empty() && text_layer_is_garbled(&blocks) {
        return Ok(ConvertedDocument {
            format: SourceFormat::Pdf,
            blocks: Vec::new(),
            warnings: vec!["ocr_required_or_no_extractable_text".to_string()],
            text_origin,
            machine_read_anchors,
        });
    }
    let warnings = if blocks.is_empty() {
        vec!["ocr_required_or_no_extractable_text".to_string()]
    } else {
        // Every PDF is withheld from same-clause answers, not only those with
        // no visible structure.
        //
        // Two rules were tried before this one and both were shown wrong by a
        // reviewer's document. Trusting a file because it contained a heading
        // let one administrative line vouch for two unrelated clauses.
        // Confining the claim to a paragraph instead assumed paragraph breaks
        // are reliably visible, and they are not: a PDF set at ordinary line
        // spacing reported starts of `true, true, false` and put two labelled
        // clauses in one span.
        //
        // A caption marks where a clause starts and never where it ends, and
        // nothing in a PDF marks the end. There is no measurement of the page
        // that closes that gap, so the claim is not made. Everything else about
        // PDFs still works -- exact phrase, whole-document search, excerpts and
        // anchors -- and caption detection still improves how provisions are
        // titled and cited. What it no longer does is license a statement about
        // two terms sharing a clause.
        vec![PDF_UNSUPPORTED_STRUCTURE_WARNING.to_string()]
    };
    Ok(ConvertedDocument {
        format: SourceFormat::Pdf,
        blocks,
        warnings,
        text_origin,
        machine_read_anchors,
    })
}

#[derive(Debug, Default)]
struct LayoutOutput {
    pages: Vec<LayoutPage>,
    current: Option<LayoutPage>,
}

#[derive(Debug)]
struct LayoutPage {
    number: u32,
    height: f64,
    glyphs: Vec<LayoutGlyph>,
}

#[derive(Debug)]
struct LayoutGlyph {
    x: f64,
    y: f64,
    end_x: f64,
    size: f64,
    text: String,
}

#[derive(Debug)]
struct LayoutLine {
    text: String,
    is_heading: bool,
}

impl LayoutPage {
    fn lines(&self) -> Vec<LayoutLine> {
        let mut glyphs = self.glyphs.iter().collect::<Vec<_>>();
        glyphs.sort_by(|left, right| left.y.total_cmp(&right.y).then(left.x.total_cmp(&right.x)));
        let mut lines: Vec<Vec<&LayoutGlyph>> = Vec::new();
        for glyph in glyphs {
            let tolerance = glyph.size.max(1.0) * 0.45;
            if let Some(line) = lines.last_mut() {
                if (line[0].y - glyph.y).abs() <= tolerance {
                    line.push(glyph);
                    continue;
                }
            }
            lines.push(vec![glyph]);
        }
        let mut raw = lines
            .into_iter()
            .map(|mut glyphs| {
                glyphs.sort_by(|left, right| left.x.total_cmp(&right.x));
                let mut text = String::new();
                let mut last_end = None;
                let mut size: f64 = 0.0;
                for glyph in &glyphs {
                    if let Some(end) = last_end {
                        if glyph.x > end + glyph.size * 0.1 && !text.is_empty() {
                            text.push(' ');
                        }
                    }
                    text.push_str(&glyph.text);
                    last_end = Some(glyph.end_x);
                    size = size.max(glyph.size);
                }
                let y = glyphs_y(&glyphs);
                (text.trim().to_string(), y, size)
            })
            .filter(|(text, _, _)| !text.is_empty())
            .collect::<Vec<_>>();

        // A largest horizontal gap is a conservative column separator. It is
        // only used when both sides contain a substantial amount of text;
        // ordinary paragraph indentation must not reorder a one-column page.
        raw.sort_by(|left, right| left.1.total_cmp(&right.1));
        let mut gaps = self.glyphs.iter().map(|glyph| glyph.x).collect::<Vec<_>>();
        gaps.sort_by(f64::total_cmp);
        let _column_split = gaps
            .windows(2)
            .max_by(|left, right| (left[1] - left[0]).total_cmp(&(right[1] - right[0])))
            .filter(|gap| gap[1] - gap[0] > self.glyphs.first().map_or(0.0, |g| g.size * 12.0));

        let sizes = raw.iter().map(|(_, _, size)| *size).collect::<Vec<_>>();
        let reference_size = median(sizes).unwrap_or(0.0);
        // Line spacing is a property of the type, not of the document's
        // paragraph habits: a set line sits at roughly 1.1-1.5x its font size
        // and a paragraph break is wider. Deriving the threshold from the
        // median gap assumed most gaps were within-paragraph line spacing,
        // which is false for any document whose paragraphs are mostly one line
        // -- there the median IS the paragraph gap, the threshold lands above
        // every gap in the document, and no break can ever be detected. That
        // is the whole reason uniformly formatted contracts reported no
        // structure at all.
        //
        // Honest limit of the evidence: with the conditions below in place,
        // widening this multiplier all the way down to 0.1 does not change the
        // output of any fixture, so the exact value is not what separates a
        // caption from body text -- the word-level tests and the "introduces
        // prose" rule are. It is kept because it states the real requirement,
        // that a caption begins a paragraph, and because the shape that would
        // exercise it (a short title-case line mid-paragraph, followed by a
        // full line of prose) is one no fixture here contains.
        //
        // A floor derived from the smallest observed gap was tried here, to
        // hold the threshold above the leading of a loosely set document. It
        // was removed: every fixture produced identical output with and
        // without it, because the "introduces prose" rule below already
        // rejects what it was guarding against, and an untested branch that
        // reads as a safeguard is worse than no branch.
        let paragraph_break_threshold = reference_size * 1.6;
        // A caption introduces something. That is what separates it from the
        // other short title-case paragraph in legal documents -- a signature
        // block line like "By: Jane Ellis" or a party name -- which is
        // followed by another short line rather than by prose. Gap alone
        // cannot tell them apart: in a uniformly formatted contract the gap
        // before a caption and the gap before a body paragraph are the same
        // number, so a rule that reads only the gap marks the signature block
        // as a run of headings and splits it into fabricated provisions.
        let is_short_line = |text: &str| text.split_whitespace().count() <= 8;
        let followed_by_prose = raw
            .iter()
            .enumerate()
            .map(|(index, _)| {
                raw.get(index + 1)
                    .is_some_and(|(next, _, _)| !is_short_line(next))
            })
            .collect::<Vec<_>>();
        let mut previous_y = None;
        raw.into_iter()
            .enumerate()
            .map(|(index, (text, y, size))| {
                // The first line on a page is deliberately NOT treated as a
                // paragraph start. It has no preceding line, so it looks like
                // the strongest possible break -- but a running header sits in
                // exactly that position on every page, and admitting it marks
                // furniture as a caption. Tried and reverted: it severed a
                // carve-out from the cap it limits, which is the fabricated
                // boundary this whole area exists to prevent. The cost is that
                // a caption at the top of a page is not flagged here; page
                // ends are already hard boundaries, so provisions still do not
                // run together across the break.
                let leading_gap = previous_y.map_or(0.0, |previous| y - previous);
                previous_y = Some(y);
                let geometric_heading = leading_gap > paragraph_break_threshold
                    && followed_by_prose[index]
                    && is_short_line(&text)
                    && text.split_whitespace().all(|word| {
                        matches!(
                            word.to_ascii_lowercase().as_str(),
                            "and" | "of" | "the" | "to" | "in" | "for" | "a" | "or"
                        ) || word.chars().next().is_some_and(char::is_uppercase)
                    })
                    && !matches!(text.chars().next_back(), Some('.') | Some(';') | Some(':'));
                LayoutLine {
                    is_heading: size > reference_size * 1.15 || geometric_heading,
                    text,
                }
            })
            .collect()
    }
}

fn glyphs_y(glyphs: &[&LayoutGlyph]) -> f64 {
    glyphs.first().map_or(0.0, |glyph| glyph.y)
}

fn median(mut values: Vec<f64>) -> Option<f64> {
    values.retain(|value| value.is_finite() && *value > 0.0);
    values.sort_by(f64::total_cmp);
    values.get(values.len() / 2).copied()
}

impl pdf_extract::OutputDev for LayoutOutput {
    fn begin_page(
        &mut self,
        page_num: u32,
        media_box: &pdf_extract::MediaBox,
        _: Option<(f64, f64, f64, f64)>,
    ) -> Result<(), pdf_extract::OutputError> {
        self.current = Some(LayoutPage {
            number: page_num,
            height: media_box.ury - media_box.lly,
            glyphs: Vec::new(),
        });
        Ok(())
    }

    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        if let Some(page) = self.current.take() {
            self.pages.push(page);
        }
        Ok(())
    }

    fn output_character(
        &mut self,
        trm: &pdf_extract::Transform,
        width: f64,
        _: f64,
        font_size: f64,
        character: &str,
    ) -> Result<(), pdf_extract::OutputError> {
        let page = self
            .current
            .as_mut()
            .ok_or(pdf_extract::OutputError::FormatError(std::fmt::Error))?;
        let position = trm.post_transform(&pdf_extract::Transform::row_major(
            1.0,
            0.0,
            0.0,
            -1.0,
            0.0,
            page.height,
        ));
        let scale_x = (trm.m11 * trm.m11 + trm.m21 * trm.m21).sqrt();
        let scale_y = (trm.m12 * trm.m12 + trm.m22 * trm.m22).sqrt();
        let size = (font_size * scale_x * font_size * scale_y).sqrt().abs();
        page.glyphs.push(LayoutGlyph {
            x: position.m31,
            y: position.m32,
            end_x: position.m31 + width * size,
            size,
            text: character.to_string(),
        });
        Ok(())
    }

    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
}

fn convert_docx(bytes: &[u8]) -> Result<ConvertedDocument, ConversionError> {
    let cursor = Cursor::new(bytes);
    let mut archive = ZipArchive::new(cursor).map_err(|_| ConversionError::MalformedSource)?;
    if archive.len() > MAX_DOCX_ENTRIES {
        return Err(ConversionError::InputBudgetExceeded);
    }
    if archive.decompressed_size().is_some_and(|size| {
        size > MAX_OUTPUT_BYTES as u128 || size > MAX_DOCX_XML_BYTES as u128 * 4
    }) {
        return Err(ConversionError::OutputBudgetExceeded);
    }
    if archive
        .has_overlapping_files()
        .map_err(|_| ConversionError::MalformedSource)?
    {
        return Err(ConversionError::MalformedSource);
    }
    let document_xml = archive
        .by_name("word/document.xml")
        .map_err(|_| ConversionError::MalformedSource)?;
    if document_xml.size() > MAX_DOCX_XML_BYTES as u64 {
        return Err(ConversionError::OutputBudgetExceeded);
    }
    let mut xml = Vec::new();
    document_xml
        .take((MAX_DOCX_XML_BYTES as u64).saturating_add(1))
        .read_to_end(&mut xml)
        .map_err(|_| ConversionError::MalformedSource)?;
    if xml.len() > MAX_DOCX_XML_BYTES {
        return Err(ConversionError::OutputBudgetExceeded);
    }
    docx_paragraphs(&xml)
}

fn docx_paragraphs(xml: &[u8]) -> Result<ConvertedDocument, ConversionError> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut paragraphs = Vec::new();
    let mut paragraph = String::new();
    let mut paragraph_ordinal: usize = 0;
    let mut in_text = false;
    let mut output_bytes = 0usize;
    // Structural signal for the paragraph currently being assembled.
    let mut heading_style = false;
    let mut saw_style = false;
    // `w:pPrChange`/`w:rPrChange` record the properties a tracked change
    // replaced. Reading them let a revision record override the live style --
    // a real Heading1 whose change-record said Normal came out as body, and
    // the reverse. Paragraph-mark formatting (`w:pPr>w:rPr`) needs no special
    // case: size is weighted by the characters set in it and a pilcrow
    // contributes none.
    let mut skip_depth = 0usize;
    // Parallel to `paragraphs`: whether the file named a heading style.
    let mut formatting: Vec<bool> = Vec::new();

    loop {
        match reader.read_event_into(&mut buffer) {
            Ok(Event::Start(event)) => {
                let name = event.name();
                let local = local_name(name.as_ref());
                // Count every element while inside a change record, and
                // decrement on every close below. Decrementing only for the
                // record's own name left the counter stuck above zero for the
                // rest of the paragraph, silently suppressing the live style.
                if skip_depth > 0 || matches!(local, "pPrChange" | "rPrChange") {
                    skip_depth += 1;
                }
                match local {
                    "t" if skip_depth == 0 => in_text = true,
                    "pStyle" if skip_depth == 0 => {
                        if let Some(value) = attribute_value(&event, "val") {
                            saw_style = true;
                            heading_style = is_heading_style(&value);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => match local_name(event.name().as_ref()) {
                // Run and paragraph properties are usually self-closing.
                "pStyle" if skip_depth == 0 => {
                    if let Some(value) = attribute_value(&event, "val") {
                        saw_style = true;
                        heading_style = is_heading_style(&value);
                    }
                }
                "tab" => paragraph.push('\t'),
                "br" | "cr" => paragraph.push('\n'),
                // `<w:p/>` is a self-closing empty paragraph and arrives as
                // Empty rather than Start/End. Word emits these constantly as
                // spacers, and each one still occupies a paragraph position
                // in the document a reader is asked to navigate to.
                "p" => paragraph_ordinal += 1,
                _ => {}
            },
            Ok(Event::Text(event)) if in_text && skip_depth == 0 => {
                paragraph.push_str(&event);
            }
            Ok(Event::GeneralRef(reference)) if in_text && skip_depth == 0 => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|_| ConversionError::MalformedSource)?
                {
                    paragraph.push(character);
                } else {
                    let value = quick_xml::escape::resolve_xml_entity(&reference)
                        .ok_or(ConversionError::MalformedSource)?;
                    paragraph.push_str(value);
                }
            }
            // Inside a tracked-change record: count every close so the
            // counter returns to zero. Decrementing only on the record's own
            // name left it stuck above zero for the rest of the paragraph,
            // silently suppressing the live style and size.
            Ok(Event::End(event)) if skip_depth > 0 => {
                skip_depth -= 1;
                if local_name(event.name().as_ref()) == "p" {
                    skip_depth = 0;
                }
            }
            Ok(Event::End(event)) => match local_name(event.name().as_ref()) {
                "t" => in_text = false,
                "p" => {
                    let paragraph_style = heading_style;
                    let paragraph_saw_style = saw_style;
                    heading_style = false;
                    saw_style = false;
                    skip_depth = 0;
                    // Count every <w:p> element, including empty spacers and
                    // paragraphs inside tables. The anchor previously used
                    // the number of paragraphs emitted so far, so any dropped
                    // empty paragraph shifted it: "paragraph:000003" did not
                    // locate the third paragraph in Word, and the drift grew
                    // monotonically through the document. A lawyer asked to
                    // verify a quote at that anchor lands somewhere else.
                    paragraph_ordinal += 1;
                    let text = normalize_extracted_text(&paragraph);
                    paragraph.clear();
                    if !text.is_empty() {
                        output_bytes = output_bytes
                            .checked_add(text.len())
                            .ok_or(ConversionError::OutputBudgetExceeded)?;
                        if output_bytes > MAX_OUTPUT_BYTES || paragraphs.len() >= MAX_BLOCKS {
                            return Err(ConversionError::OutputBudgetExceeded);
                        }
                        formatting.push(paragraph_style && paragraph_saw_style);
                        paragraphs.push(ConvertedBlock {
                            source_anchor: format!("paragraph:{paragraph_ordinal:06}"),
                            text,
                            flow: AnchorFlow::Continue,
                            is_heading: Some(paragraph_style),
                            // Deliberately None, though `w:p` does mark every
                            // paragraph. This flag sets the unit within which
                            // retrieval will allow a same-clause claim, and for
                            // DOCX that unit is the whole provision: `w:pStyle`
                            // declares where clauses begin, so a conjunction
                            // spanning two paragraphs of one clause is a real
                            // conjunction. Reporting paragraphs here would
                            // narrow DOCX to single paragraphs and refuse
                            // answers the file supports.
                        });
                    }
                }
                _ => {}
            },
            Ok(Event::DocType(_)) => return Err(ConversionError::MalformedSource),
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Err(ConversionError::MalformedSource),
        }
        buffer.clear();
    }
    // A named heading style is the only verdict. A relative-size rule was
    // measured against a real corpus and bought nothing -- every real
    // improvement came from `pStyle` alone, and the real Business Associate
    // Agreement's twenty-one captions are body-sized and found by the lexical
    // rule -- while causing every regression across five rounds. Size is
    // layout, not structure: a statutory conspicuous-type notice is set
    // larger because a statute demands it, and a twelve-point front page over
    // a ten-point back page is an order form over standard terms. Both are
    // operative text, and promoting them severs a clause from its caption.
    //
    // `None` means the file did not say, and the lexical fallback runs.
    for (block, styled_heading) in paragraphs.iter_mut().zip(formatting) {
        block.is_heading = if styled_heading { Some(true) } else { None };
    }

    Ok(ConvertedDocument {
        format: SourceFormat::Docx,
        blocks: paragraphs,
        warnings: Vec::new(),
        text_origin: TextOrigin::AuthorWritten,
        machine_read_anchors: BTreeSet::new(),
    })
}

/// Attribute value by local name, ignoring namespace prefix.
fn attribute_value(event: &quick_xml::events::BytesStart<'_>, wanted: &str) -> Option<String> {
    event.attributes().flatten().find_map(|attribute| {
        (local_name(attribute.key.as_ref()) == wanted).then(|| attribute.value.into_owned())
    })
}

/// Whether a `w:pStyle` value names one of Word's heading styles.
fn is_heading_style(value: &str) -> bool {
    // Exact identifiers only. A prefix match claimed `Subtitle`,
    // `HeadingNote`, `TitlePage` and `HeadingBase` -- and Word templates put
    // the preamble and recitals under `Subtitle`, which is operative text. A
    // style verdict is unconditional and the lexical fallback cannot recover
    // from it, so it has to be narrow.
    let lowered = value.to_ascii_lowercase();
    lowered == "title"
        || lowered
            .strip_prefix("heading")
            .is_some_and(|rest| rest.is_empty() || rest.chars().all(|c| c.is_ascii_digit()))
}

fn local_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn normalize_extracted_text(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .lines()
        .map(str::trim)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

pub fn run_worker_process(format: &str) -> i32 {
    if install_worker_security_boundary().is_err() {
        return 70;
    }
    if format == "sandbox-self-test" {
        return sandbox_self_test();
    }
    let format = match SourceFormat::parse(format) {
        Ok(format) => format,
        Err(_) => return 64,
    };
    let response = std::panic::catch_unwind(|| {
        let mut stdin = std::io::stdin().lock();
        let bytes = read_worker_input(&mut stdin)?;
        convert_bytes(format, &bytes)
    });
    let response = match response {
        Ok(Ok(document)) => WorkerResponse {
            document: Some(document),
            error: None,
        },
        Ok(Err(error)) => WorkerResponse {
            document: None,
            error: Some(error.to_string()),
        },
        Err(_) => WorkerResponse {
            document: None,
            error: Some("the source could not be converted".to_string()),
        },
    };
    let output = match serde_json::to_vec(&response) {
        Ok(output) if output.len() <= MAX_OUTPUT_BYTES => output,
        _ => return 74,
    };
    let mut stdout = std::io::stdout().lock();
    if stdout.write_all(&output).is_err() || stdout.flush().is_err() {
        return 74;
    }
    if response.document.is_some() {
        0
    } else {
        65
    }
}

fn sandbox_self_test() -> i32 {
    let network_denied = std::net::TcpListener::bind("127.0.0.1:0").is_err()
        && std::net::TcpStream::connect("127.0.0.1:1").is_err();
    // Probe paths this profile never names, the way the semantic worker's
    // test does. Reading /etc/passwd alone was a weak canary: a regression to
    // `(allow default)` plus a single deny for that literal would have passed
    // it while leaving the whole filesystem readable and writable. That is the
    // exact bug already found and fixed in the semantic worker; this test did
    // not get the same treatment until an independent reviewer said so.
    //
    // This profile is `(deny default)` with no filesystem allowance at all, so
    // every one of these must fail.
    let unnamed_read_denied = std::fs::read("/private/etc/hosts").is_err()
        && std::fs::read_dir("/Applications").is_err()
        && std::fs::read_dir("/Library").is_err()
        && std::fs::read_dir("/usr/share").is_err();
    // A converter that could write would be a place to park document bytes.
    let write_denied = ["/private/tmp", "/private/var/tmp"]
        .iter()
        .all(|directory| {
            let probe = std::path::Path::new(directory).join("minutes-archive-convert-probe");
            let denied = std::fs::write(&probe, b"probe").is_err();
            if !denied {
                let _ = std::fs::remove_file(&probe);
            }
            denied
        });
    if network_denied && unnamed_read_denied && write_denied {
        0
    } else {
        71
    }
}

fn read_worker_input(reader: &mut impl Read) -> Result<Vec<u8>, ConversionError> {
    let mut length_bytes = [0u8; 8];
    reader
        .read_exact(&mut length_bytes)
        .map_err(|_| ConversionError::MalformedSource)?;
    let length = usize::try_from(u64::from_le_bytes(length_bytes))
        .map_err(|_| ConversionError::InputBudgetExceeded)?;
    if length == 0 || length > MAX_SOURCE_BYTES {
        return Err(ConversionError::InputBudgetExceeded);
    }
    let mut bytes = vec![0u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| ConversionError::MalformedSource)?;
    let mut trailing = [0u8; 1];
    if reader
        .read(&mut trailing)
        .map_err(|_| ConversionError::MalformedSource)?
        != 0
    {
        return Err(ConversionError::MalformedSource);
    }
    Ok(bytes)
}

fn install_worker_security_boundary() -> Result<(), ConversionError> {
    install_resource_limits()?;
    install_platform_sandbox()
}

#[cfg(unix)]
fn install_resource_limits() -> Result<(), ConversionError> {
    let cpu = libc::rlimit {
        rlim_cur: WORKER_CPU_SECONDS,
        rlim_max: WORKER_CPU_SECONDS,
    };
    let file_size = libc::rlimit {
        rlim_cur: MAX_OUTPUT_BYTES as u64,
        rlim_max: MAX_OUTPUT_BYTES as u64,
    };
    let open_files = libc::rlimit {
        rlim_cur: 16,
        rlim_max: 16,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &file_size) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &open_files) } != 0
    {
        return Err(ConversionError::SecurityBoundaryUnavailable);
    }
    install_address_space_limit()
}

#[cfg(not(unix))]
fn install_resource_limits() -> Result<(), ConversionError> {
    Err(ConversionError::SecurityBoundaryUnavailable)
}

#[cfg(target_os = "macos")]
fn install_address_space_limit() -> Result<(), ConversionError> {
    use mach2::kern_return::KERN_SUCCESS;
    use mach2::task::task_info;
    use mach2::task_info::{
        task_basic_info_64, task_info_t, TASK_BASIC_INFO_64, TASK_BASIC_INFO_64_COUNT,
    };
    use mach2::traps::mach_task_self;

    let mut info = task_basic_info_64::default();
    let mut count = TASK_BASIC_INFO_64_COUNT;
    let status = unsafe {
        task_info(
            mach_task_self(),
            TASK_BASIC_INFO_64,
            (&mut info as *mut task_basic_info_64).cast::<libc::c_int>() as task_info_t,
            &mut count,
        )
    };
    if status != KERN_SUCCESS || count != TASK_BASIC_INFO_64_COUNT {
        return Err(ConversionError::SecurityBoundaryUnavailable);
    }
    let limit = info
        .virtual_size
        .checked_add(WORKER_MEMORY_GROWTH_BYTES)
        .ok_or(ConversionError::SecurityBoundaryUnavailable)?;
    let address_space = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0 {
        return Err(ConversionError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_address_space_limit() -> Result<(), ConversionError> {
    let address_space = libc::rlimit {
        rlim_cur: 2 * 1024 * 1024 * 1024,
        rlim_max: 2 * 1024 * 1024 * 1024,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0 {
        return Err(ConversionError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_platform_sandbox() -> Result<(), ConversionError> {
    use std::ffi::{c_char, c_int, CStr};
    use std::ptr;

    #[link(name = "System")]
    unsafe extern "C" {
        fn sandbox_init(
            profile: *const c_char,
            flags: u64,
            error_buffer: *mut *mut c_char,
        ) -> c_int;
        fn sandbox_free_error(error_buffer: *mut c_char);
    }

    const PROFILE: &CStr = c"(version 1)
(deny default)
(allow process-info*)
(allow sysctl-read)
(allow file-read-data (subpath \"/dev/fd\"))
(allow file-write-data (subpath \"/dev/fd\"))
";
    let mut error_buffer = ptr::null_mut();
    let status = unsafe { sandbox_init(PROFILE.as_ptr(), 0, &mut error_buffer) };
    if !error_buffer.is_null() {
        unsafe { sandbox_free_error(error_buffer) };
    }
    if status != 0 {
        return Err(ConversionError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_platform_sandbox() -> Result<(), ConversionError> {
    Err(ConversionError::SecurityBoundaryUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn synthetic_docx(document_xml: &str) -> Vec<u8> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = ZipWriter::new(&mut cursor);
            writer
                .start_file(
                    "word/document.xml",
                    SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .expect("document entry");
            writer.write_all(document_xml.as_bytes()).expect("xml");
            writer.finish().expect("zip");
        }
        cursor.seek(SeekFrom::Start(0)).expect("rewind");
        cursor.into_inner()
    }

    fn assemble_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
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

    fn pdf_stream(dictionary_entries: &str, content: Vec<u8>) -> Vec<u8> {
        [
            format!(
                "<< {dictionary_entries} /Length {} >>\nstream\n",
                content.len()
            )
            .into_bytes(),
            content,
            b"\nendstream".to_vec(),
        ]
        .concat()
    }

    fn image_xobject(
        width: usize,
        height: usize,
        extra_dictionary_entries: &str,
        content: Vec<u8>,
    ) -> Vec<u8> {
        pdf_stream(
            &format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} \
                 /ColorSpace /DeviceGray /BitsPerComponent 8 {extra_dictionary_entries}"
            ),
            content,
        )
    }

    fn grayscale_image_xobject(width: usize, height: usize) -> Vec<u8> {
        let pixels = vec![0x80; width.checked_mul(height).expect("fixture dimensions")];
        image_xobject(width, height, "", pixels)
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
        assemble_pdf(&objects)
    }

    fn synthetic_typed_pdf_with_image(image: Vec<u8>) -> Vec<u8> {
        let stream = b"BT /F1 12 Tf 72 720 Td (Readable typed page text.) Tj ET q 144 0 0 36 72 700 cm /Im1 Do Q";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", stream.to_vec()),
            image,
        ];
        assemble_pdf(&objects)
    }

    /// Build a page with optional text and a page-sized image. `Some(true)`
    /// paints the image before the text; `Some(false)` paints it afterward;
    /// `None` omits it. The two orders deliberately receive the same verdict.
    fn synthetic_copier_pdf(
        render_mode: &str,
        image_before_text: Option<bool>,
        with_text: bool,
    ) -> Vec<u8> {
        let image_draw = if image_before_text.is_some() {
            "q 612 0 0 792 0 0 cm /Im1 Do Q "
        } else {
            ""
        };
        let text_draw = if with_text {
            format!(
                "BT /F1 12 Tf {render_mode}72 720 Td (7. CONFIDENTIALITY) Tj 0 -20 Td (Confidential Information includes affiliate data.) Tj ET "
            )
        } else {
            String::new()
        };
        let stream = if image_before_text == Some(true) {
            format!("{image_draw}{text_draw}")
        } else {
            format!("{text_draw}{image_draw}")
        }
        .into_bytes();
        let resources = if image_before_text.is_some() {
            "/Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R >> >>"
        } else {
            "/Resources << /Font << /F1 4 0 R >> >>"
        };
        let mut objects = vec![
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] {resources} /Contents 5 0 R >>"
            )
            .into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            [
                format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes(),
                stream,
                b"\nendstream".to_vec(),
            ]
            .concat(),
        ];
        if image_before_text.is_some() {
            // A real, raw 8-bit grayscale raster at 150dpi letter dimensions.
            objects.push(grayscale_image_xobject(1275, 1650));
        }
        assemble_pdf(&objects)
    }

    fn synthetic_raster_pdf_on_page(
        page_width: i64,
        page_height: i64,
        image_width: i64,
        image_height: i64,
    ) -> Vec<u8> {
        let stream = format!(
            "BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET q {page_width} 0 0 {page_height} 0 0 cm /Im1 Do Q"
        )
        .into_bytes();
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {page_width} {page_height}] \
                 /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R >> >> \
                 /Contents 5 0 R >>"
            )
            .into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            [
                format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes(),
                stream,
                b"\nendstream".to_vec(),
            ]
            .concat(),
            grayscale_image_xobject(
                usize::try_from(image_width).expect("positive fixture width"),
                usize::try_from(image_height).expect("positive fixture height"),
            ),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_raster_pdf(width: i64, height: i64) -> Vec<u8> {
        synthetic_raster_pdf_on_page(612, 792, width, height)
    }

    fn synthetic_raster_pdf_with_draw(width: usize, height: usize, draw: &str) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET {draw}");
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", stream.into_bytes()),
            grayscale_image_xobject(width, height),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_raster_pdf_with_page_boxes(
        pages_boxes: &str,
        page_boxes: &str,
        draw: &str,
    ) -> Vec<u8> {
        let stream = format!("BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET {draw}");
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            format!("<< /Type /Pages /Kids [3 0 R] /Count 1 {pages_boxes} >>").into_bytes(),
            format!(
                "<< /Type /Page /Parent 2 0 R {page_boxes} /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R >> >> /Contents 5 0 R >>"
            )
            .into_bytes(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", stream.into_bytes()),
            grayscale_image_xobject(1000, 1300),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_form_with_small_bbox_pdf() -> Vec<u8> {
        let page_stream = b"BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET /Fm1 Do";
        let form_stream = b"q 612 0 0 792 0 0 cm /Im1 Do Q";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", page_stream.to_vec()),
            pdf_stream(
                "/Type /XObject /Subtype /Form /BBox [72 72 144 144] /Resources << /XObject << /Im1 7 0 R >> >>",
                form_stream.to_vec(),
            ),
            grayscale_image_xobject(1000, 1300),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_split_stream_clip_pdf() -> Vec<u8> {
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 7 0 R >> >> /Contents [5 0 R 6 0 R] >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream(
                "",
                b"BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET 72 72 72 72 re W n"
                    .to_vec(),
            ),
            pdf_stream("", b"612 0 0 792 0 0 cm /Im1 Do".to_vec()),
            grayscale_image_xobject(1000, 1300),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_four_strip_scan_pdf() -> Vec<u8> {
        let stream = b"BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET \
            q 612 0 0 198 0 0 cm /Im1 Do Q \
            q 612 0 0 198 0 198 cm /Im2 Do Q \
            q 612 0 0 198 0 396 cm /Im3 Do Q \
            q 612 0 0 198 0 594 cm /Im4 Do Q";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Im1 6 0 R /Im2 7 0 R /Im3 8 0 R /Im4 9 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", stream.to_vec()),
            grayscale_image_xobject(1275, 330),
            grayscale_image_xobject(1275, 330),
            grayscale_image_xobject(1275, 330),
            grayscale_image_xobject(1275, 330),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_two_page_raster_pdf(width: usize, height: usize) -> Vec<u8> {
        let first_page = b"BT /F1 12 Tf 72 720 Td (Readable first page text.) Tj ET";
        let second_page = b"BT /F1 12 Tf 72 720 Td (Readable second page text.) Tj ET q 612 0 0 792 0 0 cm /Im1 Do Q";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> >> /Contents 6 0 R >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 5 0 R >> /XObject << /Im1 8 0 R >> >> /Contents 7 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", first_page.to_vec()),
            pdf_stream("", second_page.to_vec()),
            grayscale_image_xobject(width, height),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_extgstate_soft_mask_pdf() -> Vec<u8> {
        let page_stream = b"BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET /GS1 gs";
        let mask_stream = b"q 612 0 0 792 0 0 cm /Im1 Do Q";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /ExtGState << /GS1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", page_stream.to_vec()),
            b"<< /Type /ExtGState /SMask << /S /Luminosity /G 7 0 R >> >>".to_vec(),
            pdf_stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Group << /S /Transparency /CS /DeviceGray >> /Resources << /XObject << /Im1 8 0 R >> >>",
                mask_stream.to_vec(),
            ),
            grayscale_image_xobject(1275, 1650),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_inline_image_in_form_pdf() -> Vec<u8> {
        let page_stream = b"BT /F1 12 Tf 72 720 Td (Readable page text.) Tj ET /Fm1 Do";
        let mut form_stream =
            b"q 612 0 0 792 0 0 cm BI /W 1275 /H 1650 /CS /Gray /BPC 8 ID\n".to_vec();
        form_stream.extend(std::iter::repeat_n(0x80, 1275 * 1650));
        form_stream.extend_from_slice(b"\nEI Q\n");
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> /XObject << /Fm1 6 0 R >> >> /Contents 5 0 R >>".to_vec(),
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
            pdf_stream("", page_stream.to_vec()),
            pdf_stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << >>",
                form_stream,
            ),
        ];
        assemble_pdf(&objects)
    }

    fn inline_scan_content() -> Vec<u8> {
        let mut content = b"q 612 0 0 792 0 0 cm BI /W 500 /H 647 /CS /Gray /BPC 8 ID\n".to_vec();
        content.extend(std::iter::repeat_n(0x80, 500 * 647));
        content.extend_from_slice(b"\nEI Q\n");
        content
    }

    fn synthetic_inline_image_in_tiling_pattern_pdf() -> Vec<u8> {
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Pattern << /P1 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            pdf_stream("", b"/Pattern cs /P1 scn".to_vec()),
            pdf_stream(
                "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 612 792] /XStep 612 /YStep 792 /Resources << >>",
                inline_scan_content(),
            ),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_inline_image_in_annotation_appearance_pdf() -> Vec<u8> {
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Annots [5 0 R] >>"
                .to_vec(),
            pdf_stream("", b"q Q".to_vec()),
            b"<< /Type /Annot /Subtype /Stamp /Rect [0 0 612 792] /AP << /N 6 0 R >> >>"
                .to_vec(),
            // Deliberately omit `/Subtype /Form` so this exercises the `/AP`
            // root rather than the independently recognized Form path.
            pdf_stream("/BBox [0 0 612 792] /Resources << >>", inline_scan_content()),
        ];
        assemble_pdf(&objects)
    }

    fn synthetic_inline_image_in_type3_glyph_pdf() -> Vec<u8> {
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F3 5 0 R >> >> /Contents 4 0 R >>"
                .to_vec(),
            pdf_stream("", b"BT /F3 1 Tf (A) Tj ET".to_vec()),
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 612 792] /FontMatrix [1 0 0 1 0 0] /CharProcs << /A 6 0 R >> >>"
                .to_vec(),
            pdf_stream("", inline_scan_content()),
        ];
        assemble_pdf(&objects)
    }

    #[test]
    fn docx_conversion_preserves_paragraph_anchors_and_text() {
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
            <w:p><w:r><w:t>Confidential Information &amp; affiliate data.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].source_anchor, "paragraph:000001");
        assert_eq!(
            document.blocks[1].text,
            "Confidential Information & affiliate data."
        );
    }

    #[test]
    fn uniform_sizing_reports_no_signal_so_the_fallback_survives() {
        // The shape that regressed five fixtures: every paragraph one size,
        // captions set apart by bold or caps rather than by size. This is the
        // standard legal template. Reporting `Some(false)` here was a
        // positive claim of body text that suppressed the lexical fallback,
        // and a real Business Associate Agreement collapsed from 21
        // provisions to 2 -- "find the indemnification provision" went from
        // one correct card to none.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:rPr><w:sz w:val="22"/><w:b/></w:rPr><w:t>14. Indemnification</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="22"/></w:rPr><w:t>Business Associate shall indemnify Covered Entity.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="22"/><w:b/></w:rPr><w:t>13. Term; Termination; Survival</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="22"/></w:rPr><w:t>These obligations survive termination.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        for block in &document.blocks {
            assert_eq!(
                block.is_heading, None,
                "uniform sizing does not distinguish {:?}; claiming a verdict \
                 here suppresses the only mechanism that segments these files",
                block.text
            );
        }
    }

    #[test]
    fn a_paragraph_mark_size_does_not_leak_into_the_first_run() {
        // `<w:pPr><w:rPr><w:sz/></w:rPr></w:pPr>` is the pilcrow's own
        // formatting and sits outside any `w:r`, so it survived into the
        // first unsized run and promoted an operative sentence to a caption.
        // Word writes it routinely after merges and deletions.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>Recipient shall protect Confidential Information.</w:t></w:r></w:p>
            <w:p><w:pPr><w:rPr><w:sz w:val="72"/></w:rPr></w:pPr><w:r><w:t>Notwithstanding the foregoing, disclosure compelled by law is permitted.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        let promoted = document
            .blocks
            .iter()
            .find(|block| block.text.contains("Notwithstanding"))
            .and_then(|block| block.is_heading);
        assert_ne!(
            promoted,
            Some(true),
            "a paragraph-mark size must not promote an operative sentence"
        );
    }

    #[test]
    fn docx_reports_no_signal_rather_than_claiming_body_text() {
        // The regression this guards: emitting Some(false) whenever no style
        // and no size were read reported absence of signal as a positive
        // claim of body text, which killed the lexical fallback for every
        // DOCX. A real Word agreement collapsed from 21 provisions to 2 --
        // one 93-sentence blob -- and answerable clauses went from 11 to 1.
        //
        // These are the template shapes that produce no direct formatting:
        // sizes living in styles.xml, a custom firm style, uniform sizing.
        for (label, body) in [
            (
                "no direct size anywhere",
                r#"<w:p><w:r><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
                   <w:p><w:r><w:t>Recipient shall not disclose.</w:t></w:r></w:p>"#,
            ),
            (
                "custom firm style, not a Word heading style",
                r#"<w:p><w:pPr><w:pStyle w:val="ArticleHeading"/></w:pPr><w:r><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
                   <w:p><w:r><w:t>Recipient shall not disclose.</w:t></w:r></w:p>"#,
            ),
        ] {
            let bytes = synthetic_docx(&format!(
                r#"<w:document xmlns:w="urn:test"><w:body>{body}</w:body></w:document>"#
            ));
            let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
            for block in &document.blocks {
                assert_eq!(
                    block.is_heading, None,
                    "{label}: absence of signal must be reported as None so the \
                     lexical fallback still runs, got {:?} for {:?}",
                    block.is_heading, block.text
                );
            }
        }
    }

    #[test]
    fn docx_bold_off_and_tracked_changes_do_not_invert_the_signal() {
        // `<w:b w:val="0"/>` means NOT bold; reading it as bold excluded
        // those paragraphs from the body-size sample and collapsed the
        // document. `w:pPrChange` records the properties a tracked change
        // replaced -- reading it let a revision record override the live
        // style, inverting the flag in both directions.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/><w:pPrChange><w:pPr><w:pStyle w:val="Normal"/></w:pPr></w:pPrChange></w:pPr><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>7. CONFIDENTIALITY</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/><w:b w:val="0"/></w:rPr><w:t>Recipient shall not disclose the information.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/><w:b w:val="0"/></w:rPr><w:t>These duties survive termination of the agreement.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        let marked = |needle: &str| {
            document
                .blocks
                .iter()
                .find(|block| block.text.contains(needle))
                .and_then(|block| block.is_heading)
        };
        // The live style wins over the change record.
        assert_eq!(marked("CONFIDENTIALITY"), Some(true));
        // Bold-off paragraphs still count toward the body-size sample, so it
        // is not empty; at body size the file does not distinguish them.
        assert_ne!(marked("shall not disclose"), Some(true));
    }

    #[test]
    fn docx_a_drop_cap_does_not_promote_an_ordinary_sentence() {
        // The paragraph's size is its most common run size, not its largest:
        // a 36pt drop cap made an operative sentence a caption, and the clause
        // beneath it was filed underneath that sentence.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:rPr><w:sz w:val="72"/></w:rPr><w:t>N</w:t></w:r><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>otwithstanding the foregoing, disclosure compelled by law is permitted.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>Recipient shall give prompt notice.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/></w:rPr><w:t>These duties survive termination.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        assert_ne!(
            document.blocks[0].is_heading,
            Some(true),
            "a drop cap must not promote an operative sentence to a caption"
        );
    }

    #[test]
    fn docx_headings_come_from_the_document_not_from_the_text() {
        // The case no lexical rule could get right, and the one the file
        // answers unambiguously: a paragraph styled as a heading whose words
        // read as a cross-reference, beside an all-caps line that reads
        // exactly like a caption and carries no style.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>9. See Sections 3 and 4</w:t></w:r></w:p>
            <w:p><w:r><w:t>Body text of the first clause.</w:t></w:r></w:p>
            <w:p><w:r><w:t>7. CONFIDENTIALITY AND SURVIVAL OF OBLIGATIONS</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        let marked = |needle: &str| {
            document
                .blocks
                .iter()
                .find(|block| block.text.contains(needle))
                .and_then(|block| block.is_heading)
        };
        assert_eq!(marked("See Sections"), Some(true));
        // Unstyled: the file did not say, so the lexical rule decides.
        assert_eq!(marked("CONFIDENTIALITY AND SURVIVAL"), None);
        assert_eq!(marked("Body text of the first"), None);
    }

    #[test]
    fn size_alone_never_promotes_a_paragraph() {
        // Five rounds of relative-size rules each promoted operative text and
        // severed it from its caption. A statutory conspicuous-type notice is
        // set larger because a statute requires it; a twelve-point front page
        // over a ten-point back page is an order form over standard terms.
        // Neither is a caption, and both were promoted. Size is layout, not
        // structure, so it is no longer consulted at all.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:rPr><w:sz w:val="20"/></w:rPr><w:t>7. INDEMNIFICATION AND HOLD HARMLESS.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="24"/><w:b/></w:rPr><w:t>NOTICE: THE SELLER SHALL INDEMNIFY THE BUYER AND THIS OBLIGATION SURVIVES TERMINATION.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        for block in &document.blocks {
            assert_eq!(
                block.is_heading, None,
                "size must never be read as structure: {:?}",
                block.text
            );
        }
    }

    #[test]
    fn an_absurd_declared_size_cannot_overflow_or_promote() {
        // `w:sz` was parsed as u32 and never range-checked, and the margin
        // comparison added to it: an attacker-declared 4294967294 panicked in
        // overflow-checked builds and wrapped in release, promoting every
        // paragraph. No arithmetic is performed on declared sizes now.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:rPr><w:sz w:val="4294967294"/></w:rPr><w:t>Recipient shall not disclose.</w:t></w:r></w:p>
            <w:p><w:r><w:rPr><w:sz w:val="22"/></w:rPr><w:t>These duties survive termination.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        for block in &document.blocks {
            assert_eq!(block.is_heading, None);
        }
    }

    #[test]
    fn a_style_verdict_is_limited_to_real_heading_identifiers() {
        // A prefix match claimed `Subtitle`, `HeadingNote`, `TitlePage` and
        // `HeadingBase`. Word templates put the preamble and recitals under
        // `Subtitle`, and a style verdict is unconditional -- the lexical
        // fallback cannot recover from it.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>7. Confidentiality</w:t></w:r></w:p>
            <w:p><w:pPr><w:pStyle w:val="Subtitle"/></w:pPr><w:r><w:t>The parties enter this Agreement as of the date below.</w:t></w:r></w:p>
            <w:p><w:pPr><w:pStyle w:val="HeadingNote"/></w:pPr><w:r><w:t>Recipient shall not disclose.</w:t></w:r></w:p>
            <w:p><w:pPr><w:pStyle w:val="TitlePage"/></w:pPr><w:r><w:t>These duties survive termination.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        let marked = |needle: &str| {
            document
                .blocks
                .iter()
                .find(|block| block.text.contains(needle))
                .and_then(|block| block.is_heading)
        };
        assert_eq!(marked("7. Confidentiality"), Some(true));
        assert_eq!(marked("parties enter this Agreement"), None);
        assert_eq!(marked("Recipient shall not disclose"), None);
        assert_eq!(marked("These duties survive"), None);
    }

    #[test]
    fn docx_paragraph_anchors_survive_empty_spacers_and_table_cells() {
        // Word documents routinely carry empty spacer paragraphs and
        // paragraphs inside tables. Anchoring on the count of paragraphs
        // *emitted* meant every skipped empty paragraph shifted the anchor,
        // so a lawyer told "paragraph 3" and asked to verify the quote in
        // Word landed somewhere else, with the drift growing through the
        // document.
        let bytes = synthetic_docx(
            r#"<w:document xmlns:w="urn:test"><w:body>
            <w:p><w:r><w:t>Recitals paragraph one.</w:t></w:r></w:p>
            <w:p/>
            <w:p><w:r><w:t>   </w:t></w:r></w:p>
            <w:p/>
            <w:p><w:r><w:t>Seller shall indemnify and hold harmless the Buyer.</w:t></w:r></w:p>
            </w:body></w:document>"#,
        );
        let document = convert_bytes(SourceFormat::Docx, &bytes).expect("convert");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].source_anchor, "paragraph:000001");
        // Fifth <w:p> in the file, not the second one emitted.
        assert_eq!(
            document.blocks[1].source_anchor, "paragraph:000005",
            "anchor must name the paragraph's position in the document, got {}",
            document.blocks[1].source_anchor
        );
        assert_eq!(
            document.blocks[1].text,
            "Seller shall indemnify and hold harmless the Buyer."
        );
    }

    #[test]
    fn docx_doctype_and_input_budgets_fail_closed() {
        let malicious = synthetic_docx(
            r#"<!DOCTYPE x [<!ENTITY e SYSTEM "file:///etc/passwd">]>
            <w:document xmlns:w="urn:test"><w:p><w:r><w:t>&e;</w:t></w:r></w:p></w:document>"#,
        );
        assert_eq!(
            convert_bytes(SourceFormat::Docx, &malicious),
            Err(ConversionError::MalformedSource)
        );
        assert_eq!(
            convert_bytes(SourceFormat::Docx, &[]),
            Err(ConversionError::InputBudgetExceeded)
        );
    }

    #[test]
    fn pdf_conversion_preserves_page_anchors() {
        // Fails if ordinary PDF extraction or the no-raster classifier path stops working.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_pdf()).expect("convert");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].source_anchor, "page:0001");
        assert_eq!(document.blocks[1].source_anchor, "page:0001");
        assert!(document.blocks[0].text.contains("CONFIDENTIALITY"));
        assert!(document.blocks[1].text.contains("affiliate data"));
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
    }

    #[test]
    fn a_scanners_embedded_ocr_layer_is_reported_as_machine_read() {
        // Fails if a real raster plus an extracted OCR text layer is not detected.
        let document = convert_bytes(
            SourceFormat::Pdf,
            &synthetic_copier_pdf("3 Tr ", Some(false), true),
        )
        .expect("convert");
        assert!(
            document.blocks.iter().any(|block| block.text.contains("CONFIDENTIALITY")),
            "the reading is still extracted -- demotion changes what it may claim, not whether it is seen"
        );
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
        assert_eq!(
            document.machine_read_anchors,
            BTreeSet::from(["page:0001".to_string()])
        );
    }

    #[test]
    fn drawing_order_does_not_change_scan_image_provenance() {
        // Fails if classification starts depending on page drawing order again.
        for image_before_text in [true, false] {
            let document = convert_bytes(
                SourceFormat::Pdf,
                &synthetic_copier_pdf("", Some(image_before_text), true),
            )
            .expect("convert");
            assert_eq!(
                document.text_origin,
                TextOrigin::MachineReadLayer,
                "an enumerated scan must demote its document in either drawing order"
            );
            assert!(document.machine_read_anchors.contains("page:0001"));
        }
    }

    #[test]
    fn invisible_text_without_a_scan_image_is_author_written() {
        // Fails if text rendering mode is mistaken for raster provenance.
        let document = convert_bytes(
            SourceFormat::Pdf,
            &synthetic_copier_pdf("3 Tr ", None, true),
        )
        .expect("convert");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
    }

    #[test]
    fn text_render_mode_does_not_change_scan_image_provenance() {
        // Fails if any visible, invisible, or clipping text mode bypasses the object sweep.
        for render_mode in ["", "3 Tr ", "7 Tr "] {
            let document = convert_bytes(
                SourceFormat::Pdf,
                &synthetic_copier_pdf(render_mode, Some(false), true),
            )
            .expect("convert");
            assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
        }
    }

    #[test]
    fn a_signature_sized_image_does_not_demote_a_page() {
        // Fails if intrinsic pixel count can override the signature's small drawn coverage.
        let bytes = synthetic_raster_pdf_with_draw(600, 600, "q 144 0 0 72 72 72 cm /Im1 Do Q");
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert signature PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn a_dctdecode_logo_on_a_typed_page_is_author_written() {
        // Fails if JPEG pixel bytes are decoded as content instead of trusting image dimensions.
        let jpeg = vec![0xff, 0xd8, 0xff, 0xd9];
        let bytes =
            synthetic_typed_pdf_with_image(image_xobject(300, 75, "/Filter /DCTDecode", jpeg));
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert JPEG-logo PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn a_flate_predictor_logo_on_a_typed_page_is_author_written() {
        // Fails if image `/DecodeParms` are treated as an undecodable content-stream signal.
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder
            .write_all(&vec![0x80; 300 * 75])
            .expect("compress logo pixels");
        let compressed = encoder.finish().expect("finish compressed logo");
        let bytes = synthetic_typed_pdf_with_image(image_xobject(
            300,
            75,
            "/Filter /FlateDecode /DecodeParms << /Predictor 1 >>",
            compressed,
        ));
        let document =
            convert_bytes(SourceFormat::Pdf, &bytes).expect("convert Flate predictor-logo PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn a_high_resolution_letterhead_banner_does_not_demote_or_consume_the_budget() {
        // Fails if a top-only banner's pixels can substitute for covering half the page.
        let bytes = synthetic_raster_pdf_with_draw(1200, 300, "q 612 0 0 144 0 648 cm /Im1 Do Q");
        let parsed = pdf_extract::Document::load_mem(&bytes).expect("load banner PDF");
        assert_eq!(pdf_has_page_scan_image_with_budget(&parsed, 128), Ok(false));
        let document =
            convert_bytes(SourceFormat::Pdf, &bytes).expect("convert letterhead-banner PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn an_inherited_crop_box_defines_the_visible_page_coverage() {
        // Fails if coverage is again normalized to an oversized MediaBox instead of the inherited
        // CropBox that a renderer exposes to the reader.
        let bytes = synthetic_raster_pdf_with_page_boxes(
            "/MediaBox [0 0 2000 2000] /CropBox [0 0 612 792]",
            "",
            "q 612 0 0 792 0 0 cm /Im1 Do Q",
        );
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert cropped-page PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
        assert_eq!(
            document.machine_read_anchors,
            BTreeSet::from(["page:0001".to_string()])
        );
    }

    #[test]
    fn a_crop_box_is_clipped_to_the_media_box() {
        // Fails if off-media CropBox area dilutes coverage or if the visible intersection is not
        // the page rectangle used by the coverage grid.
        let bytes = synthetic_raster_pdf_with_page_boxes(
            "",
            "/MediaBox [0 0 612 792] /CropBox [-100 -100 612 792]",
            "q 612 0 0 792 0 0 cm /Im1 Do Q",
        );
        let document =
            convert_bytes(SourceFormat::Pdf, &bytes).expect("convert intersected-box PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_page_sized_image_clipped_to_a_small_seal_does_not_demote_the_page() {
        // Fails if `re W n` stops narrowing the image's credited coverage to the visible seal.
        for clip_operator in ["W", "W*"] {
            let draw = format!("q 72 72 72 72 re {clip_operator} n 612 0 0 792 0 0 cm /Im1 Do Q");
            let bytes = synthetic_raster_pdf_with_draw(1000, 1300, &draw);
            let document =
                convert_bytes(SourceFormat::Pdf, &bytes).expect("convert clipped-seal PDF");
            assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
            assert!(document.machine_read_anchors.is_empty());
        }
    }

    #[test]
    fn clipping_state_carries_across_a_page_content_stream_array() {
        // Fails if independently decoded page streams accidentally reset the PDF graphics state.
        // PDF treats a Contents array as one concatenated stream, so the first stream's clip must
        // still constrain the image drawn by the second stream.
        let bytes = synthetic_split_stream_clip_pdf();
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert split-stream PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn restoring_graphics_state_restores_the_pre_clip_region() {
        // Fails if a clip established inside q/Q leaks out and hides a later visible page scan.
        let draw = "q 72 72 72 72 re W n 612 0 0 792 0 0 cm /Im1 Do Q \
                    q 612 0 0 792 0 0 cm /Im1 Do Q";
        let bytes = synthetic_raster_pdf_with_draw(1000, 1300, draw);
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert restored-clip PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn an_unsupported_complex_clip_cannot_hide_a_visible_scan() {
        // Fails if unsupported path geometry is allowed to narrow coverage and create an OCR
        // false negative. The conservative result may withhold a quote but cannot manufacture it.
        let draw = "q 0 0 m 72 0 l 72 72 l 0 72 l h W n \
                    612 0 0 792 0 0 cm /Im1 Do Q";
        let bytes = synthetic_raster_pdf_with_draw(1000, 1300, draw);
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert complex-clip PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_dangling_clip_operator_fails_the_page_closed() {
        // Fails if a malformed clipping operation can be ignored and leave readable OCR eligible
        // for quotation instead of producing an unknown, machine-read page verdict.
        let bytes =
            synthetic_raster_pdf_with_page_boxes("", "/MediaBox [0 0 612 792]", "72 72 72 72 re W");
        let document = pdf_extract::Document::load_mem(&bytes).expect("load dangling-clip PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Err(()));
        assert_eq!(pdf_text_origin(&document), TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_form_bbox_clips_image_coverage() {
        // Fails if a Form XObject's mandatory BBox stops acting as its implicit clipping path.
        let bytes = synthetic_form_with_small_bbox_pdf();
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert clipped-Form PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn a_96_dpi_letter_scan_demotes_the_document() {
        // Fails if the classifier again requires a Letter scan's 816px short edge to reach 1000.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(816, 1056))
            .expect("convert 96dpi Letter scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_100_dpi_letter_scan_demotes_the_document() {
        // Fails if the classifier again requires a Letter scan's 850px short edge to reach 1000.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(850, 1100))
            .expect("convert 100dpi Letter scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_96_dpi_a4_scan_on_a_letter_page_demotes_the_document() {
        // Fails if the page-shape tolerance cannot absorb ordinary Letter/A4 scanner variance.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(794, 1123))
            .expect("convert 96dpi A4 scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_72_dpi_letter_scan_demotes_the_document() {
        // Fails if page-filling coverage stops admitting ordinary 72dpi source pixels.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(612, 792))
            .expect("convert 72dpi Letter scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_150_dpi_letter_scan_still_demotes_the_document() {
        // Fails if replacing the size rule loses the already-correct 1275x1650 scan verdict.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(1275, 1650))
            .expect("convert 150dpi Letter scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn four_horizontal_scan_strips_covering_one_page_are_machine_read() {
        // Fails if coverage is judged per image instead of unioned across all four strip draws.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_four_strip_scan_pdf())
            .expect("convert four-strip scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
        assert_eq!(
            document.machine_read_anchors,
            BTreeSet::from(["page:0001".to_string()])
        );
    }

    #[test]
    fn a_cropped_scan_drawn_to_cover_the_page_is_machine_read() {
        // Fails if intrinsic aspect ratio is reintroduced for a page-filling cropped scan.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(1275, 1450))
            .expect("convert cropped scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_downsampled_scan_scaled_to_fill_the_page_is_machine_read() {
        // Fails if an intrinsic per-edge minimum again excludes a 499px-wide page scan.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(499, 646))
            .expect("convert downsampled scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_one_pixel_background_stretched_over_the_page_is_author_written() {
        // Fails if coverage alone can promote a stretched 1x1 tint without 250,000 source pixels.
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_raster_pdf(1, 1))
            .expect("convert stretched background PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert!(document.machine_read_anchors.is_empty());
    }

    #[test]
    fn a_landscape_page_scan_demotes_the_document() {
        // Fails if page-shape matching depends on portrait rather than normalized orientation.
        let bytes = synthetic_raster_pdf_on_page(792, 612, 1056, 816);
        let document =
            convert_bytes(SourceFormat::Pdf, &bytes).expect("convert landscape Letter scan PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn a_chart_exhibit_marks_only_its_page_machine_read() {
        // Fails if page-two coverage is flattened onto typed page one or omitted from its anchor.
        let bytes = synthetic_two_page_raster_pdf(1275, 1650);
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert scan PDF");
        assert_eq!(document.text_origin, TextOrigin::AuthorWritten);
        assert_eq!(
            document.machine_read_anchors,
            BTreeSet::from(["page:0002".to_string()])
        );
        assert!(document
            .blocks
            .iter()
            .any(|block| block.source_anchor == "page:0001"));
    }

    #[test]
    fn an_image_behind_an_extgstate_soft_mask_is_found() {
        // Fails if `gs` no longer follows an ExtGState soft-mask Form with the page CTM.
        let bytes = synthetic_extgstate_soft_mask_pdf();
        let document = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert soft-mask PDF");
        assert_eq!(document.text_origin, TextOrigin::MachineReadLayer);
        assert_eq!(
            document.machine_read_anchors,
            BTreeSet::from(["page:0001".to_string()])
        );
    }

    #[test]
    fn an_inline_image_in_a_form_xobject_is_found() {
        // Fails if a Form's content operators or inherited CTM stop contributing page coverage.
        let bytes = synthetic_inline_image_in_form_pdf();
        let document = pdf_extract::Document::load_mem(&bytes).expect("load inline-image PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Ok(true));
        assert_eq!(pdf_text_origin(&document), TextOrigin::MachineReadLayer);
        let converted = convert_bytes(SourceFormat::Pdf, &bytes).expect("convert inline-image PDF");
        assert_eq!(converted.text_origin, TextOrigin::MachineReadLayer);
    }

    #[test]
    fn an_inline_image_in_a_tiling_pattern_is_found() {
        // Fails if `scn` no longer follows its tiling-pattern stream and inline image placement.
        let bytes = synthetic_inline_image_in_tiling_pattern_pdf();
        let document = pdf_extract::Document::load_mem(&bytes).expect("load tiling-pattern PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Ok(true));
    }

    #[test]
    fn an_inline_image_in_an_annotation_appearance_is_found() {
        // Fails if an annotation appearance's BBox-to-Rect placement stops reaching the page grid.
        let bytes = synthetic_inline_image_in_annotation_appearance_pdf();
        let document = pdf_extract::Document::load_mem(&bytes).expect("load appearance PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Ok(true));
    }

    #[test]
    fn an_inline_image_in_a_type3_glyph_procedure_is_found() {
        // Fails if selecting a Type3 font no longer follows its bounded CharProcs content.
        let bytes = synthetic_inline_image_in_type3_glyph_pdf();
        let document = pdf_extract::Document::load_mem(&bytes).expect("load Type3 PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Ok(true));
    }

    #[test]
    fn a_document_with_an_unparseable_object_table_fails_closed() {
        // Fails if lopdf's skipped malformed xref entry is mistaken for a complete object sweep.
        let stream = b"BT (Readable text.) Tj ET";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            pdf_stream("", stream.to_vec()),
            b"<< /ThisObjectNeverCloses".to_vec(),
        ];
        let bytes = assemble_pdf(&objects);
        let document = pdf_extract::Document::load_mem(&bytes).expect("load PDF");
        assert!(!document.objects.contains_key(&(5, 0)));
        assert_eq!(pdf_has_page_scan_image(&document), Err(()));
        assert_eq!(pdf_text_origin(&document), TextOrigin::MachineReadLayer);
    }

    #[test]
    fn an_undecodable_content_stream_fails_closed() {
        // Fails if a page content-stream decode error no longer produces an unknown verdict.
        let page_stream = b"BT (Readable text.) Tj ET";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] >>"
                .to_vec(),
            pdf_stream("", page_stream.to_vec()),
            pdf_stream("/Filter /UnsupportedDecode", b"not decodable".to_vec()),
        ];
        let bytes = assemble_pdf(&objects);
        let document = pdf_extract::Document::load_mem(&bytes).expect("load PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Err(()));
        assert_eq!(pdf_text_origin(&document), TextOrigin::MachineReadLayer);
        assert_eq!(
            pdf_page_scan_verdict(&document).machine_read_anchors,
            BTreeSet::from(["page:0001".to_string()])
        );
    }

    #[test]
    fn the_stream_sweep_enforces_a_cumulative_decompressed_byte_budget() {
        // Fails if decoded bytes are not accumulated across all swept content streams.
        let first = b"q 1 0 0 1 0 0 cm Q";
        let second = b"BT (Readable text.) Tj ET";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents [4 0 R 5 0 R] >>"
                .to_vec(),
            pdf_stream("", first.to_vec()),
            pdf_stream("", second.to_vec()),
        ];
        let bytes = assemble_pdf(&objects);
        let document = pdf_extract::Document::load_mem(&bytes).expect("load PDF");
        assert_eq!(
            pdf_has_page_scan_image_with_budget(
                &document,
                first.len().checked_add(second.len()).expect("fixture size") - 1
            ),
            Err(())
        );
    }

    #[test]
    fn a_compressed_stream_cannot_expand_past_the_sweep_budget() {
        // Fails if one Flate stream can allocate or return more than the remaining byte budget.
        use flate2::write::ZlibEncoder;
        use flate2::Compression;

        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&vec![b'A'; 4_096]).expect("compress");
        let compressed = encoder.finish().expect("finish compressed stream");
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R >>".to_vec(),
            pdf_stream("/Filter /FlateDecode", compressed),
        ];
        let bytes = assemble_pdf(&objects);
        let document = pdf_extract::Document::load_mem(&bytes).expect("load PDF");
        assert_eq!(pdf_has_page_scan_image_with_budget(&document, 128), Err(()));
    }

    #[test]
    fn a_recursive_reference_cycle_fails_the_page_closed() {
        // Fails if revisiting an active Form stream no longer closes uncertain page provenance.
        let page_stream = b"/Fm1 Do";
        let form_stream = b"/Fm1 Do";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /XObject << /Fm1 5 0 R >> >> /Contents 4 0 R >>".to_vec(),
            pdf_stream("", page_stream.to_vec()),
            pdf_stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /XObject << /Fm1 5 0 R >> >>",
                form_stream.to_vec(),
            ),
        ];
        let bytes = assemble_pdf(&objects);
        let document = pdf_extract::Document::load_mem(&bytes).expect("load PDF");
        assert_eq!(pdf_has_page_scan_image(&document), Err(()));
        assert_eq!(pdf_text_origin(&document), TextOrigin::MachineReadLayer);
    }

    /// A text layer damaged past reading must not be quoted; it converts as
    /// "needs OCR" so the pages can be read properly instead.
    #[test]
    fn a_garbled_text_layer_converts_as_needing_ocr() {
        let block = |text: &str| ConvertedBlock {
            is_heading: None,
            source_anchor: "page:0001".to_string(),
            text: text.to_string(),
            flow: AnchorFlow::Continue,
        };
        // A broken ToUnicode CMap in practice: most characters land in the
        // private use area, a few survive.
        let mojibake = "\u{E01F}\u{E020}\u{E021} the \u{E022}\u{E023}\u{E024}\u{E025} shall \u{E026}\u{E027}\u{E028}\u{E029}"
            .repeat(20);
        assert!(text_layer_is_garbled(&[block(&mojibake)]));

        // Operative legal text with ordinary punctuation and symbols is
        // nowhere near the threshold.
        let legal = "7.1 Indemnification. Provider shall indemnify, defend and hold harmless \
                     the Buyer (including its affiliates) against 100% of Losses; see \u{00A7}7.3."
            .repeat(5);
        assert!(!text_layer_is_garbled(&[block(&legal)]));

        // Below the minimum, a ratio is noise and the layer is kept.
        assert!(!text_layer_is_garbled(&[block("\u{FFFD}\u{FFFD}\u{FFFD}")]));
    }

    #[test]
    fn converted_output_validation_rejects_control_anchors() {
        let document = ConvertedDocument {
            format: SourceFormat::Pdf,
            blocks: vec![ConvertedBlock {
                is_heading: None,
                source_anchor: "page:\n1".to_string(),
                text: "Evidence".to_string(),
                flow: AnchorFlow::HardBoundary,
            }],
            warnings: Vec::new(),
            text_origin: TextOrigin::AuthorWritten,
            machine_read_anchors: BTreeSet::new(),
        };
        assert_eq!(document.validate(), Err(ConversionError::MalformedOutput));
    }
}
