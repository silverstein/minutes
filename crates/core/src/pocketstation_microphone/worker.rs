use super::capture_error;
use super::chunks::MicrophoneAudioChunkWriter;
use super::health::{
    MicrophoneObservationEvaluator, MicrophoneObservations, MicrophoneSignalState,
    EXACT_ZERO_TIMEOUT, FIRST_FRAME_TIMEOUT, MINIMUM_PEAK_DBFS, MINIMUM_RMS_DBFS, STALL_TIMEOUT,
};
use super::recovery::{replacement_continuity_reached, MicrophoneControl};
use super::selection::MicrophoneDiagnosticIdentity;
use crate::error::CaptureError;
use crate::streaming::AudioChunk;
use pocketstation::{
    CaptureNativeFormat, RunningSession, SessionEventKind, SessionSourceActivityPolicy,
    SessionSourceReplacementError, SessionSourceSignalPolicy, StemId,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

const AUDIO_POLL_TIMEOUT: Duration = Duration::from_millis(50);
const WORKER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const OUTPUT_QUEUE_CAPACITY_CHUNKS: usize = 64;
const CONTROL_QUEUE_CAPACITY: usize = 4;

#[derive(Clone)]
struct WorkerState {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    cancel_completed: Arc<AtomicBool>,
    cancel_succeeded: Arc<AtomicBool>,
    source_failed: Arc<AtomicBool>,
    dropped_chunks_total: Arc<AtomicU64>,
    observations: Arc<Mutex<MicrophoneObservations>>,
}

impl WorkerState {
    fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            failed: Arc::new(AtomicBool::new(false)),
            cancel_completed: Arc::new(AtomicBool::new(false)),
            cancel_succeeded: Arc::new(AtomicBool::new(false)),
            source_failed: Arc::new(AtomicBool::new(false)),
            dropped_chunks_total: Arc::new(AtomicU64::new(0)),
            observations: Arc::new(Mutex::new(MicrophoneObservations::default())),
        }
    }
}

struct WorkerInputs {
    running: RunningSession,
    stem_id: StemId,
    audio_sink: crossbeam_channel::Sender<AudioChunk>,
    control: crossbeam_channel::Receiver<MicrophoneControl>,
    state: WorkerState,
    diagnostic_identity: MicrophoneDiagnosticIdentity,
    finished: crossbeam_channel::Sender<()>,
}

pub(super) struct StartedMicrophoneWorker {
    pub(super) worker: MicrophoneWorker,
    pub(super) receiver: crossbeam_channel::Receiver<AudioChunk>,
}

