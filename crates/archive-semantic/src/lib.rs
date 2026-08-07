//! Revision-pinned, on-device semantic vectors for the Archive pilot.
//!
//! This crate deliberately uses Apple's built-in `NLEmbedding` sentence model.
//! It never calls the contextual-embedding asset request API and has no network
//! dependency. Vectors are normalized in memory and callers remain responsible
//! for vault scoping, derivative lifetime, and source-revision checks.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use thiserror::Error;

pub const WORKER_MARKER: &str = "--minutes-archive-semantic-worker-v1";
pub const APPLE_ENGLISH_SENTENCE_MODEL_ID: &str = "apple-nl-sentence-en-r1";
pub const APPLE_ENGLISH_SENTENCE_REVISION: usize = 1;
pub const APPLE_ENGLISH_SENTENCE_DIMENSION: usize = 512;
pub const MAX_SEMANTIC_INPUT_CHARS: usize = 4_096;
const MAX_WORKER_REQUEST_BYTES: usize = 32 * 1024;
const MAX_WORKER_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_WORKER_STDERR_BYTES: usize = 4 * 1024;
const WORKER_RESPONSE_DEADLINE: Duration = Duration::from_secs(30);
const WORKER_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(2);
const WORKER_CPU_SECONDS: u64 = 600;
const WORKER_MEMORY_GROWTH_BYTES: u64 = 2 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SemanticModelMetadata {
    pub model_id: &'static str,
    pub revision: usize,
    pub dimension: usize,
    /// Whether THIS crate targets the OS built-in model rather than bundling
    /// or fetching one. A compile-time property of our call graph, not a
    /// runtime observation of the operating system.
    pub built_in_os_asset: bool,
    /// Whether THIS crate calls any model-download API. Also compile-time.
    ///
    /// It is deliberately not named as a claim that no download occurs. The
    /// worker is network-denied, but Apple's asset services are separate
    /// system processes acting on their own behalf, and on a Mac lacking the
    /// linguistic asset their behaviour is the OS's to decide, not ours to
    /// assert. An independent reviewer flagged the old framing as asserted
    /// rather than measured, and was right.
    pub model_download_requested: bool,
}

