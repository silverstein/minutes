//! Diagnostic capture-to-final handoff. Never authorizes reuse of live text.
//! Positions describe the forwarded 16 kHz stream, not hardware-clock continuity.
// Capture's optional sender type exists in every build, while its live consumer
// requires both features. Keep unused helpers quiet only without that consumer.
#![cfg_attr(not(all(feature = "streaming", feature = "whisper")), allow(dead_code))]

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

/// Positions in one stem's decoded 16 kHz stream, before mixing. Roles name
/// caller-supplied file slots, never a verified device or speaker identity.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum StemRole {
    Voice,
    System,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourceRange {
    pub epoch: u64,
    pub role: StemRole,
    pub source_start_sample: u64,
    pub output_start_sample: u64,
    pub sample_count: u64,
}

/// A hash of complete PCM frames already consumed by the tailer. It excludes
/// the mutable WAV header. Validation against the preserved file is still
/// required, including its final length; this is not a capture-end checkpoint.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct SourcePrefix {
    pub epoch: u64,
    pub role: StemRole,
    pub locator_sha256: Option<String>,
    pub sample_rate: u32,
    pub channels: u16,
    pub encoding: &'static str,
    pub data_offset: u64,
    pub pcm_bytes: u64,
    pub pcm_sha256: String,
    pub decoded_samples: u64,
    pub invalidated: Option<&'static str>,
}

#[derive(Clone, Debug, Serialize, Default)]
pub(crate) struct PacketSources {
    pub ranges: Vec<SourceRange>,
    pub prefixes: Vec<SourcePrefix>,
}

#[derive(Debug)]
pub(crate) struct AudioPacket {
    pub samples: Vec<f32>,
    pub span: Option<AudioSpan>,
    pub sources: Option<PacketSources>,
}

/// Allocation and sends run after recording writes, never on the audio callback.
/// Advance before try_send so a full optional-consumer queue leaves a visible gap.
#[derive(Clone)]
pub(crate) struct LiveAudioSender {
    tx: mpsc::SyncSender<AudioPacket>,
    next_sample: Arc<AtomicU64>,
    stream: u64,
    source: AudioSource,
    track_sources: bool,
}

impl LiveAudioSender {
    pub fn new(tx: mpsc::SyncSender<AudioPacket>, source: AudioSource) -> Self {
        static NEXT_STREAM: AtomicU64 = AtomicU64::new(1);
        Self {
            tx,
            next_sample: Arc::new(AtomicU64::new(0)),
            stream: NEXT_STREAM.fetch_add(1, Ordering::Relaxed),
            source,
            track_sources: false,
        }
    }

    pub fn with_source_tracking(mut self, enabled: bool) -> Self {
        self.track_sources = enabled;
        self
    }

    pub fn tracks_sources(&self) -> bool {
        self.track_sources
    }

    fn packet(&self, samples: Vec<f32>, sources: Option<PacketSources>) -> AudioPacket {
        let count = samples.len() as u64;
        AudioPacket {
            span: Some(AudioSpan {
                stream: self.stream,
                source: self.source,
                start_sample: self.next_sample.fetch_add(count, Ordering::Relaxed),
                sample_count: count,
            }),
            samples,
            sources,
        }
    }

    pub fn try_send(&self, samples: Vec<f32>) -> Result<(), mpsc::TrySendError<AudioPacket>> {
        self.tx.try_send(self.packet(samples, None))
    }

    pub fn send_with_sources(
        &self,
        samples: Vec<f32>,
        sources: Option<PacketSources>,
    ) -> Result<(), mpsc::SendError<AudioPacket>> {
        self.tx.send(self.packet(samples, sources))
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
        // session keeps its old file and cannot interleave with the new file.
        // Windows may refuse replacement while that file is open. Return the
        // error instead of truncating it; the caller disables diagnostics only.
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

    pub fn observe_sources(&mut self, span: Option<&AudioSpan>, sources: Option<&PacketSources>) {
        if let Some(sources) = sources {
            self.record(serde_json::json!({
                "event":"source_inputs", "version":1, "span":span, "sources":sources,
                "epoch_scope":"this-manifest", "source_roles":"caller-file-slots",
                "resampling":"channel-average-nearest-source-v1", "mixing":"sum-clamp-v1",
                "locator_format":"canonical-path-utf8-sha256",
                "binding":"observed-pcm-prefix", "final_file_validation":"not-performed",
                "capture_end":"unknown", "reuse_authorized":false
            }));
        }
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

    #[test]
    fn source_observations_join_forwarded_ranges_without_granting_reuse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("coverage.jsonl");
        let mut ledger = CoverageLedger::open(&path, Some("fixture-session")).unwrap();
        let span = AudioSpan {
            stream: 1,
            source: AudioSource::Mixed,
            start_sample: 800,
            sample_count: 1600,
        };
        let sources = PacketSources {
            ranges: vec![SourceRange {
                epoch: 7,
                role: StemRole::System,
                source_start_sample: 3200,
                output_start_sample: 0,
                sample_count: 1600,
            }],
            prefixes: vec![SourcePrefix {
                epoch: 7,
                role: StemRole::System,
                locator_sha256: Some("fixture-locator-hash".into()),
                sample_rate: 48000,
                channels: 2,
                encoding: "f32-le",
                data_offset: 4096,
                pcm_bytes: 115200,
                pcm_sha256: "fixture-pcm-hash".into(),
                decoded_samples: 4800,
                invalidated: None,
            }],
        };
        ledger.observe(Some(&span));
        ledger.observe_sources(Some(&span), None);
        ledger.observe_sources(Some(&span), Some(&sources));
        ledger.finish();
        let text = std::fs::read_to_string(&path).unwrap();
        let rows: Vec<serde_json::Value> = text
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let bindings: Vec<_> = rows
            .iter()
            .filter(|r| r["event"] == "source_inputs")
            .collect();
        assert_eq!(bindings.len(), 1);
        let binding = bindings[0];
        assert_eq!(binding["span"]["start_sample"], 800);
        assert_eq!(binding["sources"]["ranges"][0]["source_start_sample"], 3200);
        assert_eq!(binding["sources"]["prefixes"][0]["epoch"], 7);
        assert_eq!(binding["final_file_validation"], "not-performed");
        assert_eq!(binding["capture_end"], "unknown");
        assert_eq!(binding["reuse_authorized"], false);
        assert_eq!(rows.last().unwrap()["complete"], false);
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
        old.writer.flush().unwrap();
        let original = std::fs::read(&path).unwrap();
        let replacement = CoverageLedger::open(&path, Some("new-session"));
        if let Err(error) = &replacement {
            #[cfg(windows)]
            assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
            #[cfg(not(windows))]
            panic!("unexpected replacement failure: {error}");
        }
        if replacement.is_err() {
            assert_eq!(std::fs::read(&path).unwrap(), original);
        }
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
        drop(old);
        // Windows can safely retry after the old handle closes. On platforms
        // that replaced it, the late old write must stay in the unlinked file.
        let mut current = replacement
            .unwrap_or_else(|_| CoverageLedger::open(&path, Some("new-session")).unwrap());
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
