//! Metadata-only progress for the optional live input path.
//!
//! A sampler independent of the feeder/consumer keeps reporting when either is
//! blocked in file IO, a channel send, or VAD. Wall time includes descheduling;
//! it is not kernel/CPU time and cannot establish an OS scheduling cause.
//! Snapshots are approximate concurrent observations, never capture health or
//! authorization evidence. No audio, text, file paths, or meeting identity.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const INTERVAL: Duration = Duration::from_secs(10);
static NEXT_PROGRESS_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy)]
#[cfg_attr(not(all(feature = "streaming", feature = "whisper")), allow(dead_code))]
pub(crate) enum Stage {
    Idle,
    HeaderWait,
    VoicePoll,
    SystemPoll,
    Mix,
    Send,
    PollSleep,
    Receive,
    DraftGate,
    Vad,
    Segment,
    Heartbeat,
    PublishDraft,
    WorkerJoin,
    Finalize,
    VadInit,
}

const STAGES: [&str; 16] = [
    "idle",
    "header_wait",
    "voice_poll",
    "system_poll",
    "mix",
    "send",
    "poll_sleep",
    "receive",
    "draft_gate",
    "vad",
    "segment",
    "heartbeat",
    "publish_draft",
    "worker_join",
    "finalize",
    "vad_init",
];

#[derive(Clone, Copy)]
#[cfg_attr(not(all(feature = "streaming", feature = "whisper")), allow(dead_code))]
pub(crate) enum Counter {
    VoiceSamplesRead,
    SystemSamplesRead,
    SamplesSent,
    SamplesReceived,
    SamplesProcessed,
    SamplesGated,
    SpeakingWindows,
    SilenceWindows,
    PollErrors,
    EmptyPolls,
    VoicePendingSamples,
    SystemPendingSamples,
    ChunkRemainingSamples,
    SendsCompleted,
}

const COUNTERS: [&str; 14] = [
    "voice_samples_read",
    "system_samples_read",
    "samples_sent",
    "samples_received",
    "samples_processed",
    "samples_gated",
    "speaking_windows",
    "silence_windows",
    "poll_errors",
    "empty_polls",
    "voice_pending_samples",
    "system_pending_samples",
    "chunk_remaining_samples",
    "sends_completed",
];

#[derive(Default)]
struct Timing {
    count: AtomicU64,
    total_us: AtomicU64,
    max_us: AtomicU64,
}

struct State {
    id: u64,
    started: Instant,
    stage: AtomicUsize,
    stage_started_us: AtomicU64,
    last_progress_us: AtomicU64,
    counters: [AtomicU64; COUNTERS.len()],
    timing: [Timing; STAGES.len()],
}

impl State {
    fn new() -> Self {
        Self {
            id: NEXT_PROGRESS_ID.fetch_add(1, Ordering::Relaxed),
            started: Instant::now(),
            stage: AtomicUsize::new(Stage::Idle as usize),
            stage_started_us: AtomicU64::new(0),
            last_progress_us: AtomicU64::new(0),
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
            timing: std::array::from_fn(|_| Timing::default()),
        }
    }

    fn elapsed_us(&self) -> u64 {
        micros(self.started.elapsed())
    }

    fn snapshot(&self) -> serde_json::Value {
        let now = self.elapsed_us();
        let stage = self.stage.load(Ordering::Acquire);
        let counters: serde_json::Map<_, _> = COUNTERS
            .iter()
            .enumerate()
            .map(|(i, name)| {
                (
                    (*name).into(),
                    self.counters[i].load(Ordering::Relaxed).into(),
                )
            })
            .collect();
        let timing: serde_json::Map<_, _> = STAGES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let timing = &self.timing[i];
                (
                    (*name).into(),
                    serde_json::json!({
                        "count": timing.count.load(Ordering::Relaxed),
                        "total_us": timing.total_us.load(Ordering::Relaxed),
                        "max_us": timing.max_us.load(Ordering::Relaxed),
                    }),
                )
            })
            .collect();
        serde_json::json!({
            "progress_id": self.id,
            "elapsed_us": now,
            "stage": STAGES[stage],
            "stage_age_us": now.saturating_sub(self.stage_started_us.load(Ordering::Relaxed)),
            "last_progress_age_us": now.saturating_sub(self.last_progress_us.load(Ordering::Relaxed)),
            "counters": counters,
            "wall_timing": timing,
        })
    }
}