impl SemanticModelMetadata {
    pub const fn apple_english_sentence_revision_one() -> Self {
        Self {
            model_id: APPLE_ENGLISH_SENTENCE_MODEL_ID,
            revision: APPLE_ENGLISH_SENTENCE_REVISION,
            dimension: APPLE_ENGLISH_SENTENCE_DIMENSION,
            built_in_os_asset: true,
            model_download_requested: false,
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SemanticError {
    #[error("the on-device semantic model is unavailable on this platform")]
    PlatformUnavailable,
    #[error("the pinned on-device semantic model revision is unavailable")]
    ModelUnavailable,
    #[error("the semantic input is empty or exceeds its character budget")]
    InputBudgetExceeded,
    #[error("the on-device semantic model returned an invalid vector")]
    InvalidVector,
    #[error("the semantic worker executable is unavailable or mutable")]
    ExecutableUnavailable,
    #[error("the semantic worker could not install or prove its security boundary")]
    SecurityBoundaryUnavailable,
    #[error("the semantic worker exceeded its response deadline or byte budget")]
    WorkerBudgetExceeded,
    #[error("the semantic worker stopped without a valid result")]
    WorkerFailed,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerRequest {
    text: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkerResponse {
    vector: Option<Vec<f32>>,
    error: Option<String>,
}

/// Binds semantic execution to one pinned copy of the app executable, running
/// in place. Each build session installs the macOS sandbox before it creates
/// the Natural Language model or reads any confidential text.
pub struct BoundedSemanticEngine {
    executable_path: PathBuf,
    /// Held open with `O_NOFOLLOW` for the object's lifetime, pinning one
    /// inode so the digest is always re-read from the same file.
    executable: fs::File,
    executable_identity: FileIdentity,
    executable_bytes: u64,
    executable_digest: [u8; 32],
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

impl std::fmt::Debug for BoundedSemanticEngine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BoundedSemanticEngine([pinned worker executable])")
    }
}

impl BoundedSemanticEngine {
    pub fn bind(worker_executable: &Path) -> Result<Self, SemanticError> {
        let canonical = fs::canonicalize(worker_executable)
            .map_err(|_| SemanticError::ExecutableUnavailable)?;
        let lexical =
            fs::symlink_metadata(&canonical).map_err(|_| SemanticError::ExecutableUnavailable)?;
        if lexical.file_type().is_symlink() || !lexical.is_file() {
            return Err(SemanticError::ExecutableUnavailable);
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
            .map_err(|_| SemanticError::ExecutableUnavailable)?;
        let source_metadata = source
            .metadata()
            .map_err(|_| SemanticError::ExecutableUnavailable)?;
        if !source_metadata.is_file() {
            return Err(SemanticError::ExecutableUnavailable);
        }

        // Runs in place, like the converter and for the same reason: a
        // Developer ID signature with the hardened runtime is bound to its
        // bundle, so a copy of the executable fails validation and is SIGKILLed
        // on exec. The descriptor opened above pins the inode instead, and
        // `verify_executable` re-reads the digest through it before every use.
        let executable_identity = file_identity(&source_metadata);
        let (executable_bytes, executable_digest) =
            digest_file(&source).map_err(|_| SemanticError::ExecutableUnavailable)?;
        if executable_bytes != source_metadata.len() {
            return Err(SemanticError::ExecutableUnavailable);
        }
        let engine = Self {
            executable_path: canonical,
            executable: source,
            executable_identity,
            executable_bytes,
            executable_digest,
        };
        engine.verify_sandbox()?;
        Ok(engine)
    }

    pub fn open_session(&self) -> Result<BoundedSemanticSession, SemanticError> {
        self.verify_executable()?;
        BoundedSemanticSession::spawn(&self.executable_path)
    }

    pub fn embed_once(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
        let mut session = self.open_session()?;
        session.embed(text)
    }

    fn verify_sandbox(&self) -> Result<(), SemanticError> {
        self.verify_executable()?;
        let mut command = self.worker_command("sandbox-self-test");
        let mut child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SemanticError::WorkerFailed)?;
        let deadline = Instant::now() + WORKER_RESPONSE_DEADLINE;
        loop {
            match child.try_wait().map_err(|_| SemanticError::WorkerFailed)? {
                Some(status) if status.success() => return Ok(()),
                Some(_) => return Err(SemanticError::SecurityBoundaryUnavailable),
                None if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                None => {
                    terminate_child(&mut child);
                    return Err(SemanticError::WorkerBudgetExceeded);
                }
            }
        }
    }

    fn verify_executable(&self) -> Result<(), SemanticError> {
        let metadata = fs::symlink_metadata(&self.executable_path)
            .map_err(|_| SemanticError::ExecutableUnavailable)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(SemanticError::ExecutableUnavailable);
        }
        // Identity, not permissions. The worker used to be a private 0500
        // copy; an installed bundle's executable is 0755 like any other, so a
        // no-write-bits rule would refuse every real installation.
        if file_identity(&metadata) != self.executable_identity {
            return Err(SemanticError::ExecutableUnavailable);
        }
        let pinned = self
            .executable
            .metadata()
            .map_err(|_| SemanticError::ExecutableUnavailable)?;
        if file_identity(&pinned) != self.executable_identity {
            return Err(SemanticError::ExecutableUnavailable);
        }
        let (bytes, digest) =
            digest_file(&self.executable).map_err(|_| SemanticError::ExecutableUnavailable)?;
        if bytes != self.executable_bytes || digest != self.executable_digest {
            return Err(SemanticError::ExecutableUnavailable);
        }
        Ok(())
    }

    fn worker_command(&self, operation: &str) -> Command {
        worker_command(&self.executable_path, operation)
    }
}

pub struct BoundedSemanticSession {
    child: Child,
    stdin: Option<ChildStdin>,
    responses: Receiver<Result<Vec<u8>, ()>>,
    reader: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for BoundedSemanticSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("BoundedSemanticSession([sandboxed pipe worker])")
    }
}

/// The one thing an index build needs from a semantic worker.
///
/// A trait rather than the concrete session so the build's behaviour when the
/// worker *dies partway through* can be tested without a live Apple linguistic
/// asset. That failure discarded a complete index in the field -- sixteen
/// thousand documents, twenty-two minutes -- and no test could reach it,
/// because every test either had a working worker or no worker at all. The
/// interesting case is the one in between.
pub trait SemanticEmbedder {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, SemanticError>;
}

impl SemanticEmbedder for BoundedSemanticSession {
    fn embed(&mut self, text: &str) -> Result<Vec<f32>, SemanticError> {
        BoundedSemanticSession::embed(self, text)
    }
}

impl BoundedSemanticSession {
    fn spawn(executable: &Path) -> Result<Self, SemanticError> {
        let mut command = worker_command(executable, "embed-stream");
        let mut child = command.spawn().map_err(|_| SemanticError::WorkerFailed)?;
        let stdin = child.stdin.take().ok_or(SemanticError::WorkerFailed)?;
        let mut stdout = child.stdout.take().ok_or(SemanticError::WorkerFailed)?;
        let stderr = child.stderr.take().ok_or(SemanticError::WorkerFailed)?;
        let (sender, responses) = mpsc::channel();
        let reader = thread::spawn(move || loop {
            let response = read_frame(&mut stdout, MAX_WORKER_RESPONSE_BYTES).map_err(|_| ());
            match response {
                Ok(Some(response)) => {
                    if sender.send(Ok(response)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(error));
                    break;
                }
            }
        });
        thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stderr
                .take((MAX_WORKER_STDERR_BYTES as u64).saturating_add(1))
                .read_to_end(&mut bytes);
        });
        Ok(Self {
            child,
            stdin: Some(stdin),
            responses,
            reader: Some(reader),
        })
    }

