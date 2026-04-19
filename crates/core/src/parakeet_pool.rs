//! Parallel chunk transcription worker pool for Parakeet.
//!
//! Each worker owns its own `ParakeetSidecarManager` backed by a distinct
//! example-server subprocess with a distinct Unix socket. Callers construct
//! a pool sized to `min(num_chunks, config.chunk_workers)`, submit chunks,
//! and receive results in completion order (not input order — assembly is
//! the caller's job).
//!
//! Memory cost scales linearly with `worker_count`: each worker loads a
//! full parakeet model replica (~1.2 GB for tdt-600m). Callers should size
//! the pool based on available RAM and GPU contention, not chunk count
//! alone.
//!
//! Only available when the `parakeet` feature is enabled.

#![cfg(all(feature = "parakeet", unix))]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use crossbeam_channel::{bounded, Receiver, Sender};

use crate::config::Config;
use crate::events::{append_event, MinutesEvent};
use crate::parakeet_sidecar::{
    build_launch_spec, build_request, ParakeetSidecarManager, SidecarError, SidecarLaunchSpec,
    SidecarTranscriptResult,
};
use crate::transcribe::{write_wav_16k_mono, DecodeHints};

/// Unit of work dispatched to a worker.
pub struct ChunkJob {
    /// Zero-based index of this chunk in the input audio.
    pub index: u32,
    /// Sample range in the original audio (for timestamp offset + event).
    pub start_sample: usize,
    pub end_sample: usize,
    /// Owned, pre-extracted 16 kHz mono samples for this chunk.
    pub samples: Vec<f32>,
}

/// Successful worker output — transcript + timing metadata.
pub struct ChunkSuccess {
    pub index: u32,
    pub start_sample: usize,
    pub end_sample: usize,
    pub result: SidecarTranscriptResult,
    pub elapsed_wall_ms: u64,
}

pub type ChunkOutcome = Result<ChunkSuccess, (u32, SidecarError)>;

/// Owned pool of Parakeet sidecars. Dropping the pool shuts every worker
/// via `ParakeetSidecarManager::Drop`.
pub struct ParakeetPool {
    managers: Vec<ParakeetSidecarManager>,
    specs: Vec<SidecarLaunchSpec>,
}

impl ParakeetPool {
    /// Build a pool with `worker_count` distinct sidecar managers, each with
    /// its own example-server subprocess and Unix socket.
    ///
    /// The pool is NOT pre-warmed — the first request on each worker pays
    /// the cold-start cost. That's intentional: if the caller decides not
    /// to use a given worker (e.g., fewer chunks than workers), no sidecar
    /// gets launched at all.
    pub fn new(
        config: &Config,
        model_path: &Path,
        vocab_path: &Path,
        vad_path: Option<&Path>,
        worker_count: usize,
    ) -> Result<Self, SidecarError> {
        if worker_count == 0 {
            return Err(SidecarError::new(
                "parakeet_pool: worker_count must be >= 1",
            ));
        }
        let base_spec = build_launch_spec(config, model_path, vocab_path, vad_path)?;
        let mut managers = Vec::with_capacity(worker_count);
        let mut specs = Vec::with_capacity(worker_count);
        for worker_id in 0..worker_count {
            managers.push(ParakeetSidecarManager::default());
            specs.push(spec_for_worker(&base_spec, worker_id));
        }
        Ok(Self { managers, specs })
    }

    pub fn worker_count(&self) -> usize {
        self.managers.len()
    }

    /// Submit `jobs` to the pool and invoke `on_complete` for each finished
    /// chunk (successful or failed). Jobs are processed in parallel across
    /// workers but `on_complete` is called from the caller's thread in the
    /// order chunks complete.
    ///
    /// Returns when every job has been processed. Uses `std::thread::scope`
    /// so workers cannot outlive the borrow on `self`.
    pub fn run_chunks<F>(
        &mut self,
        jobs: Vec<ChunkJob>,
        config: &Config,
        hints: &DecodeHints,
        mut on_complete: F,
    ) -> Result<Vec<ChunkOutcome>, SidecarError>
    where
        F: FnMut(&ChunkOutcome),
    {
        let total = jobs.len();
        if total == 0 {
            return Ok(Vec::new());
        }
        let active_workers = total.min(self.managers.len());
        let (job_tx, job_rx): (Sender<ChunkJob>, Receiver<ChunkJob>) = bounded(total);
        let (result_tx, result_rx): (Sender<ChunkOutcome>, Receiver<ChunkOutcome>) = bounded(total);

        for job in jobs {
            job_tx
                .send(job)
                .expect("pool channel closed before dispatch");
        }
        drop(job_tx);

        let managers = &mut self.managers[..active_workers];
        let specs = &self.specs[..active_workers];
        let request_counter = AtomicU64::new(0);

        std::thread::scope(|scope| {
            for (manager, spec) in managers.iter_mut().zip(specs.iter()) {
                let job_rx = job_rx.clone();
                let result_tx = result_tx.clone();
                let request_counter = &request_counter;
                scope.spawn(move || {
                    worker_loop(
                        manager,
                        spec,
                        config,
                        hints,
                        request_counter,
                        job_rx,
                        result_tx,
                    );
                });
            }
            drop(job_rx);
            drop(result_tx);
        });

        let mut outcomes = Vec::with_capacity(total);
        while let Ok(outcome) = result_rx.recv() {
            on_complete(&outcome);
            outcomes.push(outcome);
        }
        Ok(outcomes)
    }
}

