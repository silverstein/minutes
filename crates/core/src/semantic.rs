//! Local semantic search: the pinned embedding model and the client for the
//! isolated embedding worker (`crates/embed-worker`).
//!
//! The worker runs Google's EmbeddingGemma 2 through llama.cpp in its own
//! process. This module installs the model it needs, finds and starts the
//! worker, and turns its line-delimited JSON protocol into typed calls with
//! deadlines, so a stuck or crashed worker becomes an error rather than a hang.
use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// EmbeddingGemma 2, Q8_0 GGUF (Apache 2.0), pinned to one Hugging Face
/// revision; the digest is verified before the file is used.
const MODEL_URL: &str = "https://huggingface.co/unsloth/embeddinggemma-2-GGUF/resolve/031f0d4b35536f69ab3509d4893c923264fcf253/embeddinggemma-2-Q8_0.gguf";
const MODEL_SHA256: &str = "6f1bd4ac6c5df7444f9cca7ca36cafe6cfa34cd6f49fefb1e0b4be8143aed8bc";
const MODEL_BYTES: u64 = 309_855_520;
const MODEL_DIRECTORY: &str = "embedding/embeddinggemma-2-q8_0";
const MODEL_FILENAME: &str = "embeddinggemma-2-Q8_0.gguf";

/// Stored vector width. EmbeddingGemma 2 is Matryoshka-trained: 256 of its
/// 768 dimensions retrieved exactly as well on Mat's corpus, at a third of
/// the size.
pub const DIMENSIONS: usize = 256;

/// Executable name, beside the app's own binaries.
pub const WORKER_FILENAME: &str = "minutes-embed-worker";

const STARTUP_DEADLINE: Duration = Duration::from_secs(60);
const QUERY_DEADLINE: Duration = Duration::from_secs(15);
const DOCUMENTS_DEADLINE: Duration = Duration::from_secs(120);
const MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum SemanticError {
    #[error("the semantic search worker is not installed")]
    WorkerUnavailable,
    #[error("the semantic search model is not installed")]
    ModelUnavailable,
    #[error("could not start the semantic search worker: {0}")]
    Spawn(std::io::Error),
    #[error("the semantic search worker did not answer in time")]
    Timeout,
    #[error("the semantic search worker stopped")]
    Stopped,
    #[error("the semantic search worker reported: {0}")]
    Worker(String),
    #[error("the semantic search worker sent an invalid reply: {0}")]
    Protocol(String),
}

/// Where the model lives under the transcription model root.
pub fn model_path(model_root: &Path) -> PathBuf {
    model_root.join(MODEL_DIRECTORY).join(MODEL_FILENAME)
}

/// Cheap presence check (size only); [`install_model`] verifies the digest.
pub fn model_installed(model_root: &Path) -> bool {
    std::fs::metadata(model_path(model_root)).is_ok_and(|meta| meta.len() == MODEL_BYTES)
}

/// Download and verify the pinned model. A verified file is reused without
/// any network request; a failed download never replaces a good file.
pub fn install_model(model_root: &Path) -> Result<PathBuf, String> {
    install_with(
        &model_root.join(MODEL_DIRECTORY),
        MODEL_SHA256,
        MODEL_BYTES,
        || {
            let response = ureq::get(MODEL_URL)
                .config()
                .timeout_connect(Some(Duration::from_secs(15)))
                .timeout_recv_response(Some(Duration::from_secs(30)))
                .timeout_global(Some(Duration::from_secs(30 * 60)))
                .build()
                .call()
                .map_err(|e| format!("semantic model download failed: {e}"))?;
            Ok(Box::new(response.into_body().into_reader()) as Box<dyn Read>)
        },
    )
}

fn install_with(
    dir: &Path,
    sha256: &str,
    bytes: u64,
    fetch: impl FnOnce() -> Result<Box<dyn Read>, String>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".install.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock_exclusive().map_err(|e| e.to_string())?;
    let dest = dir.join(MODEL_FILENAME);
    if verified(&dest, sha256, bytes)? {
        return Ok(dest);
    }
    let mut staging = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    let mut body = fetch()?.take(bytes + 1);
    std::io::copy(&mut body, &mut staging).map_err(|e| e.to_string())?;
    staging.flush().map_err(|e| e.to_string())?;
    if !verified(staging.path(), sha256, bytes)? {
        return Err("semantic model checksum mismatch".into());
    }
    staging.persist(&dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

fn verified(path: &Path, sha256: &str, bytes: u64) -> Result<bool, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|e| e.to_string())?.len() != bytes {
        return Ok(false);
    }
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", digest.finalize()) == sha256)
}