    pub fn embed(&mut self, text: &str) -> Result<Vec<f32>, SemanticError> {
        let request = WorkerRequest {
            text: text.to_string(),
        };
        validate_semantic_input(&request.text)?;
        let bytes = serde_json::to_vec(&request).map_err(|_| SemanticError::WorkerFailed)?;
        if bytes.len() > MAX_WORKER_REQUEST_BYTES {
            return Err(SemanticError::InputBudgetExceeded);
        }
        let stdin = self.stdin.as_mut().ok_or(SemanticError::WorkerFailed)?;
        write_frame(stdin, &bytes, MAX_WORKER_REQUEST_BYTES)?;
        let bytes = match self.responses.recv_timeout(WORKER_RESPONSE_DEADLINE) {
            Ok(Ok(bytes)) => bytes,
            Ok(Err(())) | Err(RecvTimeoutError::Disconnected) => {
                return Err(SemanticError::WorkerFailed);
            }
            Err(RecvTimeoutError::Timeout) => {
                terminate_child(&mut self.child);
                return Err(SemanticError::WorkerBudgetExceeded);
            }
        };
        let response: WorkerResponse =
            serde_json::from_slice(&bytes).map_err(|_| SemanticError::WorkerFailed)?;
        if response.error.is_some() {
            return Err(SemanticError::InvalidVector);
        }
        let vector = response.vector.ok_or(SemanticError::WorkerFailed)?;
        normalize_vector(vector)
    }
}

