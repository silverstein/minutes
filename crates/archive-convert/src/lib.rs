//! Bounded byte-to-text conversion for untrusted legal documents.
//!
//! Parsing functions are public for deterministic fixture tests. Production
//! callers must use the worker entry point so PDF and ZIP/XML parsing never
//! occurs in the Tauri process.

use quick_xml::events::Event;
use quick_xml::Reader;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
const WORKER_MEMORY_GROWTH_BYTES: u64 = 1024 * 1024 * 1024;
const WORKER_DEADLINE: Duration = Duration::from_secs(20);
const MAX_WORKER_STDERR_BYTES: usize = 4 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Pdf,
    Docx,
}

impl SourceFormat {
    pub fn parse(value: &str) -> Result<Self, ConversionError> {
        match value {
            "pdf" => Ok(Self::Pdf),
            "docx" => Ok(Self::Docx),
            _ => Err(ConversionError::UnsupportedFormat),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertedDocument {
    pub format: SourceFormat,
    pub blocks: Vec<ConvertedBlock>,
    pub warnings: Vec<String>,
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
    _snapshot_directory: tempfile::TempDir,
    executable_path: PathBuf,
    executable: fs::File,
    executable_bytes: u64,
    executable_digest: [u8; 32],
}

impl std::fmt::Debug for BoundedConverter {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BoundedConverter([private immutable worker snapshot])")
    }
}

impl BoundedConverter {
    /// Path of the private worker snapshot directory.
    ///
    /// Exposed so the app can reclaim it explicitly. `exit(0)` does not
    /// unwind, and during a vault build this object lives inside a blocking
    /// task rather than in shared session state, so nothing the close handler
    /// can reach owns it and no destructor will run.
    pub fn snapshot_directory(&self) -> &Path {
        self._snapshot_directory.path()
    }

    pub fn bind(worker_executable: &Path) -> Result<Self, WorkerError> {
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
        let mut source = source_options
            .open(&canonical)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        let source_metadata = source
            .metadata()
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        if !source_metadata.is_file() {
            return Err(WorkerError::ExecutableUnavailable);
        }

        let snapshot_directory = tempfile::Builder::new()
            .prefix("minutes-archive-worker-")
            .tempdir()
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        #[cfg(unix)]
        fs::set_permissions(
            snapshot_directory.path(),
            <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .map_err(|_| WorkerError::ExecutableUnavailable)?;
        let executable_path = snapshot_directory.path().join("worker");
        let mut snapshot_options = fs::OpenOptions::new();
        snapshot_options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            snapshot_options.mode(0o500);
        }
        let mut snapshot = snapshot_options
            .open(&executable_path)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        std::io::copy(&mut source, &mut snapshot)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        snapshot
            .sync_all()
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            snapshot
                .set_permissions(fs::Permissions::from_mode(0o500))
                .map_err(|_| WorkerError::ExecutableUnavailable)?;
        }
        drop(snapshot);
        let executable =
            fs::File::open(&executable_path).map_err(|_| WorkerError::ExecutableUnavailable)?;
        let (executable_bytes, executable_digest) =
            digest_file(&executable).map_err(|_| WorkerError::ExecutableUnavailable)?;
        if executable_bytes != source_metadata.len() {
            return Err(WorkerError::ExecutableUnavailable);
        }
        let converter = Self {
            _snapshot_directory: snapshot_directory,
            executable_path,
            executable,
            executable_bytes,
            executable_digest,
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

    fn verify_executable(&self) -> Result<(), WorkerError> {
        let metadata = fs::symlink_metadata(&self.executable_path)
            .map_err(|_| WorkerError::ExecutableUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(WorkerError::ExecutableUnavailable);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if metadata.permissions().mode() & 0o222 != 0 {
                return Err(WorkerError::ExecutableUnavailable);
            }
        }
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
        let mut child = command.spawn().map_err(|_| WorkerError::WorkerFailed)?;
        let mut stdin = child.stdin.take().ok_or(WorkerError::WorkerFailed)?;
        let stdout = child.stdout.take().ok_or(WorkerError::WorkerFailed)?;
        let stderr = child.stderr.take().ok_or(WorkerError::WorkerFailed)?;
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
            match child.try_wait().map_err(|_| WorkerError::WorkerFailed)? {
                Some(exit_status) => break exit_status,
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => {
                    #[cfg(unix)]
                    unsafe {
                        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
                    }
                    let _ = child.kill();
                    let _ = child.wait();
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
    };
    document.validate()?;
    Ok(document)
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
    let warnings = if blocks.is_empty() {
        vec!["ocr_required_or_no_extractable_text".to_string()]
    } else if !pdf_has_usable_structure_signal(&blocks) {
        // A text-only PDF can be perfectly extractable while still being
        // unsafe to segment: uniform-size, uniformly-spaced title-case
        // captions are indistinguishable from body prose here. Do not let
        // retrieval turn that ambiguity into a fabricated conjunction.
        vec![PDF_UNSUPPORTED_STRUCTURE_WARNING.to_string()]
    } else {
        Vec::new()
    };
    Ok(ConvertedDocument {
        format: SourceFormat::Pdf,
        blocks,
        warnings,
    })
}

fn pdf_has_usable_structure_signal(blocks: &[ConvertedBlock]) -> bool {
    blocks.iter().any(|block| {
        block.is_heading == Some(true)
            || block
                .text
                .lines()
                .map(str::trim)
                .any(pdf_lexical_structure_signal)
    })
}

/// Keep this deliberately narrower than title-case detection. A short
/// title-case sentence is the exact shape that pdf-extract cannot distinguish
/// from body prose in a uniformly formatted PDF.
fn pdf_lexical_structure_signal(line: &str) -> bool {
    if line.is_empty() || line.len() > 180 {
        return false;
    }
    let words = line.split_whitespace().collect::<Vec<_>>();
    if words.is_empty() || words.len() > 12 {
        return false;
    }
    let lowercase = line.to_ascii_lowercase();
    let known_prefix = ["section ", "article ", "schedule ", "exhibit "]
        .iter()
        .any(|prefix| lowercase.starts_with(prefix));
    let numbered = line.split_once(['.', ')']).is_some_and(|(prefix, rest)| {
        !rest.trim().is_empty()
            && prefix.len() <= 12
            && prefix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte == b'.')
    });
    let letters = line.chars().filter(|character| character.is_alphabetic());
    let (letter_count, uppercase_count) = letters.fold((0usize, 0usize), |counts, character| {
        (
            counts.0 + 1,
            counts.1 + usize::from(character.is_uppercase()),
        )
    });
    let uppercase = letter_count >= 4 && uppercase_count == letter_count;
    let run_in = line.ends_with('.')
        && letter_count >= 4
        && words.iter().all(|word| {
            word.chars()
                .find(|character| character.is_alphabetic())
                .is_some_and(char::is_uppercase)
        });
    known_prefix || numbered || uppercase || run_in
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
        let gaps = raw
            .windows(2)
            .map(|pair| (pair[1].1 - pair[0].1).max(0.0))
            .collect::<Vec<_>>();
        let reference_gap = median(
            gaps.into_iter()
                .filter(|gap| *gap <= reference_size * 3.0)
                .collect(),
        )
        .unwrap_or(reference_size * 1.35);
        let mut previous_y = None;
        raw.into_iter()
            .map(|(text, y, size)| {
                let leading_gap = previous_y.map_or(0.0, |previous| y - previous);
                previous_y = Some(y);
                let geometric_heading = leading_gap > reference_gap * 1.3
                    && text.split_whitespace().count() <= 8
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
                if skip_depth > 0 || matches!(local, b"pPrChange" | b"rPrChange") {
                    skip_depth += 1;
                }
                match local {
                    b"t" if skip_depth == 0 => in_text = true,
                    b"pStyle" if skip_depth == 0 => {
                        if let Some(value) = attribute_value(&event, b"val") {
                            saw_style = true;
                            heading_style = is_heading_style(&value);
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Empty(event)) => match local_name(event.name().as_ref()) {
                // Run and paragraph properties are usually self-closing.
                b"pStyle" if skip_depth == 0 => {
                    if let Some(value) = attribute_value(&event, b"val") {
                        saw_style = true;
                        heading_style = is_heading_style(&value);
                    }
                }
                b"tab" => paragraph.push('\t'),
                b"br" | b"cr" => paragraph.push('\n'),
                // `<w:p/>` is a self-closing empty paragraph and arrives as
                // Empty rather than Start/End. Word emits these constantly as
                // spacers, and each one still occupies a paragraph position
                // in the document a reader is asked to navigate to.
                b"p" => paragraph_ordinal += 1,
                _ => {}
            },
            Ok(Event::Text(event)) if in_text && skip_depth == 0 => {
                let decoded = event
                    .decode()
                    .map_err(|_| ConversionError::MalformedSource)?;
                paragraph.push_str(&decoded);
            }
            Ok(Event::GeneralRef(reference)) if in_text && skip_depth == 0 => {
                if let Some(character) = reference
                    .resolve_char_ref()
                    .map_err(|_| ConversionError::MalformedSource)?
                {
                    paragraph.push(character);
                } else {
                    let name = reference
                        .decode()
                        .map_err(|_| ConversionError::MalformedSource)?;
                    let value = quick_xml::escape::resolve_xml_entity(&name)
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
                if local_name(event.name().as_ref()) == b"p" {
                    skip_depth = 0;
                }
            }
            Ok(Event::End(event)) => match local_name(event.name().as_ref()) {
                b"t" => in_text = false,
                b"p" => {
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
    })
}

/// Attribute value by local name, ignoring namespace prefix.
fn attribute_value(event: &quick_xml::events::BytesStart<'_>, wanted: &[u8]) -> Option<String> {
    event.attributes().flatten().find_map(|attribute| {
        (local_name(attribute.key.as_ref()) == wanted)
            .then(|| String::from_utf8_lossy(&attribute.value).into_owned())
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

fn local_name(name: &[u8]) -> &[u8] {
    name.rsplit(|byte| *byte == b':').next().unwrap_or(name)
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
        let document = convert_bytes(SourceFormat::Pdf, &synthetic_pdf()).expect("convert");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[0].source_anchor, "page:0001");
        assert_eq!(document.blocks[1].source_anchor, "page:0001");
        assert!(document.blocks[0].text.contains("CONFIDENTIALITY"));
        assert!(document.blocks[1].text.contains("affiliate data"));
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
        };
        assert_eq!(document.validate(), Err(ConversionError::MalformedOutput));
    }
}
