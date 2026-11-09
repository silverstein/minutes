//! Best-effort native-call spans. Capture never sends audio to this consumer:
//! an independent reader tails the stems and uses nonblocking bounded channels.
use crate::config::Config;
use crate::stem_tail::StemTail;
use crate::transcript_checkpoint::{self, CheckpointSource, CheckpointSpan, TranscriptCheckpoint};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

const RATE: usize = 16_000;
const SPAN: usize = 120 * RATE;
const POLL: Duration = Duration::from_millis(100);
// The final pass's chunker measures 100 ms windows from the start of the
// stem. It closes a chunk, and resets its state, after 800 ms of silence. A
// span that starts at that point, on that grid, gets the same chunks as the
// whole-stem pass, so whisper gets the same audio. Chunks are at most 45 s.
const WINDOW: usize = RATE / 10;
const MIN_PAUSE: usize = 8 * WINDOW;
const MAX_CHUNK: usize = 45 * RATE;
const SEARCH_BEFORE_TARGET: usize = 15 * RATE;
// Two sources share one decoder, so a total RTF above 0.5 cannot keep up.
// A single span may be slow (whisper temperature fallback on a hard chunk).
// Judge the total after a few spans; stop at once only when one span is
// slower than real time.
const MAX_TOTAL_RTF: f64 = 0.5;
const MAX_SPAN_RTF: f64 = 1.0;
const MIN_SPANS_FOR_TOTAL: u32 = 3;
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
    stop_log: Option<mpsc::SyncSender<serde_json::Value>>,
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
        let stop_log = stop_log_sender();
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
            stop_log,
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
        if let Some(log) = &self.stop_log {
            let _ = log.try_send(serde_json::json!({
                "accepted": spans.len(),
                "rejected": offered.saturating_sub(spans.len()),
                "skipped": self.counts.rejected.load(Ordering::Relaxed),
                "covered_secs": spans.iter().map(|s| (s.end_sample - s.start_sample) as f64 / RATE as f64).sum::<f64>(),
            }));
        }
        TranscriptCheckpoint {
            spans,
            ..Default::default()
        }
    }
}

// The desktop has no tracing subscriber. Keep its Stop receipt durable without
// adding filesystem work to Stop. One process-wide logger and a bounded queue
// also bound resource use if the log filesystem stalls.
fn stop_log_sender() -> Option<mpsc::SyncSender<serde_json::Value>> {
    static SENDER: OnceLock<Option<mpsc::SyncSender<serde_json::Value>>> = OnceLock::new();
    SENDER
        .get_or_init(|| {
            let (send, receive) = mpsc::sync_channel(8);
            std::thread::Builder::new()
                .name("call-checkpoint-log".into())
                .spawn(move || {
                    low_priority();
                    for summary in receive {
                        crate::logging::log_step("capture_checkpoint_stop", "", 0, summary);
                    }
                })
                .ok()
                .map(|_| send)
        })
        .clone()
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
    let mut speed = SpeedGuard::default();
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
        let decode_secs = started.elapsed().as_secs_f64();
        let audio_secs = work.samples.len() as f64 / RATE as f64;
        let rtf = decode_secs / audio_secs;
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
        if !speed.record(decode_secs, audio_secs) {
            tracing::info!(
                rtf,
                total_rtf = speed.total_rtf(),
                spans = speed.spans,
                "checkpoint decoder too slow; skipping remaining spans"
            );
            break;
        }
    }
    // Also ends the reader on decoder errors, panic, or slow-model cutoff.
    stop.store(true, Ordering::Release);
}

#[derive(Default)]
struct SpeedGuard {
    decode_secs: f64,
    audio_secs: f64,
    spans: u32,
}