impl Drop for BoundedSemanticSession {
    fn drop(&mut self) {
        drop(self.stdin.take());
        let deadline = Instant::now() + WORKER_SHUTDOWN_DEADLINE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                _ => {
                    terminate_child(&mut self.child);
                    break;
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Worker-internal access to the pinned Apple model. External callers can only
/// obtain vectors through `BoundedSemanticEngine`.
struct AppleSentenceEmbeddingSession {
    #[cfg(target_os = "macos")]
    embedding: objc2::rc::Retained<objc2_natural_language::NLEmbedding>,
}

impl std::fmt::Debug for AppleSentenceEmbeddingSession {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("AppleSentenceEmbeddingSession([built-in model r1])")
    }
}

impl AppleSentenceEmbeddingSession {
    fn new() -> Result<Self, SemanticError> {
        platform::new_session()
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed.chars().count() > MAX_SEMANTIC_INPUT_CHARS {
            return Err(SemanticError::InputBudgetExceeded);
        }
        platform::embed(self, trimmed)
    }
}

pub fn cosine_similarity(left: &[f32], right: &[f32]) -> Result<f32, SemanticError> {
    if left.len() != APPLE_ENGLISH_SENTENCE_DIMENSION
        || right.len() != APPLE_ENGLISH_SENTENCE_DIMENSION
        || left
            .iter()
            .chain(right.iter())
            .any(|value| !value.is_finite())
    {
        return Err(SemanticError::InvalidVector);
    }
    Ok(left
        .iter()
        .zip(right.iter())
        .map(|(left, right)| left * right)
        .sum())
}

fn normalize_vector(mut vector: Vec<f32>) -> Result<Vec<f32>, SemanticError> {
    if vector.len() != APPLE_ENGLISH_SENTENCE_DIMENSION
        || vector.iter().any(|value| !value.is_finite())
    {
        return Err(SemanticError::InvalidVector);
    }
    let magnitude = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if !magnitude.is_finite() || magnitude <= f32::EPSILON {
        return Err(SemanticError::InvalidVector);
    }
    for value in &mut vector {
        *value /= magnitude;
    }
    Ok(vector)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2_foundation::NSString;
    use objc2_natural_language::{NLEmbedding, NLLanguageEnglish};
    use std::ptr::NonNull;

    pub(super) fn new_session() -> Result<AppleSentenceEmbeddingSession, SemanticError> {
        let language = unsafe { NLLanguageEnglish }.ok_or(SemanticError::ModelUnavailable)?;
        let embedding = unsafe {
            NLEmbedding::sentenceEmbeddingForLanguage_revision(
                language,
                APPLE_ENGLISH_SENTENCE_REVISION,
            )
        }
        .ok_or(SemanticError::ModelUnavailable)?;
        let dimension = unsafe { embedding.dimension() };
        let revision = unsafe { embedding.revision() };
        if dimension != APPLE_ENGLISH_SENTENCE_DIMENSION
            || revision != APPLE_ENGLISH_SENTENCE_REVISION
        {
            return Err(SemanticError::ModelUnavailable);
        }
        Ok(AppleSentenceEmbeddingSession { embedding })
    }

    pub(super) fn embed(
        session: &AppleSentenceEmbeddingSession,
        text: &str,
    ) -> Result<Vec<f32>, SemanticError> {
        let text = NSString::from_str(text);
        let mut vector = vec![0.0f32; APPLE_ENGLISH_SENTENCE_DIMENSION];
        let pointer = NonNull::new(vector.as_mut_ptr()).ok_or(SemanticError::InvalidVector)?;
        let populated = unsafe { session.embedding.getVector_forString(pointer, &text) };
        if !populated {
            return Err(SemanticError::InvalidVector);
        }
        normalize_vector(vector)
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) fn new_session() -> Result<AppleSentenceEmbeddingSession, SemanticError> {
        Err(SemanticError::PlatformUnavailable)
    }

    pub(super) fn embed(
        _session: &AppleSentenceEmbeddingSession,
        _text: &str,
    ) -> Result<Vec<f32>, SemanticError> {
        Err(SemanticError::PlatformUnavailable)
    }
}

fn validate_semantic_input(text: &str) -> Result<(), SemanticError> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_SEMANTIC_INPUT_CHARS {
        return Err(SemanticError::InputBudgetExceeded);
    }
    Ok(())
}

fn worker_command(executable: &Path, operation: &str) -> Command {
    let mut command = Command::new(executable);
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
    command
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn digest_file(file: &fs::File) -> Result<(u64, [u8; 32]), std::io::Error> {
    use std::io::{Seek, SeekFrom};

    let mut file = file.try_clone()?;
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher)?;
    Ok((bytes, hasher.finalize().into()))
}

fn write_frame(writer: &mut impl Write, bytes: &[u8], maximum: usize) -> Result<(), SemanticError> {
    if bytes.is_empty() || bytes.len() > maximum {
        return Err(SemanticError::WorkerBudgetExceeded);
    }
    writer
        .write_all(&(bytes.len() as u64).to_le_bytes())
        .and_then(|_| writer.write_all(bytes))
        .and_then(|_| writer.flush())
        .map_err(|_| SemanticError::WorkerFailed)
}

fn read_frame(reader: &mut impl Read, maximum: usize) -> Result<Option<Vec<u8>>, SemanticError> {
    let mut length_bytes = [0u8; 8];
    match reader.read(&mut length_bytes[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => {}
        Ok(_) => unreachable!(),
        Err(_) => return Err(SemanticError::WorkerFailed),
    }
    reader
        .read_exact(&mut length_bytes[1..])
        .map_err(|_| SemanticError::WorkerFailed)?;
    let length = usize::try_from(u64::from_le_bytes(length_bytes))
        .map_err(|_| SemanticError::WorkerBudgetExceeded)?;
    if length == 0 || length > maximum {
        return Err(SemanticError::WorkerBudgetExceeded);
    }
    let mut bytes = vec![0u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| SemanticError::WorkerFailed)?;
    Ok(Some(bytes))
}

pub fn run_worker_process(operation: &str) -> i32 {
    if install_worker_security_boundary().is_err() {
        return 70;
    }
    if operation == "sandbox-self-test" {
        return sandbox_self_test();
    }
    if operation != "embed-stream" {
        return 64;
    }
    let session = match AppleSentenceEmbeddingSession::new() {
        Ok(session) => session,
        Err(_) => return 69,
    };
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    loop {
        let request = match read_frame(&mut stdin, MAX_WORKER_REQUEST_BYTES) {
            Ok(Some(bytes)) => serde_json::from_slice::<WorkerRequest>(&bytes),
            Ok(None) => return 0,
            Err(_) => return 65,
        };
        let response = match request {
            Ok(request) => match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                session.embed(&request.text)
            })) {
                Ok(Ok(vector)) => WorkerResponse {
                    vector: Some(vector),
                    error: None,
                },
                Ok(Err(error)) => WorkerResponse {
                    vector: None,
                    error: Some(error.to_string()),
                },
                Err(_) => WorkerResponse {
                    vector: None,
                    error: Some("the semantic model could not process the input".to_string()),
                },
            },
            Err(_) => WorkerResponse {
                vector: None,
                error: Some("the semantic request was malformed".to_string()),
            },
        };
        let bytes = match serde_json::to_vec(&response) {
            Ok(bytes) if bytes.len() <= MAX_WORKER_RESPONSE_BYTES => bytes,
            _ => return 74,
        };
        if write_frame(&mut stdout, &bytes, MAX_WORKER_RESPONSE_BYTES).is_err() {
            return 74;
        }
    }
}

