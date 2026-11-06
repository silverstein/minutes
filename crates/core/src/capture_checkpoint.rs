//! Best-effort native-call spans. Capture never sends audio to this consumer:
//! an independent reader tails the stems and uses nonblocking bounded channels.
use crate::config::Config;
use crate::stem_tail::StemTail;
use crate::transcript_checkpoint::{self, CheckpointSource, CheckpointSpan, TranscriptCheckpoint};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const RATE: usize = 16_000;
const SPAN: usize = 120 * RATE;
const POLL: Duration = Duration::from_millis(100);
// At most one abandoned decoder can remain alive after Stop in this process.
static DECODER_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Default)]
struct Counts {
    rejected: AtomicUsize,
    offered: AtomicUsize,
}

struct Work {
    samples: Vec<f32>,
    start: u64,
    source: CheckpointSource,
}

/// Owned by one native recording. Dropping or finishing never joins a thread.
pub struct CaptureCheckpoint {
    stop: Arc<AtomicBool>,
    completed: mpsc::Receiver<CheckpointSpan>,
    counts: Arc<Counts>,
}

impl CaptureCheckpoint {
    /// Start only for ambient native meeting capture, with the same title and
    /// pre-context that will be supplied when queueing the final job.
    pub fn start(
        anchor: &Path,
        config: &Config,
        title: Option<&str>,
        context: Option<&str>,
    ) -> Option<Self> {
        Self::start_at(anchor, config, title, context, chrono::Local::now())
    }

    pub fn start_at(
        anchor: &Path,
        config: &Config,
        title: Option<&str>,
        context: Option<&str>,
        recorded_at: chrono::DateTime<chrono::Local>,
    ) -> Option<Self> {
        if !config.recording.transcript_checkpoints || crate::sensitive::is_active() {
            return None;
        }
        let title = title.map(str::to_owned);
        let context = context.map(str::to_owned);
        let config = config.clone();
        let mut decoder = None;
        Self::spawn(
            anchor,
            SPAN,
            move |work| {
                let decoder = decoder.get_or_insert_with(|| {
                    let hints = transcript_checkpoint::decode_hints_for_queued_meeting_at(
                        title.as_deref(),
                        context.as_deref(),
                        recorded_at,
                        &config,
                    );
                    transcript_checkpoint::SpanTranscriber::new(&config, &hints)
                });
                decoder
                    .transcribe_span(&work.samples, work.start, work.source)
                    .map_err(|e| e.to_string())
            },
            true,
        )
    }

    fn spawn<F>(anchor: &Path, span_samples: usize, decode: F, exclusive: bool) -> Option<Self>
    where
        F: FnMut(&Work) -> Result<CheckpointSpan, String> + Send + 'static,
    {
        if exclusive
            && DECODER_ACTIVE
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Relaxed)
                .is_err()
        {
            tracing::warn!("checkpoint decoder still busy; recording continues without spans");
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let counts = Arc::new(Counts::default());
        let (send, receive) = mpsc::sync_channel(2);
        // Bounded even if nobody polls results until Stop. Excess spans fall
        // back to ordinary processing, including unusually long captures.
        let (complete, completed) = mpsc::sync_channel(256);
        let worker_stop = stop.clone();
        let worker_counts = counts.clone();
        let worker = std::thread::Builder::new()
            .name("call-checkpoint-decode".into())
            .spawn(move || {
                struct Release(bool);
                impl Drop for Release {
                    fn drop(&mut self) {
                        if self.0 {
                            DECODER_ACTIVE.store(false, Ordering::Release);
                        }
                    }
                }
                let _release = Release(exclusive);
                low_priority();
                decode_loop(receive, complete, worker_stop, worker_counts, decode);
            });
        if worker.is_err() {
            if exclusive {
                DECODER_ACTIVE.store(false, Ordering::Release);
            }
            return None;
        }
        let paths = [
            anchor.with_extension("voice.wav"),
            anchor.with_extension("system.wav"),
        ];
        let feeder_stop = stop.clone();
        let feeder_counts = counts.clone();
        if std::thread::Builder::new()
            .name("call-checkpoint-stems".into())
            .spawn(move || {
                low_priority();
                feed(paths, span_samples, send, feeder_stop, feeder_counts);
            })
            .is_err()
        {
            stop.store(true, Ordering::Release);
            return None;
        }
        Some(Self {
            stop,
            completed,
            counts,
        })
    }

