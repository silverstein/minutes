//! Diagnostic capture-to-final handoff. Never authorizes reuse of live text.
//! Positions describe the forwarded 16 kHz stream, not hardware-clock continuity.
#![cfg_attr(not(feature = "whisper"), allow(dead_code))]

use serde::Serialize;
use sha2::{Digest, Sha256};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum AudioSource {
    Unknown,
    Mixed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct AudioSpan {
    pub stream: u64,
    pub source: AudioSource,
    pub start_sample: u64,
    pub sample_count: u64,
}

impl AudioSpan {
    pub fn slice(&self, offset: usize, count: usize) -> Self {
        Self {
            start_sample: self.start_sample + offset as u64,
            sample_count: count as u64,
            ..self.clone()
        }
    }
}

pub(crate) fn append_span(spans: &mut Vec<AudioSpan>, span: AudioSpan) {
    if let Some(last) = spans.last_mut() {
        if last.stream == span.stream
            && last.source == span.source
            && last.start_sample.checked_add(last.sample_count) == Some(span.start_sample)
        {
            last.sample_count += span.sample_count;
            return;
        }
    }
    spans.push(span);
}

#[derive(Debug)]
pub(crate) struct AudioPacket {
    pub samples: Vec<f32>,
    pub span: Option<AudioSpan>,
}

/// Allocation and sends run after recording writes, never on the audio callback.
/// Advance before try_send so a full optional-consumer queue leaves a visible gap.
#[derive(Clone)]
pub(crate) struct LiveAudioSender {
    tx: mpsc::SyncSender<AudioPacket>,
    next_sample: Arc<AtomicU64>,
    stream: u64,
    source: AudioSource,
}

impl LiveAudioSender {
    pub fn new(tx: mpsc::SyncSender<AudioPacket>, source: AudioSource) -> Self {
        static NEXT_STREAM: AtomicU64 = AtomicU64::new(1);
        Self {
            tx,
            next_sample: Arc::new(AtomicU64::new(0)),
            stream: NEXT_STREAM.fetch_add(1, Ordering::Relaxed),
            source,
        }
    }

    fn packet(&self, samples: Vec<f32>) -> AudioPacket {
        let count = samples.len() as u64;
        AudioPacket {
            span: Some(AudioSpan {
                stream: self.stream,
                source: self.source,
                start_sample: self.next_sample.fetch_add(count, Ordering::Relaxed),
                sample_count: count,
            }),
            samples,
        }
    }

    pub fn try_send(&self, samples: Vec<f32>) -> Result<(), mpsc::TrySendError<AudioPacket>> {
        self.tx.try_send(self.packet(samples))
    }

    pub fn send(&self, samples: Vec<f32>) -> Result<(), mpsc::SendError<AudioPacket>> {
        self.tx.send(self.packet(samples))
    }
}

#[derive(Serialize)]
pub(crate) struct RecognitionProvenance {
    pub backend: String,
    pub model: String,
    pub language: Option<String>,
}

/// Bounded, text-free shadow file in the same private scratch slot as JSONL.
/// An IO failure disables only this diagnostic. No caller waits for it at capture.
pub(crate) struct CoverageLedger {
    writer: BufWriter<std::fs::File>,
    remaining: usize,
    failed: bool,
    expected: Option<AudioSpan>,
}

impl CoverageLedger {
    pub fn open(path: &std::path::Path, session: Option<&str>) -> std::io::Result<Self> {
        // Replace the scratch slot atomically. A late writer from the previous
        // session keeps its old inode and cannot interleave with the new file.
        let parent = path
            .parent()
            .ok_or_else(|| std::io::Error::other("coverage path has no parent"))?;
        let temporary = tempfile::Builder::new()
            .prefix(".live-coverage-")
            .tempfile_in(parent)?;
        let file = temporary.persist(path).map_err(|error| error.error)?;
        let mut ledger = Self {
            writer: BufWriter::new(file),
            remaining: 16 * 1024 * 1024,
            failed: false,
            expected: None,
        };
        ledger.record(serde_json::json!({
            "event": "manifest", "version": 1, "session_id": session,
            "sample_rate": 16000, "timeline": "forwarded-stream",
            "reuse_authorized": false, "source_file_binding": "unverified",
            "stream_id_scope": "this-manifest", "drafts": "excluded"
        }));
        Ok(ledger)
    }

    fn record(&mut self, event: serde_json::Value) {
        if self.failed {
            return;
        }
        let Ok(mut bytes) = serde_json::to_vec(&event) else {
            self.failed = true;
            return;
        };
        bytes.push(b'\n');
        if bytes.len() > self.remaining || self.writer.write_all(&bytes).is_err() {
            self.failed = true;
        } else {
            self.remaining -= bytes.len();
        }
    }

    pub fn observe(&mut self, span: Option<&AudioSpan>) {
        let Some(span) = span else {
            self.record(serde_json::json!({"event":"unpositioned_input"}));
            return;
        };
        if let Some(previous) = &self.expected {
            if previous.stream != span.stream || previous.source != span.source {
                self.record(serde_json::json!({"event":"stream_changed","span":span}));
            } else if previous.start_sample != span.start_sample {
                self.record(serde_json::json!({
                    "event":"gap_or_reorder", "expected_start":previous.start_sample,
                    "received":span
                }));
            }
        } else if span.start_sample != 0 {
            self.record(serde_json::json!({"event":"initial_gap","received":span}));
        }
        self.expected = span
            .start_sample
            .checked_add(span.sample_count)
            .map(|end| AudioSpan {
                start_sample: end,
                sample_count: 0,
                ..span.clone()
            });
        self.record(serde_json::json!({"event":"received","span":span}));
    }

    pub fn classified(&mut self, spans: &[AudioSpan], state: &str) {
        self.record(serde_json::json!({"event":"classification","state":state,"spans":spans}));
    }

    pub fn finalized(
        &mut self,
        spans: &[AudioSpan],
        line: usize,
        text: &str,
        provenance: &RecognitionProvenance,
    ) {
        self.record(serde_json::json!({
            "event":"finalized", "spans":spans, "transcript_line":line,
            "text_sha256":format!("{:x}", Sha256::digest(text.as_bytes())),
            "revision":1, "reuse_authorized":false, "recognition":provenance,
            "normalization":"live-transcript-v1", "model_binding":"configured-name-only"
        }));
    }

    pub fn finish(&mut self) {
        // The sender is not a capture-completion authority. In particular,
        // Stop may precede the writer's last batch and a full queue can lose tail.
        self.record(serde_json::json!({
            "event":"checkpoint", "last_received_end":self.expected,
            "capture_end":"unknown", "complete":false, "reuse_authorized":false
        }));
        if self.writer.flush().is_err() {
            self.failed = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dropped_audio_is_visible_in_the_next_position_and_never_blocks_recording() {
        let (tx, rx) = mpsc::sync_channel(1);
        let sender = LiveAudioSender::new(tx, AudioSource::Mixed);
        sender.try_send(vec![0.; 10]).unwrap();
        assert!(matches!(
            sender.try_send(vec![0.; 20]),
            Err(mpsc::TrySendError::Full(_))
        ));
        assert_eq!(rx.recv().unwrap().span.unwrap().start_sample, 0);
        sender.try_send(vec![0.; 30]).unwrap();
        assert_eq!(rx.recv().unwrap().span.unwrap().start_sample, 30);
    }

    #[test]
    fn disjoint_audio_and_changed_streams_never_merge_into_contiguous_speech() {
        let span = AudioSpan {
            stream: 1,
            source: AudioSource::Mixed,
            start_sample: 0,
            sample_count: 10,
        };
        let mut spans = vec![span.clone()];
        append_span(
            &mut spans,
            AudioSpan {
                start_sample: 10,
                ..span.clone()
            },
        );
        append_span(
            &mut spans,
            AudioSpan {
                start_sample: 30,
                ..span.clone()
            },
        );
        append_span(
            &mut spans,
            AudioSpan {
                stream: 2,
                start_sample: 40,
                ..span
            },
        );
        assert_eq!(
            spans.iter().map(|s| s.sample_count).collect::<Vec<_>>(),
            vec![20, 10, 10]
        );
    }

    #[test]
    fn ledger_records_gaps_finals_and_unknown_tail_without_text_or_reuse_authority() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coverage.jsonl");
        let mut ledger = CoverageLedger::open(&path, Some("fixture-session")).unwrap();
        let span = AudioSpan {
            stream: 1,
            source: AudioSource::Mixed,
            start_sample: 0,
            sample_count: 10,
        };
        ledger.observe(Some(&span));
        ledger.observe(Some(&AudioSpan {
            start_sample: 20,
            ..span.clone()
        }));
        ledger.finalized(
            &[span],
            7,
            "private fixture words",
            &RecognitionProvenance {
                backend: "fixture".into(),
                model: "fixture".into(),
                language: None,
            },
        );
        ledger.finish();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("private fixture words"));
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(rows.iter().any(|r| r["event"] == "gap_or_reorder"));
        assert!(rows
            .iter()
            .any(|r| r["event"] == "finalized" && r["transcript_line"] == 7));
        let end = rows.last().unwrap();
        assert_eq!(end["complete"], false);
        assert_eq!(end["capture_end"], "unknown");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn diagnostic_path_never_follows_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("preserved");
        std::fs::write(&target, "original bytes").unwrap();
        let path = dir.path().join("coverage.jsonl");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        let mut ledger = CoverageLedger::open(&path, None).unwrap();
        ledger.finish();
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original bytes");
    }

    #[test]
    fn a_late_old_writer_cannot_corrupt_the_next_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coverage.jsonl");
        let mut old = CoverageLedger::open(&path, Some("old-session")).unwrap();
        let mut current = CoverageLedger::open(&path, Some("new-session")).unwrap();
        old.finalized(
            &[],
            77,
            "old private words",
            &RecognitionProvenance {
                backend: "fixture".into(),
                model: "fixture".into(),
                language: None,
            },
        );
        old.finish();
        current.finish();
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("new-session"));
        assert!(!text.contains("old-session"));
        assert!(!text.contains("finalized"));
    }

    #[test]
    fn exhausted_diagnostic_budget_does_not_affect_final_transcript_or_claim_completion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coverage.jsonl");
        let mut ledger = CoverageLedger::open(&path, None).unwrap();
        ledger.remaining = 0;
        ledger.classified(&[], "failed");
        ledger.finish();
        assert!(ledger.failed);
        let text = std::fs::read_to_string(path).unwrap();
        assert!(!text.contains("checkpoint"));
    }
}
