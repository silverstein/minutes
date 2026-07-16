//! Live speaker attribution via the external `minutes-diarize-sidecar` process.
//!
//! Behind the optional `streaming-diarize` feature (off by default). This module
//! is the live-path *consumer* side: it spawns the sidecar binary, feeds it the
//! same 16 kHz mono PCM the live loop already captures, and reads back NDJSON
//! speaker segments. It deliberately has NO `parakeet-rs` / `ort` dependency: the
//! ort rc.12 / ndarray 0.17 those need conflicts with Minutes' `pyannote-rs`
//! (ort rc.10 / ndarray 0.16), so the diarizer lives in its own cargo workspace
//! and is reached only across the process boundary. This side is pure subprocess
//! IPC + JSON, which shares no dependency graph with the sidecar.
//!
//! Because streaming Sortformer lags ~10s, speaker labels arrive well after the
//! corresponding utterance was written. The backfill is therefore append-only:
//! `assign_speakers` maps already-written utterances to speakers by maximum time
//! overlap, and the live loop surfaces those assignments as later events rather
//! than mutating the JSONL line in place.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One diarized speaker segment, in milliseconds from session start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiarSegment {
    /// Segment start, ms from session start (absolute).
    pub start_ms: u64,
    /// Segment end, ms from session start (absolute).
    pub end_ms: u64,
    /// Diarizer speaker id (0..=3 for the 4-speaker Sortformer model).
    pub speaker: u32,
}

/// A running diarization sidecar process plus the background reader collecting
/// its segments. Drop kills the child; `finish` shuts it down cleanly.
pub struct StreamingDiarizeSidecar {
    child: Child,
    stdin: Option<ChildStdin>,
    segments: Arc<Mutex<Vec<DiarSegment>>>,
    reader: Option<JoinHandle<()>>,
}

impl StreamingDiarizeSidecar {
    /// Spawn the sidecar binary against `model` (a Sortformer `.onnx`). The
    /// child's stderr is inherited so its logs surface in the parent's logs.
    pub fn spawn(bin: &Path, model: &Path) -> std::io::Result<Self> {
        let mut child = crate::engine_process::command(bin)
            .arg("--model")
            .arg(model)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| std::io::Error::other("sidecar stdout not piped"))?;
        let segments = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&segments);
        // Dedicated reader thread: satisfies the sidecar's full-duplex contract
        // (stdout must be drained concurrently with feeding stdin).
        let reader = std::thread::spawn(move || {
            let buf = BufReader::new(stdout);
            for line in buf.lines() {
                let Ok(line) = line else { break };
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if v.get("event").is_some() {
                    continue; // ready / flush_done markers
                }
                if let (Some(s), Some(e), Some(sp)) = (
                    v.get("start_ms").and_then(serde_json::Value::as_u64),
                    v.get("end_ms").and_then(serde_json::Value::as_u64),
                    v.get("speaker").and_then(serde_json::Value::as_u64),
                ) {
                    if let Ok(mut g) = sink.lock() {
                        g.push(DiarSegment {
                            start_ms: s,
                            end_ms: e,
                            speaker: sp as u32,
                        });
                    }
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            segments,
            reader: Some(reader),
        })
    }

    /// Feed a chunk of 16 kHz mono f32 PCM. Best-effort: if the sidecar has died,
    /// the write fails, stdin is dropped, and feeding silently becomes a no-op so
    /// diarization loss never interrupts the recording.
    pub fn feed_audio(&mut self, samples: &[f32]) {
        let Some(stdin) = self.stdin.as_mut() else {
            return;
        };
        let mut frame = Vec::with_capacity(4 + samples.len() * 4);
        frame.extend_from_slice(&((samples.len() * 4) as u32).to_le_bytes());
        for s in samples {
            frame.extend_from_slice(&s.to_le_bytes());
        }
        if stdin.write_all(&frame).is_err() {
            self.stdin = None;
        }
    }

    /// Snapshot of segments collected so far (diarization lags real time, so this
    /// trails the live audio by roughly the model's latency).
    pub fn segments(&self) -> Vec<DiarSegment> {
        self.segments.lock().map(|g| g.clone()).unwrap_or_default()
    }

    /// Close stdin (signals the sidecar to flush + exit), join the reader, and
    /// return all segments including the flushed tail.
    pub fn finish(mut self) -> Vec<DiarSegment> {
        self.stdin = None; // dropping ChildStdin closes the pipe -> sidecar EOF
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        let _ = self.child.wait();
        self.segments()
    }
}