/// The worker executable: `MINUTES_EMBED_WORKER`, else beside this binary
/// (inside the app bundle that is `Contents/MacOS`).
pub fn worker_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("MINUTES_EMBED_WORKER").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe()
        .ok()?
        .parent()?
        .join(WORKER_FILENAME);
    beside.is_file().then_some(beside)
}

/// A running embedding worker. Dropping it stops the process.
pub struct EmbedWorker {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<Result<String, String>>,
    dimensions: usize,
}

#[derive(serde::Deserialize)]
struct Reply {
    #[serde(default)]
    ready: bool,
    #[serde(default)]
    dimensions: Option<usize>,
    #[serde(default)]
    vectors: Option<Vec<Vec<f32>>>,
    #[serde(default)]
    error: Option<String>,
}

impl EmbedWorker {
    /// Start the worker with the model and wait until it is ready.
    pub fn spawn(worker: &Path, model: &Path, dimensions: usize) -> Result<Self, SemanticError> {
        Self::spawn_command(
            crate::engine_process::command(worker)
                .arg("--model")
                .arg(model)
                .arg("--dimensions")
                .arg(dimensions.to_string()),
            dimensions,
        )
    }

    fn spawn_command(command: &mut Command, dimensions: usize) -> Result<Self, SemanticError> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(SemanticError::Spawn)?;
        let stdin = child.stdin.take().ok_or(SemanticError::Stopped)?;
        let stdout = child.stdout.take().ok_or(SemanticError::Stopped)?;
        let (sender, replies) = mpsc::channel();
        std::thread::Builder::new()
            .name("embed-worker-reader".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut line = String::new();
                    // Bound each reply so a misbehaving worker cannot exhaust memory.
                    match (&mut reader).take(MAX_RESPONSE_BYTES).read_line(&mut line) {
                        Ok(0) => return,
                        Ok(_) if !line.ends_with('\n') => {
                            let _ = sender.send(Err("reply too large".into()));
                            return;
                        }
                        Ok(_) => {
                            if sender.send(Ok(line)).is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = sender.send(Err(error.to_string()));
                            return;
                        }
                    }
                }
            })
            .map_err(SemanticError::Spawn)?;
        let mut worker = Self {
            child,
            stdin,
            replies,
            dimensions,
        };
        let ready = worker.receive(STARTUP_DEADLINE)?;
        if let Some(error) = ready.error {
            return Err(SemanticError::Worker(error));
        }
        if !ready.ready || ready.dimensions != Some(dimensions) {
            return Err(SemanticError::Protocol("unexpected startup reply".into()));
        }
        Ok(worker)
    }

    /// Vectors for (title, text) documents, in order.
    pub fn embed_documents(
        &mut self,
        documents: &[(&str, &str)],
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let documents: Vec<_> = documents
            .iter()
            .map(|(title, text)| serde_json::json!({ "title": title, "text": text }))
            .collect();
        let count = documents.len();
        let vectors = self.request(
            &serde_json::json!({ "documents": documents }),
            DOCUMENTS_DEADLINE,
        )?;
        if vectors.len() != count {
            return Err(SemanticError::Protocol(format!(
                "{} vectors for {count} documents",
                vectors.len()
            )));
        }
        Ok(vectors)
    }

    /// Vector for a search query.
    pub fn embed_query(&mut self, query: &str) -> Result<Vec<f32>, SemanticError> {
        self.request(&serde_json::json!({ "query": query }), QUERY_DEADLINE)?
            .pop()
            .ok_or_else(|| SemanticError::Protocol("no vector for the query".into()))
    }

    fn request(
        &mut self,
        body: &serde_json::Value,
        deadline: Duration,
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        let mut line =
            serde_json::to_vec(body).map_err(|e| SemanticError::Protocol(e.to_string()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.flush())
            .map_err(|_| SemanticError::Stopped)?;
        let reply = self.receive(deadline)?;
        if let Some(error) = reply.error {
            return Err(SemanticError::Worker(error));
        }
        let vectors = reply
            .vectors
            .ok_or_else(|| SemanticError::Protocol("reply without vectors".into()))?;
        if vectors.iter().any(|vector| vector.len() != self.dimensions) {
            return Err(SemanticError::Protocol("vector has the wrong width".into()));
        }
        Ok(vectors)
    }

    fn receive(&mut self, deadline: Duration) -> Result<Reply, SemanticError> {
        let line = match self.replies.recv_timeout(deadline) {
            Ok(Ok(line)) => line,
            Ok(Err(error)) => return Err(SemanticError::Protocol(error)),
            Err(RecvTimeoutError::Timeout) => {
                // A late reply would desynchronize every later request.
                let _ = self.child.kill();
                return Err(SemanticError::Timeout);
            }
            Err(RecvTimeoutError::Disconnected) => return Err(SemanticError::Stopped),
        };
        serde_json::from_str(&line).map_err(|e| SemanticError::Protocol(e.to_string()))
    }
}