fn sandbox_self_test() -> i32 {
    let network_denied = std::net::TcpListener::bind("127.0.0.1:0").is_err();
    // Probe paths the profile never names. The previous test read
    // /etc/passwd, which passed only because that exact literal had been
    // added as a deny -- it could not detect that everything outside three
    // subtrees was still readable and writable. These probes are chosen so
    // that a regression to `(allow default)` fails the test.
    let unnamed_read_denied = std::fs::read("/private/etc/hosts").is_err()
        && std::fs::read_dir("/Applications").is_err()
        && std::fs::read_dir("/Library").is_err();
    // The worker must not be able to persist document-derived text anywhere.
    let write_denied = ["/private/tmp", "/private/var/tmp"]
        .iter()
        .all(|directory| {
            let probe = std::path::Path::new(directory).join("minutes-archive-sandbox-probe");
            let denied = std::fs::write(&probe, b"probe").is_err();
            if !denied {
                let _ = std::fs::remove_file(&probe);
            }
            denied
        })
        && std::env::temp_dir()
            .join("minutes-archive-sandbox-probe")
            .pipe_write_denied();
    if network_denied && unnamed_read_denied && write_denied {
        0
    } else {
        71
    }
}

trait ProbeWriteDenied {
    fn pipe_write_denied(&self) -> bool;
}