    /// Take only spans already published. In-flight and queued spans are left
    /// to final processing. No mutex, file scan, wait, or thread join here.
    pub fn finish(self) -> TranscriptCheckpoint {
        self.stop.store(true, Ordering::Release);
        let spans: Vec<_> = self.completed.try_iter().collect();
        let offered = self.counts.offered.load(Ordering::Relaxed);
        tracing::info!(
            accepted = spans.len(),
            rejected = offered.saturating_sub(spans.len()),
            skipped = self.counts.rejected.load(Ordering::Relaxed),
            "native call transcript checkpoints at Stop"
        );
        TranscriptCheckpoint {
            spans,
            ..Default::default()
        }
    }
}

impl Drop for CaptureCheckpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

fn decode_loop<F>(
    receive: mpsc::Receiver<Work>,
    complete: mpsc::SyncSender<CheckpointSpan>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    mut decode: F,
) where
    F: FnMut(&Work) -> Result<CheckpointSpan, String>,
{
    while !stop.load(Ordering::Acquire) {
        let work = match receive.recv_timeout(POLL) {
            Ok(work) => work,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if stop.load(Ordering::Acquire) {
            break;
        }
        let started = Instant::now();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| decode(&work)));
        let rtf = started.elapsed().as_secs_f64() / (work.samples.len() as f64 / RATE as f64);
        if stop.load(Ordering::Acquire) {
            break;
        }
        match result {
            Ok(Ok(span)) => {
                tracing::info!(source = ?span.source, start_sample = span.start_sample,
                    end_sample = span.end_sample, rtf, "native call checkpoint span completed");
                if complete.try_send(span).is_err() {
                    counts.rejected.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {
                counts.rejected.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(
                    "checkpoint decoder failed; remaining audio will be transcribed after Stop"
                );
                break;
            }
        }
        if rtf > 0.5 {
            tracing::info!(rtf, "checkpoint decoder too slow; skipping remaining spans");
            break;
        }
    }
    // Also ends the reader on decoder errors, panic, or slow-model cutoff.
    stop.store(true, Ordering::Release);
}

struct Source {
    path: PathBuf,
    role: CheckpointSource,
    tail: Option<StemTail>,
    pending: Vec<f32>,
    start: u64,
    peak: f32,
}

fn feed(
    paths: [PathBuf; 2],
    span: usize,
    send: mpsc::SyncSender<Work>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
) {
    let mut sources = paths
        .into_iter()
        .zip([CheckpointSource::Voice, CheckpointSource::System])
        .map(|(path, role)| Source {
            path,
            role,
            tail: None,
            pending: Vec::new(),
            start: 0,
            peak: 0.0,
        })
        .collect::<Vec<_>>();
    while !stop.load(Ordering::Acquire) {
        for source in &mut sources {
            if stop.load(Ordering::Acquire) {
                return;
            }
            if source.tail.is_none() {
                source.tail = StemTail::open_canonical(&source.path).ok();
            }
            let Some(tail) = source.tail.as_mut() else {
                continue;
            };
            let samples = match tail.poll() {
                Ok(samples) => samples,
                // Continuing after lost bytes could give the wrong clock.
                Err(_) => {
                    tracing::warn!("checkpoint stem read failed; skipping remaining spans");
                    return;
                }
            };
            source.peak = samples.iter().map(|s| s.abs()).fold(source.peak, f32::max);
            source.pending.extend(samples);
            while source.pending.len() >= span {
                let cut = quiet_cut(&source.pending[..span]);
                let rest = source.pending.split_off(cut);
                let samples = std::mem::replace(&mut source.pending, rest);
                let start = source.start;
                source.start += cut as u64;
                // The final WAV decoder normalizes a quiet file using its
                // whole-file peak. Until a >=0.1 peak is observed, its eventual
                // gain is unknown, so do not offer potentially different PCM.
                if source.peak < 0.1 {
                    counts.rejected.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                counts.offered.fetch_add(1, Ordering::Relaxed);
                match send.try_send(Work {
                    samples,
                    start,
                    source: source.role,
                }) {
                    Ok(()) => (),
                    Err(mpsc::TrySendError::Full(_)) => {
                        counts.rejected.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "checkpoint worker fell behind; stopped cutting spans for this call"
                        );
                        stop.store(true, Ordering::Release);
                        return;
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => return,
                }
            }
        }
        std::thread::sleep(POLL);
    }
}

fn quiet_cut(samples: &[f32]) -> usize {
    let window = (RATE / 10).min(samples.len());
    let begin = samples.len().saturating_sub(15 * RATE).max(window);
    (begin..=samples.len())
        .step_by(window)
        .min_by(|&a, &b| {
            let energy = |end: usize| {
                samples[end - window..end]
                    .iter()
                    .map(|&s| f64::from(s).powi(2))
                    .sum::<f64>()
            };
            energy(a).total_cmp(&energy(b))
        })
        .unwrap_or(samples.len())
        - window / 2
}

fn low_priority() {
    #[cfg(target_os = "macos")]
    // Only this optional thread changes QoS. Capture and the UI retain theirs.
    unsafe {
        libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_BACKGROUND, 0);
    }
    #[cfg(target_os = "linux")]
    unsafe {
        libc::setpriority(
            libc::PRIO_PROCESS,
            libc::syscall(libc::SYS_gettid) as u32,
            10,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::with_temp_home;

    fn write_stem(path: &Path, seconds: u32, rate: u32) {
        let mut writer = hound::WavWriter::create(
            path,
            hound::WavSpec {
                channels: 1,
                sample_rate: rate,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for n in 0..seconds * rate {
            let sample = (0.25 * (n as f32 * 0.031).sin() * 32768.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn wait_until(mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !condition() {
            assert!(Instant::now() < deadline, "checkpoint test timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn fake_span(work: &Work) -> CheckpointSpan {
        CheckpointSpan {
            source: work.source,
            start_sample: work.start,
            end_sample: work.start + work.samples.len() as u64,
            settings: "test".into(),
            pcm_sha256: "test".into(),
            lines: vec!["[0:00] fixture".into()],
        }
    }

    fn queue(anchor: &Path, checkpoint: &TranscriptCheckpoint) -> crate::jobs::ProcessingJob {
        crate::jobs::queue_live_capture_with_transcript_checkpoint(
            crate::CaptureMode::Meeting,
            Some("checkpoint failure isolation".into()),
            anchor,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(checkpoint),
        )
        .unwrap()
    }

    #[test]
    fn stalled_decoder_does_not_block_stem_writes_stop_or_job_queue() {
        with_temp_home(|home| {
            let anchor = home.join("call.mov");
            std::fs::write(&anchor, []).unwrap();
            for role in ["voice", "system"] {
                write_stem(&anchor.with_extension(format!("{role}.wav")), 20, 16_000);
            }
            let (entered, entry) = mpsc::sync_channel(1);
            let (release, blocked) = mpsc::sync_channel(1);
            let (exited, exit) = mpsc::sync_channel(1);
            let worker = CaptureCheckpoint::spawn(
                &anchor,
                4 * RATE,
                move |work| {
                    entered.send(()).unwrap();
                    blocked.recv().unwrap();
                    exited.send(()).unwrap();
                    Ok(fake_span(work))
                },
                false,
            )
            .unwrap();
            entry.recv_timeout(Duration::from_secs(5)).unwrap();
            wait_until(|| worker.counts.rejected.load(Ordering::Relaxed) > 0);
            assert!(
                worker.stop.load(Ordering::Acquire),
                "a full queue must stop span cutting"
            );
            // Stand in for capture continuing to append while decode is wedged.
            use std::io::Write;
            let voice = anchor.with_extension("voice.wav");
            let before = std::fs::metadata(&voice).unwrap().len();
            let mut writer = std::fs::OpenOptions::new()
                .append(true)
                .open(&voice)
                .unwrap();
            writer.write_all(&[0; 32_000]).unwrap();
            writer.flush().unwrap();
            assert_eq!(std::fs::metadata(&voice).unwrap().len(), before + 32_000);
            let started = Instant::now();
            let checkpoint = worker.finish();
            assert!(started.elapsed() < Duration::from_millis(100));
            assert!(checkpoint.is_empty());
            let job = queue(&anchor, &checkpoint);
            assert_eq!(
                crate::jobs::load_job(&job.id).unwrap().state,
                crate::jobs::JobState::Queued
            );
            let queued = PathBuf::from(&job.audio_path).with_extension("voice.wav");
            assert_eq!(std::fs::metadata(queued).unwrap().len(), before + 32_000);
            assert!(!crate::jobs::job_checkpoint_path(&job.id).exists());
            release.send(()).unwrap();
            exit.recv_timeout(Duration::from_secs(2)).unwrap();
        });
    }

    #[test]
    fn panicking_decoder_keeps_stems_and_queues_without_checkpoint() {
        with_temp_home(|home| {
            let anchor = home.join("call.mov");
            std::fs::write(&anchor, []).unwrap();
            for role in ["voice", "system"] {
                write_stem(&anchor.with_extension(format!("{role}.wav")), 5, 16_000);
            }
            let worker = CaptureCheckpoint::spawn(
                &anchor,
                4 * RATE,
                |_| panic!("injected optional decoder panic"),
                false,
            )
            .unwrap();
            wait_until(|| worker.stop.load(Ordering::Acquire));
            let checkpoint = worker.finish();
            assert!(checkpoint.is_empty());
            let job = queue(&anchor, &checkpoint);
            let voice = PathBuf::from(&job.audio_path).with_extension("voice.wav");
            assert_eq!(
                hound::WavReader::open(voice).unwrap().duration(),
                5 * 16_000
            );
            assert_eq!(
                crate::jobs::load_job(&job.id).unwrap().state,
                crate::jobs::JobState::Queued
            );
        });
    }

    #[test]
    fn canonical_tail_matches_final_decoder_at_48k_across_small_polls() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("source.wav");
        write_stem(&path, 5, 48_000);
        let final_samples = crate::transcribe::load_audio_samples_with_format(
            &path,
            Some("wav"),
            &Config::default(),
        )
        .unwrap();
        let mut tail = StemTail::open_canonical(&path).unwrap();
        let mut samples = Vec::new();
        loop {
            let next = tail.poll().unwrap();
            if next.is_empty() {
                break;
            }
            samples.extend(next);
        }
        assert!(samples.len() > final_samples.len() - 32);
        assert_eq!(samples, final_samples[..samples.len()]);
    }

    #[test]
    fn first_slow_span_disables_the_decoder() {
        let (send, receive) = mpsc::sync_channel(2);
        let (complete, result) = mpsc::sync_channel(2);
        for _ in 0..2 {
            send.send(Work {
                samples: vec![0.2; 160],
                start: 0,
                source: CheckpointSource::Voice,
            })
            .unwrap();
        }
        let stop = Arc::new(AtomicBool::new(false));
        let mut calls = 0;
        decode_loop(
            receive,
            complete,
            stop.clone(),
            Arc::new(Counts::default()),
            |work| {
                calls += 1;
                std::thread::sleep(Duration::from_millis(20));
                Ok(fake_span(work))
            },
        );
        assert_eq!(calls, 1);
        assert_eq!(result.try_iter().count(), 1);
        assert!(stop.load(Ordering::Acquire));
    }

    #[test]
    fn cuts_inside_the_quietest_pause_in_last_fifteen_seconds() {
        let mut samples = vec![0.2; SPAN];
        samples[112 * RATE..112 * RATE + RATE / 10].fill(0.0);
        let cut = quiet_cut(&samples);
        assert_eq!(cut, 112 * RATE + RATE / 20);
    }

    #[test]
    fn config_defaults_on_and_explicit_off_does_not_spawn() {
        let old: Config = toml::from_str("").unwrap();
        assert!(old.recording.transcript_checkpoints);
        let off: Config = toml::from_str("[recording]\ntranscript_checkpoints = false\n").unwrap();
        assert!(!off.recording.transcript_checkpoints);
        assert!(CaptureCheckpoint::start(Path::new("unused.mov"), &off, None, None).is_none());
    }
}