/// Something that turns text into vectors: a worker this caller owns, or the
/// process-wide [`SharedEmbedder`].
pub trait Embedder {
    fn embed_documents(
        &mut self,
        documents: &[(&str, &str)],
    ) -> Result<Vec<Vec<f32>>, SemanticError>;
    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>, SemanticError>;
}

impl Embedder for EmbedWorker {
    fn embed_documents(
        &mut self,
        documents: &[(&str, &str)],
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        EmbedWorker::embed_documents(self, documents)
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>, SemanticError> {
        EmbedWorker::embed_query(self, query)
    }
}

/// Stop the shared worker after this long unused: the model holds ~300 MB.
const SHARED_IDLE: Duration = Duration::from_secs(10 * 60);

struct SharedState {
    worker: Option<EmbedWorker>,
    last_used: Instant,
}

fn shared_state() -> &'static Mutex<SharedState> {
    static STATE: OnceLock<Mutex<SharedState>> = OnceLock::new();
    STATE.get_or_init(|| {
        std::thread::Builder::new()
            .name("embed-worker-idle".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(60));
                if let Ok(mut state) = shared_state().lock() {
                    if state.worker.is_some() && state.last_used.elapsed() > SHARED_IDLE {
                        state.worker = None;
                    }
                }
            })
            .ok();
        Mutex::new(SharedState {
            worker: None,
            last_used: Instant::now(),
        })
    })
}

/// One warm worker for the whole process, started on first use and stopped
/// when idle. Each call holds it only for one request, so a search can run
/// between the batches of a long sync. A worker that fails is discarded and
/// restarted on the next call.
pub struct SharedEmbedder {
    model_root: PathBuf,
}

impl SharedEmbedder {
    pub fn new(model_root: &Path) -> Self {
        Self {
            model_root: model_root.to_path_buf(),
        }
    }

    fn with_worker<T>(
        &self,
        call: impl FnOnce(&mut EmbedWorker) -> Result<T, SemanticError>,
    ) -> Result<T, SemanticError> {
        let mut state = shared_state().lock().map_err(|_| SemanticError::Stopped)?;
        if state.worker.is_none() {
            let worker = worker_path().ok_or(SemanticError::WorkerUnavailable)?;
            if !model_installed(&self.model_root) {
                return Err(SemanticError::ModelUnavailable);
            }
            state.worker = Some(EmbedWorker::spawn(
                &worker,
                &model_path(&self.model_root),
                DIMENSIONS,
            )?);
        }
        state.last_used = Instant::now();
        let worker = state.worker.as_mut().ok_or(SemanticError::Stopped)?;
        let result = call(worker);
        if matches!(
            result,
            Err(SemanticError::Stopped | SemanticError::Timeout | SemanticError::Protocol(_))
        ) {
            state.worker = None;
        }
        result
    }
}

impl Embedder for SharedEmbedder {
    fn embed_documents(
        &mut self,
        documents: &[(&str, &str)],
    ) -> Result<Vec<Vec<f32>>, SemanticError> {
        self.with_worker(|worker| worker.embed_documents(documents))
    }

    fn embed_query(&mut self, query: &str) -> Result<Vec<f32>, SemanticError> {
        self.with_worker(|worker| worker.embed_query(query))
    }
}