impl Drop for StreamingDiarizeSidecar {
    fn drop(&mut self) {
        // If the caller did not `finish()`, make sure we don't leak the child.
        self.stdin = None;
        let _ = self.child.kill();
    }
}

/// Assign each utterance the speaker whose diarized segments overlap it most.
///
/// `utterances` is `(line, start_ms, end_ms)`; returns `(line, speaker)` only for
/// utterances with any overlapping segment (others stay unassigned until more
/// segments arrive). Pure function: the live loop calls this against the current
/// segment snapshot and emits the new assignments as append-only events.
pub fn assign_speakers(
    utterances: &[(u64, u64, u64)],
    segments: &[DiarSegment],
) -> Vec<(u64, u32)> {
    let mut out = Vec::new();
    for &(line, u_start, u_end) in utterances {
        let mut by_speaker: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
        for seg in segments {
            let ov = overlap_ms(u_start, u_end, seg.start_ms, seg.end_ms);
            if ov > 0 {
                *by_speaker.entry(seg.speaker).or_insert(0) += ov;
            }
        }
        // Pick the speaker with the most overlap; ties break to the lower id for
        // determinism.
        let best = by_speaker
            .into_iter()
            .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)));
        if let Some((speaker, _)) = best {
            out.push((line, speaker));
        }
    }
    out
}

/// Overlap of `[a0, a1)` and `[b0, b1)` in ms (0 if disjoint).
fn overlap_ms(a0: u64, a1: u64, b0: u64, b1: u64) -> u64 {
    let lo = a0.max(b0);
    let hi = a1.min(b1);
    hi.saturating_sub(lo)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(start_ms: u64, end_ms: u64, speaker: u32) -> DiarSegment {
        DiarSegment {
            start_ms,
            end_ms,
            speaker,
        }
    }

    #[test]
    fn overlap_basic() {
        assert_eq!(overlap_ms(0, 100, 50, 150), 50);
        assert_eq!(overlap_ms(0, 100, 100, 200), 0); // touching, not overlapping
        assert_eq!(overlap_ms(0, 100, 200, 300), 0); // disjoint
        assert_eq!(overlap_ms(50, 150, 0, 1000), 100); // contained
    }

    #[test]
    fn assigns_max_overlap_speaker() {
        // utterance [1000,3000): speaker 0 overlaps 500ms, speaker 1 overlaps 1500ms.
        let utts = [(7u64, 1000u64, 3000u64)];
        let segs = [seg(500, 1500, 0), seg(1500, 3200, 1)];
        assert_eq!(assign_speakers(&utts, &segs), vec![(7, 1)]);
    }

    #[test]
    fn unassigned_when_no_overlap() {
        let utts = [(1u64, 0u64, 1000u64)];
        let segs = [seg(5000, 6000, 2)];
        assert!(assign_speakers(&utts, &segs).is_empty());
    }

    #[test]
    fn tie_breaks_to_lower_speaker_id() {
        // equal overlap (500ms each) -> deterministic lower id wins.
        let utts = [(3u64, 0u64, 1000u64)];
        let segs = [seg(0, 500, 2), seg(500, 1000, 1)];
        assert_eq!(assign_speakers(&utts, &segs), vec![(3, 1)]);
    }
}