pub(super) struct MicrophoneWorker {
    state: WorkerState,
    control: crossbeam_channel::Sender<MicrophoneControl>,
    finished: crossbeam_channel::Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl MicrophoneWorker {
    pub(super) fn start(
        running: RunningSession,
        stem_id: StemId,
        diagnostic_identity: MicrophoneDiagnosticIdentity,
    ) -> Result<StartedMicrophoneWorker, CaptureError> {
        let (audio_sink, receiver) = crossbeam_channel::bounded(OUTPUT_QUEUE_CAPACITY_CHUNKS);
        let (control, control_receiver) = crossbeam_channel::bounded(CONTROL_QUEUE_CAPACITY);
        let state = WorkerState::new();
        let (finished_sender, finished) = crossbeam_channel::bounded(1);
        let thread = spawn(WorkerInputs {
            running,
            stem_id,
            audio_sink,
            control: control_receiver,
            state: state.clone(),
            diagnostic_identity,
            finished: finished_sender,
        })?;

        Ok(StartedMicrophoneWorker {
            worker: Self {
                state,
                control,
                finished,
                thread: Some(thread),
            },
            receiver,
        })
    }

    pub(super) fn has_error(&self) -> bool {
        self.state.failed.load(Ordering::Relaxed)
    }

    pub(super) fn mark_failed(&self) {
        self.state.failed.store(true, Ordering::Relaxed);
    }

    pub(super) fn observations(&self) -> MicrophoneObservations {
        self.state
            .observations
            .lock()
            .map(|observations| *observations)
            .unwrap_or(MicrophoneObservations {
                state: MicrophoneSignalState::SourceFailed,
                ..MicrophoneObservations::default()
            })
    }

    pub(super) fn control(&self) -> &crossbeam_channel::Sender<MicrophoneControl> {
        &self.control
    }
}

impl Drop for MicrophoneWorker {
    fn drop(&mut self) {
        if !stop_cancel_and_join(
            &self.state.stop,
            self.thread.take(),
            &self.state.cancel_completed,
            &self.state.cancel_succeeded,
            &self.finished,
            WORKER_SHUTDOWN_TIMEOUT,
        ) {
            self.mark_failed();
            tracing::error!("PocketStation microphone worker did not cancel cleanly before join");
        }
        let dropped_chunks_total = self.state.dropped_chunks_total.load(Ordering::Relaxed);
        if dropped_chunks_total > 0 {
            tracing::warn!(
                dropped_chunks_total,
                "Minutes dropped PocketStation microphone audio because its queue was full"
            );
        }
    }
}

fn spawn(inputs: WorkerInputs) -> Result<JoinHandle<()>, CaptureError> {
    std::thread::Builder::new()
        .name("minutes-pocketstation-microphone".into())
        .spawn(move || run(inputs))
        .map_err(|error| capture_error("start PocketStation microphone worker", error))
}

fn run(inputs: WorkerInputs) {
    let WorkerInputs {
        mut running,
        stem_id,
        audio_sink,
        control,
        state,
        diagnostic_identity,
        finished,
    } = inputs;
    let activity_policy = SessionSourceActivityPolicy::new(FIRST_FRAME_TIMEOUT, STALL_TIMEOUT)
        .expect("fixed microphone activity policy must be valid");
    let signal_policy =
        SessionSourceSignalPolicy::new(MINIMUM_PEAK_DBFS, MINIMUM_RMS_DBFS, EXACT_ZERO_TIMEOUT)
            .expect("fixed microphone signal policy must be valid");
    let mut writer = MicrophoneAudioChunkWriter::default();
    let mut observation_evaluator =
        MicrophoneObservationEvaluator::new(activity_policy, signal_policy);
    let mut diagnostic_identity = diagnostic_identity;
    let mut pending_diagnostic_identity = None;
    let mut logged_native_format_attachment = None;
    let mut pending_format_continuity = None;

    while !state.stop.load(Ordering::Relaxed) {
        while let Ok(command) = control.try_recv() {
            state.source_failed.store(false, Ordering::Relaxed);
            let continuity_before_request = state
                .observations
                .lock()
                .map(|entry| (entry.source_generation, entry.discontinuity_epoch))
                .unwrap_or((1, 0));
            let (result, identity, response) = match command {
                MicrophoneControl::Reopen {
                    selector,
                    identity,
                    response,
                } => (
                    running.reopen_microphone_source(stem_id, selector),
                    identity,
                    response,
                ),
                MicrophoneControl::Replace {
                    selector,
                    identity,
                    response,
                } => (
                    running.replace_microphone_source(stem_id, selector),
                    identity,
                    response,
                ),
            };
            match &result {
                Ok(replacement) => {
                    diagnostic_identity = identity;
                    pending_diagnostic_identity = None;
                    logged_native_format_attachment = None;
                    pending_format_continuity = Some((
                        replacement.source_generation,
                        replacement.discontinuity_epoch,
                    ));
                }
                Err(SessionSourceReplacementError::ResponseTimedOut { .. }) => {
                    pending_diagnostic_identity = Some((
                        identity,
                        continuity_before_request.0.saturating_add(1),
                        continuity_before_request.1.saturating_add(1),
                    ));
                }
                Err(_) => {}
            }
            writer.reset_for_discontinuity();
            let _ = response.try_send(result);
        }

        match running.wait_audio(AUDIO_POLL_TIMEOUT) {
            Ok(Some(batch)) => {
                for frame_index in 0..batch.len() {
                    let Some(frame) = batch.frame(frame_index) else {
                        eprintln!(
                            "[minutes] PocketStation microphone returned an invalid frame lease at batch index {frame_index}"
                        );
                        state.failed.store(true, Ordering::Relaxed);
                        break;
                    };
                    if frame.lineage().stem_id() != stem_id {
                        continue;
                    }
                    if let Err(error) =
                        writer.write_frame(frame, &audio_sink, &state.dropped_chunks_total)
                    {
                        eprintln!(
                            "[minutes] PocketStation microphone frame conversion failed: {error}"
                        );
                        tracing::error!(error = %error, "PocketStation microphone stopped");
                        state.failed.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("[minutes] PocketStation microphone polling failed: {error}");
                tracing::error!(error = %error, "PocketStation microphone polling failed");
                state.failed.store(true, Ordering::Relaxed);
            }
        }

        while let pocketstation::SessionEventReceive::Event(event) = running.try_recv_event() {
            match event.kind() {
                SessionEventKind::Source(failure) => {
                    eprintln!(
                        "[minutes] PocketStation microphone source failed: {:?}",
                        failure.event()
                    );
                    state.source_failed.store(true, Ordering::Relaxed);
                }
                SessionEventKind::Endpoint(_)
                | SessionEventKind::Rollback(_)
                | SessionEventKind::Finalization(_)
                | SessionEventKind::Lifecycle(pocketstation::SessionLifecycleState::Failed) => {
                    eprintln!(
                        "[minutes] PocketStation microphone session failed: {:?}",
                        event.kind()
                    );
                    state.failed.store(true, Ordering::Relaxed);
                }
                SessionEventKind::Terminal(outcome)
                    if outcome.state() == pocketstation::SessionTerminalState::Failed =>
                {
                    eprintln!(
                        "[minutes] PocketStation microphone terminal failure: {:?}",
                        outcome
                    );
                    state.failed.store(true, Ordering::Relaxed);
                }
                _ => {}
            }
        }

        if let Some(current_observations) = observation_evaluator.update_observations(
            &running,
            stem_id,
            state.source_failed.load(Ordering::Relaxed),
            &state.observations,
        ) {
            if pending_diagnostic_identity.as_ref().is_some_and(
                |(_, source_generation, discontinuity_epoch)| {
                    replacement_continuity_reached(
                        current_observations,
                        *source_generation,
                        *discontinuity_epoch,
                    )
                },
            ) {
                if let Some((identity, source_generation, discontinuity_epoch)) =
                    pending_diagnostic_identity.take()
                {
                    diagnostic_identity = identity;
                    logged_native_format_attachment = None;
                    pending_format_continuity = Some((source_generation, discontinuity_epoch));
                }
            }
            if report_opened_native_format_once(
                current_observations,
                &diagnostic_identity,
                pending_format_continuity,
                &mut logged_native_format_attachment,
            ) {
                pending_format_continuity = None;
            }
        }
        if state.failed.load(Ordering::Relaxed) {
            break;
        }
    }

    let cancellation_succeeded = running.cancel().is_success();
    state
        .cancel_succeeded
        .store(cancellation_succeeded, Ordering::Release);
    state.cancel_completed.store(true, Ordering::Release);
    if !cancellation_succeeded {
        eprintln!("[minutes] PocketStation microphone cancellation failed");
        state.failed.store(true, Ordering::Relaxed);
    }
    let _ = finished.try_send(());
}

pub(super) fn stop_cancel_and_join(
    stop: &AtomicBool,
    worker: Option<JoinHandle<()>>,
    cancel_completed: &AtomicBool,
    cancel_succeeded: &AtomicBool,
    worker_finished: &crossbeam_channel::Receiver<()>,
    timeout: Duration,
) -> bool {
    stop.store(true, Ordering::Release);
    let Some(worker) = worker else {
        return cancel_completed.load(Ordering::Acquire)
            && cancel_succeeded.load(Ordering::Acquire);
    };
    match worker_finished.recv_timeout(timeout) {
        Ok(()) => {
            worker.join().is_ok()
                && cancel_completed.load(Ordering::Acquire)
                && cancel_succeeded.load(Ordering::Acquire)
        }
        Err(_) => {
            drop(worker);
            false
        }
    }
}

fn report_opened_native_format_once(
    observations: MicrophoneObservations,
    identity: &MicrophoneDiagnosticIdentity,
    minimum_continuity: Option<(u32, u64)>,
    logged: &mut Option<(u32, String)>,
) -> bool {
    if minimum_continuity.is_some_and(|minimum| {
        (
            observations.source_generation,
            observations.discontinuity_epoch,
        ) < minimum
    }) {
        return false;
    }
    let Some(native_format) = observations.native_format else {
        return false;
    };
    let key = opened_native_format_diagnostic_key(observations, identity);
    if logged.as_ref() == Some(&key) {
        return false;
    }

    let entry = opened_native_format_log_entry(
        identity,
        native_format,
        observations.source_generation,
        observations.discontinuity_epoch,
    );
    eprintln!(
        "[minutes] PocketStation microphone opened: {} Hz, {} channel(s), {:?}",
        native_format.sample_rate_hz,
        native_format.channel_count,
        native_format.sample_representation
    );
    if let Err(error) = crate::logging::append_log(&entry) {
        tracing::warn!(%error, "failed to persist PocketStation microphone format diagnostic");
    }
    *logged = Some(key);
    true
}

pub(super) fn opened_native_format_diagnostic_key(
    observations: MicrophoneObservations,
    identity: &MicrophoneDiagnosticIdentity,
) -> (u32, String) {
    (observations.source_generation, identity.device_id.clone())
}

pub(super) fn opened_native_format_log_entry(
    identity: &MicrophoneDiagnosticIdentity,
    native_format: CaptureNativeFormat,
    source_generation: u32,
    discontinuity_epoch: u64,
) -> serde_json::Value {
    serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "level": "info",
        "step": "pocketstation_microphone_opened_format",
        "device_id": identity.device_id,
        "device_name": identity.display_name,
        "sample_rate_hz": native_format.sample_rate_hz,
        "channel_count": native_format.channel_count,
        "sample_representation": format!("{:?}", native_format.sample_representation),
        "source_generation": source_generation,
        "discontinuity_epoch": discontinuity_epoch,
        "message": "PocketStation microphone opened a native input format",
    })
}