fn micros(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX)
}

/// Each input thread owns its own sampler. The sampler never reads the stems,
/// transcript, configuration, or writer lock. Failure to start/log is ignored.
/// Dropping this object wakes the sampler, but never joins behind log file IO.
pub(crate) struct InputProgress {
    state: Arc<State>,
    stop: Option<mpsc::Sender<()>>,
}

impl InputProgress {
    pub(crate) fn start(component: &'static str) -> Self {
        // Resolve once in the owner thread. A detached final sample must not
        // follow a later test/library switch into a different log directory.
        let log_path = crate::logging::log_path();
        Self::with_observer(INTERVAL, move |snapshot, ended| {
            let entry = serde_json::json!({
                "ts": chrono::Local::now().to_rfc3339(),
                "level": "info",
                "step": "live_input_progress",
                "file": "",
                "message": "optional live input progress; wall time includes scheduling pauses",
                "extra": {
                    "component": component,
                    "pid": std::process::id(),
                    "ended": ended,
                    "progress": snapshot,
                },
            });
            let _ = append_progress_log(&log_path, &entry);
        })
    }

    fn with_observer(
        interval: Duration,
        mut observer: impl FnMut(serde_json::Value, bool) + Send + 'static,
    ) -> Self {
        let state = Arc::new(State::new());
        let sampled = Arc::clone(&state);
        let (stop, stopped) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("live-input-progress".into())
            .spawn(move || loop {
                let ended = !matches!(
                    stopped.recv_timeout(interval),
                    Err(mpsc::RecvTimeoutError::Timeout)
                );
                observer(sampled.snapshot(), ended);
                if ended {
                    break;
                }
            })
            .is_ok();
        Self {
            state,
            stop: spawned.then_some(stop),
        }
    }

    pub(crate) fn add(&self, counter: Counter, samples: usize) {
        self.state.counters[counter as usize].fetch_add(samples as u64, Ordering::Relaxed);
        if samples > 0
            && matches!(
                counter,
                Counter::VoiceSamplesRead
                    | Counter::SystemSamplesRead
                    | Counter::SamplesSent
                    | Counter::SamplesReceived
                    | Counter::SamplesProcessed
            )
        {
            self.state
                .last_progress_us
                .store(self.state.elapsed_us(), Ordering::Relaxed);
        }
    }

    pub(crate) fn set(&self, counter: Counter, value: usize) {
        self.state.counters[counter as usize].store(value as u64, Ordering::Relaxed);
    }

    pub(crate) fn stage(&self, stage: Stage) -> StageGuard<'_> {
        self.state
            .stage_started_us
            .store(self.state.elapsed_us(), Ordering::Relaxed);
        self.state.stage.store(stage as usize, Ordering::Release);
        StageGuard {
            progress: self,
            stage,
            started: Instant::now(),
        }
    }

    pub(crate) fn measure<T>(&self, stage: Stage, operation: impl FnOnce() -> T) -> T {
        let _guard = self.stage(stage);
        operation()
    }

    #[cfg(any(test, all(feature = "streaming", feature = "whisper")))]
    pub(crate) fn snapshot(&self) -> serde_json::Value {
        self.state.snapshot()
    }
}

fn append_progress_log(path: &std::path::Path, entry: &serde_json::Value) -> std::io::Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(entry)? + "\n";
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?
        .write_all(line.as_bytes())
}

impl Drop for InputProgress {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

pub(crate) struct StageGuard<'a> {
    progress: &'a InputProgress,
    stage: Stage,
    started: Instant,
}