impl SpeedGuard {
    /// Returns false when the decoder cannot keep up with the call.
    fn record(&mut self, decode_secs: f64, audio_secs: f64) -> bool {
        self.decode_secs += decode_secs;
        self.audio_secs += audio_secs;
        self.spans += 1;
        if decode_secs > audio_secs * MAX_SPAN_RTF {
            return false;
        }
        self.spans < MIN_SPANS_FOR_TOTAL || self.total_rtf() <= MAX_TOTAL_RTF
    }

    fn total_rtf(&self) -> f64 {
        self.decode_secs / self.audio_secs.max(f64::EPSILON)
    }
}

struct Source {
    path: PathBuf,
    role: CheckpointSource,
    tail: Option<StemTail>,
    pending: Vec<f32>,
    start: u64,
    peak: f32,
    checked_len: usize,
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
            checked_len: 0,
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
            loop {
                // The pause search reads up to 1.5 spans. Repeat it once a
                // second at most, not on every poll.
                if source.pending.len() < span || source.pending.len() < source.checked_len + RATE {
                    break;
                }
                source.checked_len = source.pending.len();
                let Some(cut) = span_cut(&source.pending, span) else {
                    break;
                };
                source.checked_len = 0;
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

/// Where to end the open span, or None to wait for more audio. `pending`
/// must start where the final pass's chunker is in its reset state, on its
/// 100 ms grid: at the stem start, or at an earlier cut from this function.
///
/// The cut is a point where the chunker resets, so the next span starts in
/// the state that the whole-stem pass has there, and gets the same chunks.
/// The first choice, at or after 15 s before the target length, is the end of
/// the longest pause, where the chunker closes a chunk after 800 ms of
/// silence. In speech without such a pause, the chunker splits at 45 s and
/// resets too. At 1.5x the target, take the latest reset point anywhere in the
/// span. Only with none at all, fall back to the quietest 100 ms window.
fn span_cut(pending: &[f32], span: usize) -> Option<usize> {
    if pending.len() < span {
        return None;
    }
    let max = span + span / 2;
    let view = &pending[..pending.len().min(max)];
    let earliest = span.saturating_sub(SEARCH_BEFORE_TARGET);
    let chunks = crate::transcribe::detect_meeting_vad_chunks(view);
    if chunks.is_empty() {
        // No speech at all: any point on the grid is a pause.
        return Some(span - span % WINDOW);
    }
    let next_starts = chunks
        .iter()
        .skip(1)
        .map(|&(start, _)| start)
        .chain([view.len()]);
    // (cut, pause length). A chunk shorter than 1 s does not close, and a
    // 45 s chunk closes without a pause.
    let pauses: Vec<(usize, usize)> = chunks
        .iter()
        .zip(next_starts)
        .filter(|&(&(start, end), next)| {
            (RATE..MAX_CHUNK).contains(&(end - start)) && next >= end + MIN_PAUSE
        })
        .map(|(&(_, end), next)| (end + MIN_PAUSE, next - end))
        .collect();
    let splits = chunks
        .iter()
        .filter(|&&(start, end)| end - start >= MAX_CHUNK)
        .map(|&(_, end)| end);
    let preferred = pauses
        .iter()
        .filter(|&&(cut, _)| cut >= earliest)
        .max_by_key(|&&(cut, pause)| (pause, std::cmp::Reverse(cut)))
        .map(|&(cut, _)| cut)
        .or_else(|| splits.clone().find(|&end| end >= earliest));
    if preferred.is_some() || view.len() < max {
        return preferred;
    }
    pauses
        .iter()
        .map(|&(cut, _)| cut)
        .chain(splits)
        .filter(|&cut| cut > 0 && cut <= view.len())
        .max()
        .or_else(|| Some(quiet_cut(view) / WINDOW * WINDOW))
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
            // A tone has no pause, so the cut falls back at 1.5 spans (4.5 s).
            let worker = CaptureCheckpoint::spawn(
                &anchor,
                3 * RATE,
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

    fn speech_with_pauses(seconds: usize, pauses: &[(f32, f32)]) -> Vec<f32> {
        let mut samples: Vec<f32> = (0..seconds * RATE)
            .map(|n| 0.2 * (n as f32 * 0.031).sin())
            .collect();
        for &(from, to) in pauses {
            samples[(from * RATE as f32) as usize..(to * RATE as f32) as usize].fill(0.0);
        }
        samples
    }

    #[test]
    fn cuts_in_the_middle_of_a_real_pause_near_the_target() {
        let samples = speech_with_pauses(125, &[(110.0, 111.5)]);
        let cut = span_cut(&samples, SPAN).expect("a pause after 105 s");
        assert!(
            (110 * RATE..=111 * RATE + RATE / 2).contains(&cut),
            "cut {cut} is outside the pause"
        );
    }

    #[test]
    fn waits_for_a_pause_rather_than_cutting_inside_speech() {
        // Only a 100 ms dip, which is a gap between words, not a pause. The
        // chunker splits this speech at 45 s and 90 s, before the window.
        let samples = speech_with_pauses(120, &[(112.0, 112.1)]);
        assert_eq!(span_cut(&samples, SPAN), None);
        // The span grows until a pause comes.
        let samples = speech_with_pauses(150, &[(140.0, 141.5)]);
        let cut = span_cut(&samples, SPAN).expect("a pause at 140 s");
        assert!((140 * RATE..=141 * RATE + RATE / 2).contains(&cut));
    }

    #[test]
    fn speech_without_a_pause_is_cut_where_the_chunker_splits_at_45_s() {
        let samples = speech_with_pauses(150, &[(112.0, 112.1)]);
        assert_eq!(span_cut(&samples, SPAN), Some(135 * RATE));
        // A pause before the window does not count, but it moves the splits.
        let samples = speech_with_pauses(150, &[(60.0, 62.0)]);
        let cut = span_cut(&samples, SPAN).unwrap();
        assert!(
            cut >= SPAN - 15 * RATE && cut.is_multiple_of(WINDOW),
            "cut {cut}"
        );
    }

    #[test]
    fn falls_back_to_the_quietest_window_at_one_and_a_half_spans() {
        // A 0.3 s sound and its 0.5 s hangover are shorter than a 1 s chunk,
        // so the chunker never closes or splits it: no reset point.
        let mut samples = vec![0.0; 185 * RATE];
        for (n, sample) in samples[RATE..RATE + RATE * 3 / 10].iter_mut().enumerate() {
            *sample = 0.2 * (n as f32 * 0.031).sin();
        }
        // The quietest window is in the last 15 s of the 180 s span.
        let cut = span_cut(&samples, SPAN).unwrap();
        assert!(cut >= 164 * RATE && cut.is_multiple_of(WINDOW), "cut {cut}");
    }

    /// Cut `samples` the way `feed` does, and return each span's range.
    fn cut_like_feed(samples: &[f32], span: usize) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut start = 0;
        let mut len = span;
        while start + len <= samples.len() {
            match span_cut(&samples[start..start + len], span) {
                Some(cut) => {
                    spans.push((start, start + cut));
                    start += cut;
                    len = span;
                }
                None => len += RATE,
            }
        }
        spans
    }

    fn assert_spans_chunk_like_the_whole_stem(samples: &[f32]) {
        let whole = crate::transcribe::detect_meeting_vad_chunks(samples);
        let spans = cut_like_feed(samples, SPAN);
        assert!(spans.len() >= 2, "too few spans to compare");
        let covered = spans.last().unwrap().1;
        let per_span: Vec<_> = spans
            .iter()
            .flat_map(|&(a, b)| {
                crate::transcribe::detect_meeting_vad_chunks(&samples[a..b])
                    .into_iter()
                    .map(move |(s, e)| (a + s, a + e))
            })
            .collect();
        let whole: Vec<_> = whole.into_iter().filter(|&(s, _)| s < covered).collect();
        if let Some(i) =
            (0..per_span.len().max(whole.len())).find(|&i| per_span.get(i) != whole.get(i))
        {
            let secs = |v: Option<&(usize, usize)>| {
                v.map(|&(s, e)| (s as f64 / RATE as f64, e as f64 / RATE as f64))
            };
            let near: Vec<_> = spans
                .iter()
                .map(|&(_, b)| b as f64 / RATE as f64)
                .filter(|&b| {
                    let t = whole.get(i).map_or(0.0, |c| c.0 as f64 / RATE as f64);
                    (b - t).abs() < 120.0
                })
                .collect();
            panic!(
                "chunk {i} of {} differs: span pass {:?}, whole pass {:?}; previous {:?}; cuts near {near:?}",
                whole.len(),
                secs(per_span.get(i)),
                secs(whole.get(i)),
                secs(whole.get(i.wrapping_sub(1)))
            );
        }
    }

    #[test]
    fn spans_give_the_same_chunks_as_the_whole_stem() {
        // Speech with pauses of different lengths at odd sample offsets.
        let mut pauses = Vec::new();
        let mut t = 3.37_f32;
        while t < 590.0 {
            let len = [0.3, 1.2, 0.5, 2.1, 0.9][pauses.len() % 5];
            pauses.push((t, t + len));
            t += 7.913 + len;
        }
        assert_spans_chunk_like_the_whole_stem(&speech_with_pauses(600, &pauses));
    }

    /// Run on a real stem: MINUTES_REAL_STEM=/path/voice.wav cargo test
    /// real_stem -- --ignored. It decodes no speech, so it is fast.
    #[test]
    #[ignore]
    fn real_stem_spans_give_the_same_chunks_as_the_whole_stem() {
        let path = std::env::var("MINUTES_REAL_STEM").expect("MINUTES_REAL_STEM");
        let samples = crate::transcribe::load_audio_samples_with_format(
            Path::new(&path),
            Some("wav"),
            &Config::default(),
        )
        .unwrap();
        assert_spans_chunk_like_the_whole_stem(&samples);
    }

    #[test]
    fn a_long_silence_after_an_early_pause_is_cut_at_that_pause() {
        // Speech, a pause at 60 s, then silence with one short sound. The
        // short sound stays an open chunk, so the early pause is the last
        // point where the chunker resets.
        let mut samples = speech_with_pauses(185, &[(60.0, 185.0)]);
        for (n, sample) in samples[160 * RATE..160 * RATE + RATE * 3 / 10]
            .iter_mut()
            .enumerate()
        {
            *sample = 0.2 * (n as f32 * 0.031).sin();
        }
        let cut = span_cut(&samples, SPAN).unwrap();
        assert!((60 * RATE..62 * RATE).contains(&cut), "cut {cut}");
    }

    #[test]
    fn silence_is_cut_at_the_target() {
        assert_eq!(span_cut(&vec![0.0; SPAN + RATE], SPAN), Some(SPAN));
    }

    #[test]
    fn one_slow_span_does_not_stop_a_decoder_that_keeps_up() {
        // The real 80-minute call: one 66 s chunk of temperature fallback.
        let mut speed = SpeedGuard::default();
        for rtf in [0.06, 0.42, 0.18, 0.07, 0.05, 0.24, 0.07, 0.55, 0.1, 0.1] {
            assert!(speed.record(rtf * 110.0, 110.0), "stopped at rtf {rtf}");
        }
        assert!(speed.total_rtf() < MAX_TOTAL_RTF);
    }

    #[test]
    fn a_decoder_that_cannot_keep_up_stops_after_three_spans() {
        let mut speed = SpeedGuard::default();
        assert!(speed.record(66.0, 110.0));
        assert!(speed.record(66.0, 110.0));
        assert!(!speed.record(66.0, 110.0));
    }

    #[test]
    fn a_span_slower_than_real_time_stops_at_once() {
        let mut speed = SpeedGuard::default();
        assert!(!speed.record(187.0, 110.0));
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