impl Drop for EmbedWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in worker: answers every request with fixed 2-d vectors, one
    /// per line it is sent (one vector per document for a two-document batch).
    fn fake_worker(script: &str) -> Command {
        let mut command = crate::engine_process::command("/bin/sh");
        command.arg("-c").arg(script);
        command
    }

    #[test]
    fn worker_round_trip_and_width_check() {
        let script = r#"echo '{"ready":true,"dimensions":2}'
read q; echo '{"vectors":[[0.6,0.8]]}'
read d; echo '{"vectors":[[1,0],[0,1]]}'
read bad; echo '{"vectors":[[1,0,0]]}'
read err; echo '{"error":"nope"}'"#;
        let mut worker = EmbedWorker::spawn_command(&mut fake_worker(script), 2).unwrap();
        assert_eq!(worker.embed_query("q").unwrap(), vec![0.6, 0.8]);
        assert_eq!(
            worker
                .embed_documents(&[("a", "x"), ("b", "y")])
                .unwrap()
                .len(),
            2
        );
        assert!(matches!(
            worker.embed_query("q"),
            Err(SemanticError::Protocol(_))
        ));
        assert!(matches!(worker.embed_query("q"), Err(SemanticError::Worker(e)) if e == "nope"));
    }

    #[test]
    fn startup_errors_and_wrong_dimensions_are_refused() {
        let failing = r#"echo '{"error":"load model: bad file"}'; exit 1"#;
        assert!(matches!(
            EmbedWorker::spawn_command(&mut fake_worker(failing), 2),
            Err(SemanticError::Worker(e)) if e.contains("bad file")
        ));
        let wrong = r#"echo '{"ready":true,"dimensions":3}'"#;
        assert!(matches!(
            EmbedWorker::spawn_command(&mut fake_worker(wrong), 2),
            Err(SemanticError::Protocol(_))
        ));
    }

    #[test]
    fn a_worker_that_exits_is_reported_as_stopped() {
        let script = r#"echo '{"ready":true,"dimensions":2}'; exit 0"#;
        let mut worker = EmbedWorker::spawn_command(&mut fake_worker(script), 2).unwrap();
        assert!(matches!(
            worker.embed_query("q"),
            Err(SemanticError::Stopped | SemanticError::Protocol(_))
        ));
    }

    /// Real worker and model: `MINUTES_EMBED_WORKER=<binary>
    /// MINUTES_EMBED_MODEL=<gguf> cargo test -p minutes-core --lib
    /// semantic::tests::real_worker -- --ignored`.
    #[test]
    #[ignore]
    fn real_worker_ranks_the_meant_document_first() {
        let worker = PathBuf::from(std::env::var_os("MINUTES_EMBED_WORKER").unwrap());
        let model = PathBuf::from(std::env::var_os("MINUTES_EMBED_MODEL").unwrap());
        let mut worker = EmbedWorker::spawn(&worker, &model, DIMENSIONS).unwrap();
        let docs = worker
            .embed_documents(&[
                (
                    "Mercury Payroll Onboarding",
                    "We decided to move payroll from Gusto to Mercury.",
                ),
                ("Pill ads", "Brainstorm about a broken pill container ad."),
            ])
            .unwrap();
        let query = worker
            .embed_query("switching our payroll provider")
            .unwrap();
        let dot = |v: &[f32]| v.iter().zip(&query).map(|(a, b)| a * b).sum::<f32>();
        assert!(
            dot(&docs[0]) > dot(&docs[1]) + 0.1,
            "{} vs {}",
            dot(&docs[0]),
            dot(&docs[1])
        );
    }

    #[test]
    fn model_install_verifies_and_reuses_without_fetching() {
        let temp = tempfile::tempdir().unwrap();
        let body = b"pretend gguf".to_vec();
        let sha = format!("{:x}", Sha256::digest(&body));
        let fetched = body.clone();
        let path = install_with(temp.path(), &sha, body.len() as u64, move || {
            Ok(Box::new(std::io::Cursor::new(fetched)) as Box<dyn Read>)
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), body);
        install_with(temp.path(), &sha, body.len() as u64, || {
            panic!("a verified model was downloaded again")
        })
        .unwrap();
    }

    #[test]
    fn a_corrupt_download_never_replaces_the_model() {
        let temp = tempfile::tempdir().unwrap();
        let good = b"good model".to_vec();
        let sha = format!("{:x}", Sha256::digest(&good));
        let result = install_with(temp.path(), &sha, good.len() as u64, || {
            Ok(Box::new(std::io::Cursor::new(b"evil model".to_vec())) as Box<dyn Read>)
        });
        assert!(result.is_err());
        assert!(!temp.path().join(MODEL_FILENAME).exists());
    }
}