/// Build a per-worker launch spec by varying the socket path. Everything
/// else (model paths, GPU/FP16 flags, VAD) stays identical to the base
/// spec so the workers produce byte-identical output.
fn spec_for_worker(base: &SidecarLaunchSpec, worker_id: usize) -> SidecarLaunchSpec {
    let mut spec = base.clone();
    spec.socket_path = worker_socket_path(&base.socket_path, worker_id);
    spec
}

fn worker_socket_path(base: &Path, worker_id: usize) -> PathBuf {
    let parent = base.parent().unwrap_or_else(|| Path::new("."));
    let stem = base
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("parakeet-sidecar");
    let ext = base.extension().and_then(|s| s.to_str()).unwrap_or("sock");
    parent.join(format!("{stem}-w{worker_id}.{ext}"))
}

fn worker_loop(
    manager: &mut ParakeetSidecarManager,
    spec: &SidecarLaunchSpec,
    config: &Config,
    hints: &DecodeHints,
    request_counter: &AtomicU64,
    job_rx: Receiver<ChunkJob>,
    result_tx: Sender<ChunkOutcome>,
) {
    while let Ok(job) = job_rx.recv() {
        let outcome = run_one_chunk(manager, spec, config, hints, request_counter, &job);
        // Channel closes only after the scope ends; send failures mean the
        // consumer is gone, which shouldn't happen within `run_chunks`.
        if result_tx.send(outcome).is_err() {
            break;
        }
    }
}

fn run_one_chunk(
    manager: &mut ParakeetSidecarManager,
    spec: &SidecarLaunchSpec,
    config: &Config,
    hints: &DecodeHints,
    request_counter: &AtomicU64,
    job: &ChunkJob,
) -> ChunkOutcome {
    let tmp_wav = match make_chunk_wav(request_counter, &job.samples) {
        Ok(path) => path,
        Err(e) => return Err((job.index, SidecarError::new(e.to_string()))),
    };
    let started = Instant::now();
    let request = match build_request(manager, config, &tmp_wav, spec.vad_path.is_some(), hints) {
        Ok(req) => req,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_wav);
            return Err((job.index, e));
        }
    };
    let audio_duration_secs = job.samples.len() as f64 / 16_000.0;
    let result = manager.transcribe(spec.clone(), request, config, audio_duration_secs);
    let elapsed_wall_ms = started.elapsed().as_millis() as u64;
    let _ = std::fs::remove_file(&tmp_wav);

    match result {
        Ok(res) => Ok(ChunkSuccess {
            index: job.index,
            start_sample: job.start_sample,
            end_sample: job.end_sample,
            result: res,
            elapsed_wall_ms,
        }),
        Err(e) => Err((job.index, e)),
    }
}

fn make_chunk_wav(request_counter: &AtomicU64, samples: &[f32]) -> Result<PathBuf, SidecarError> {
    let pid = std::process::id();
    let seq = request_counter.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("minutes-pool-chunk-{pid}-{seq}.wav"));
    write_wav_16k_mono(&tmp, samples)
        .map_err(|e| SidecarError::new(format!("failed to write chunk WAV: {}", e)))?;
    Ok(tmp)
}

/// Emit a `TranscribeChunkCompleted` event for a successful chunk. Factored
/// out so callers (e.g., the batch assembly loop in `transcribe.rs`) can
/// attach the audio path and total chunk count — the pool itself only knows
/// the chunk's own coordinates.
pub fn emit_chunk_completed_event(
    audio_path: &Path,
    chunk: &ChunkSuccess,
    chunk_count: u32,
    engine: &str,
) {
    let word_count = chunk
        .result
        .transcript
        .transcript
        .split_whitespace()
        .count();
    append_event(MinutesEvent::TranscribeChunkCompleted {
        audio_path: audio_path.to_string_lossy().to_string(),
        chunk_index: chunk.index,
        chunk_count,
        start_sec: chunk.start_sample as f64 / 16_000.0,
        end_sec: chunk.end_sample as f64 / 16_000.0,
        words: word_count,
        duration_ms: chunk.elapsed_wall_ms,
        engine: engine.to_string(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_socket_path_is_unique_per_worker() {
        let base = Path::new("/tmp/foo/parakeet-sidecar-123-tdt.sock");
        let p0 = worker_socket_path(base, 0);
        let p1 = worker_socket_path(base, 1);
        let p2 = worker_socket_path(base, 2);
        assert_ne!(p0, p1);
        assert_ne!(p1, p2);
        assert!(p0.to_string_lossy().contains("-w0."));
        assert!(p1.to_string_lossy().contains("-w1."));
        assert!(p2.to_string_lossy().contains("-w2."));
    }

    #[test]
    fn worker_socket_path_keeps_extension() {
        let base = Path::new("/tmp/parakeet-sidecar-pid-model.sock");
        let p = worker_socket_path(base, 3);
        assert_eq!(p.extension().and_then(|e| e.to_str()), Some("sock"));
    }
}