impl Drop for StageGuard<'_> {
    fn drop(&mut self) {
        let timing = &self.progress.state.timing[self.stage as usize];
        let elapsed = micros(self.started.elapsed());
        timing.count.fetch_add(1, Ordering::Relaxed);
        timing.total_us.fetch_add(elapsed, Ordering::Relaxed);
        timing.max_us.fetch_max(elapsed, Ordering::Relaxed);
        self.progress
            .state
            .stage_started_us
            .store(self.progress.state.elapsed_us(), Ordering::Relaxed);
        self.progress
            .state
            .stage
            .store(Stage::Idle as usize, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_blocked_send_then_exact_delivery_without_touching_audio() {
        let (snapshots, observed) = mpsc::channel();
        let progress = InputProgress::with_observer(Duration::from_millis(5), move |s, ended| {
            snapshots.send((s, ended)).ok();
        });
        let (tx, rx) = mpsc::sync_channel(1);
        tx.send(vec![0.25_f32; 1600]).unwrap();
        let feeder = std::thread::spawn(move || {
            progress.add(Counter::VoiceSamplesRead, 3200);
            progress.add(Counter::SamplesSent, 1600);
            progress
                .measure(Stage::Send, || tx.send(vec![0.5_f32; 1600]))
                .unwrap();
            progress.add(Counter::SamplesSent, 1600);
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let (s, ended) = observed.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(!ended);
            if s["stage"] == "send" {
                assert_eq!(s["counters"]["voice_samples_read"], 3200);
                assert_eq!(s["counters"]["samples_sent"], 1600);
                break;
            }
            assert!(Instant::now() < deadline);
        }
        assert_eq!(rx.recv().unwrap(), vec![0.25; 1600]);
        assert_eq!(rx.recv().unwrap(), vec![0.5; 1600]);
        feeder.join().unwrap();
        loop {
            let (s, ended) = observed.recv_timeout(Duration::from_secs(2)).unwrap();
            if ended {
                assert_eq!(s["counters"]["samples_sent"], 3200);
                assert_eq!(s["wall_timing"]["send"]["count"], 1);
                break;
            }
        }
    }

    #[test]
    fn reports_vad_in_flight_and_unprocessed_burst_separately_from_receipt() {
        let (snapshots, observed) = mpsc::channel();
        let progress = InputProgress::with_observer(Duration::from_millis(5), move |s, _| {
            snapshots.send(s).ok();
        });
        progress.add(Counter::SamplesReceived, 9600);
        progress.add(Counter::SamplesProcessed, 1600);
        progress.set(Counter::ChunkRemainingSamples, 8000);
        let guard = progress.stage(Stage::Vad);
        let s = observed.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(s["stage"], "vad");
        assert_eq!(s["counters"]["samples_received"], 9600);
        assert_eq!(s["counters"]["samples_processed"], 1600);
        assert_eq!(s["counters"]["chunk_remaining_samples"], 8000);
        assert_eq!(s["wall_timing"]["vad"]["count"], 0);
        drop(guard);
        assert_eq!(progress.snapshot()["wall_timing"]["vad"]["count"], 1);
    }

    #[test]
    fn drop_does_not_wait_for_observer_io_and_observer_exits_when_released() {
        let (entered, entry) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (exited, exit) = mpsc::channel();
        let progress = InputProgress::with_observer(Duration::from_millis(1), move |_, ended| {
            if ended {
                exited.send(()).unwrap();
            } else {
                entered.send(()).unwrap();
                released.recv().unwrap();
            }
        });
        entry.recv_timeout(Duration::from_secs(2)).unwrap();
        let (dropped, done) = mpsc::channel();
        std::thread::spawn(move || {
            drop(progress);
            dropped.send(()).unwrap();
        });
        done.recv_timeout(Duration::from_secs(2))
            .expect("diagnostic IO blocked input teardown");
        release.send(()).unwrap();
        exit.recv_timeout(Duration::from_secs(2)).unwrap();
    }

    #[test]
    fn metadata_counts_and_stage_labels_do_not_carry_operation_results() {
        let progress = InputProgress::with_observer(Duration::from_secs(60), |_, _| {});
        let result: Result<(), _> = progress.measure(Stage::VoicePoll, || {
            Err("private sample, transcript, meeting title and path")
        });
        assert!(result.is_err());
        let json = progress.snapshot();
        assert!(!json.to_string().contains("private"));
        assert_eq!(json["wall_timing"]["voice_poll"]["count"], 1);
        assert_eq!(json["counters"]["samples_received"], 0);
        assert_eq!(json["counters"]["samples_processed"], 0);
    }
}