impl ProbeWriteDenied for std::path::PathBuf {
    fn pipe_write_denied(&self) -> bool {
        let denied = std::fs::write(self, b"probe").is_err();
        if !denied {
            let _ = std::fs::remove_file(self);
        }
        denied
    }
}

fn install_worker_security_boundary() -> Result<(), SemanticError> {
    install_resource_limits()?;
    install_platform_sandbox()
}

#[cfg(unix)]
fn install_resource_limits() -> Result<(), SemanticError> {
    let cpu = libc::rlimit {
        rlim_cur: WORKER_CPU_SECONDS,
        rlim_max: WORKER_CPU_SECONDS,
    };
    let file_size = libc::rlimit {
        rlim_cur: MAX_WORKER_RESPONSE_BYTES as u64,
        rlim_max: MAX_WORKER_RESPONSE_BYTES as u64,
    };
    let open_files = libc::rlimit {
        rlim_cur: 32,
        rlim_max: 32,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_CPU, &cpu) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_FSIZE, &file_size) } != 0
        || unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &open_files) } != 0
    {
        return Err(SemanticError::SecurityBoundaryUnavailable);
    }
    install_address_space_limit()
}

#[cfg(not(unix))]
fn install_resource_limits() -> Result<(), SemanticError> {
    Err(SemanticError::SecurityBoundaryUnavailable)
}

#[cfg(target_os = "macos")]
fn install_address_space_limit() -> Result<(), SemanticError> {
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
        return Err(SemanticError::SecurityBoundaryUnavailable);
    }
    let limit = info
        .virtual_size
        .checked_add(WORKER_MEMORY_GROWTH_BYTES)
        .ok_or(SemanticError::SecurityBoundaryUnavailable)?;
    let address_space = libc::rlimit {
        rlim_cur: limit,
        rlim_max: limit,
    };
    if unsafe { libc::setrlimit(libc::RLIMIT_AS, &address_space) } != 0 {
        return Err(SemanticError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn install_address_space_limit() -> Result<(), SemanticError> {
    Err(SemanticError::SecurityBoundaryUnavailable)
}

#[cfg(target_os = "macos")]
fn install_platform_sandbox() -> Result<(), SemanticError> {
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

    // Deny by default. `mach-lookup` cannot be denied outright -- Apple's
    // built-in NLEmbedding model fails to load without it -- so the services
    // that provide an out-of-process persistence or broadcast channel are
    // denied by name: the pasteboard (which Universal Clipboard would carry
    // off-device despite `(deny network*)`), the distributed notification
    // center (any process on the machine can subscribe), and syslog. This is a
    // denylist and therefore weaker than the file rules below; it is
    // defence-in-depth rather than the primary control. The primary control is
    // that this worker never logs, pastes, or broadcasts document-derived text
    // -- verified by there being no logging or IPC call of any kind in this
    // crate. Note os_log may reach the unified log without a mach-lookup, so
    // that discipline, not this profile, is what keeps text out of
    // /var/db/diagnostics.
    //
    // The previous profile was `(allow default)` with a few
    // subpath denies, which left the whole filesystem outside /Users,
    // /Volumes and /Network readable AND writable -- including $TMPDIR and
    // /private/tmp -- while this worker receives verbatim privileged
    // provision text. Reads are allowed only where Apple's built-in
    // NLEmbedding model and the dyld shared cache live; no write anywhere on
    // the filesystem is permitted, so document-derived text cannot be
    // persisted from here at all.
    //
    // `com.apple.system.logger` is the LEGACY ASL name. An independent
    // reviewer replicated this profile and confirmed that logd, diagnosticd
    // and launchservicesd stayed reachable through it -- and on macOS 26
    // os_log and NSLog go to logd, not ASL. This worker receives verbatim
    // privileged provision text and calls into Apple's NLEmbedding, which the
    // pilot does not control. No leak was observed (the reviewer searched the
    // unified log by message and by process and found nothing), but the
    // denies cost nothing because this crate never logs, and a channel that
    // is open only because nobody used it is not a control.
    const PROFILE: &CStr = c"(version 1)
(deny default)
(deny network*)
(allow process-info-pidinfo (target self))
(allow sysctl-read)
(allow mach-lookup)
(deny mach-lookup (global-name \"com.apple.pasteboard.1\"))
(deny mach-lookup (global-name \"com.apple.distributed_notifications@1v3\"))
(deny mach-lookup (global-name \"com.apple.distributed_notifications@Uv3\"))
(deny mach-lookup (global-name \"com.apple.system.logger\"))
(deny mach-lookup (global-name \"com.apple.logd\"))
(deny mach-lookup (global-name \"com.apple.logd.events\"))
(deny mach-lookup (global-name \"com.apple.diagnosticd\"))
(deny mach-lookup (global-name \"com.apple.analyticsd\"))
(deny mach-lookup (global-name \"com.apple.ReportCrash\"))
(deny mach-lookup (global-name \"com.apple.coreservices.launchservicesd\"))
(deny mach-lookup (global-name \"com.apple.system.notification_center\"))
(allow file-read-metadata)
(allow file-read* (subpath \"/System\"))
(allow file-read* (subpath \"/usr/lib\"))
(allow file-read* (subpath \"/usr/share\"))
(allow file-read-data (subpath \"/dev/fd\"))
(allow file-read-data (literal \"/dev/urandom\") (literal \"/dev/random\"))
(allow file-write-data (subpath \"/dev/fd\"))
";
    let mut error_buffer = ptr::null_mut();
    let status = unsafe { sandbox_init(PROFILE.as_ptr(), 0, &mut error_buffer) };
    if !error_buffer.is_null() {
        unsafe { sandbox_free_error(error_buffer) };
    }
    if status != 0 {
        return Err(SemanticError::SecurityBoundaryUnavailable);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_platform_sandbox() -> Result<(), SemanticError> {
    Err(SemanticError::SecurityBoundaryUnavailable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_similarity_vectors() {
        assert_eq!(
            cosine_similarity(&[1.0], &[1.0]),
            Err(SemanticError::InvalidVector)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pinned_builtin_model_returns_normalized_vectors() {
        let session = AppleSentenceEmbeddingSession::new().expect("built-in model");
        let vector = session
            .embed("The recipient shall not disclose proprietary information.")
            .expect("vector");
        let norm = vector.iter().map(|value| value * value).sum::<f32>();
        assert!((norm - 1.0).abs() < 0.0001);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn legal_paraphrase_ranks_above_unrelated_text() {
        let session = AppleSentenceEmbeddingSession::new().expect("built-in model");
        let query = session
            .embed("A party must keep client information confidential.")
            .expect("query");
        let paraphrase = session
            .embed("The recipient shall not disclose proprietary materials.")
            .expect("paraphrase");
        let unrelated = session
            .embed("A banana grows in a tropical climate.")
            .expect("unrelated");
        assert!(
            cosine_similarity(&query, &paraphrase).expect("similarity")
                > cosine_similarity(&query, &unrelated).expect("similarity")
        );
    }
}
