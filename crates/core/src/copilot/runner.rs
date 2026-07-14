use super::latency::LatencyTracker;
use super::{
    CancelToken, CopilotHealth, CopilotModel, CopilotRequest, CopilotState, LatencyRecord,
    ModelErrorKind, ModelStreamEvent, Nudge, NudgePolicy, PartialLatencySeed, TranscriptUpdateKind,
};
use chrono::{DateTime, Utc};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(test)]
use std::sync::mpsc::Sender;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static NEXT_SESSION_EPOCH: AtomicU64 = AtomicU64::new(1);
// Both lanes use nonblocking try_send. Request saturation drops work only
// after freshness has advanced (so older advice is still suppressed); event
// saturation drops transient UI/model updates rather than growing memory.
const COMMAND_CHANNEL_CAPACITY: usize = 32;
const EVENT_CHANNEL_CAPACITY: usize = 256;

fn next_session_epoch() -> u64 {
    NEXT_SESSION_EPOCH.fetch_add(1, Ordering::Relaxed)
}

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    StateChanged(CopilotState),
    Model(ModelStreamEvent),
    Nudge(Nudge),
    RequestCancelled {
        evidence_revision: u64,
    },
    EvidenceRetracted {
        session_epoch: u64,
        through_utterance_sequence: u64,
    },
    Degraded {
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Queued,
    CancelledOlderRequest,
    IgnoredNotMateriallyNewer,
    IgnoredWhilePaused,
    IgnoredAfterStop,
    IgnoredStaleSession,
    DroppedQueueFull,
}

struct PendingRequest {
    request: CopilotRequest,
    invalidates_partials: bool,
}

enum RunnerCommand {
    Request(Box<PendingRequest>),
    InvalidatePartials,
    BeginSession,
    Wake,
    Stop,
}

struct RunnerRuntime {
    state: CopilotState,
    provider: String,
    model: String,
    current_request: Option<CopilotRequest>,
    current_cancel: Option<CancelToken>,
    session_epoch: u64,
    latest_evidence_revision: Option<u64>,
    latest_partial_identity: Option<(u64, u64)>,
    retracted_through_utterance_sequence: u64,
    latency: LatencyTracker,
    last_error: Option<String>,
    nudge_expires_at: Option<DateTime<Utc>>,
    paused: bool,
    stopped: bool,
}

/// Single-lane request runner.
///
/// The worker is the only code that invokes `stream_structured`, so two fast
/// requests can never overlap. Submitters can still cancel the current token
/// immediately when materially newer evidence arrives; the worker then drains
/// queued requests and runs only the newest revision.
pub struct CopilotRunner {
    command_tx: SyncSender<RunnerCommand>,
    event_tx: SyncSender<RunnerEvent>,
    event_rx: Mutex<Receiver<RunnerEvent>>,
    runtime: Arc<Mutex<RunnerRuntime>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CopilotRunner {
    pub fn start(model: Arc<dyn CopilotModel>, policy: NudgePolicy) -> Self {
        Self::start_with_debounce(model, policy, Duration::ZERO)
    }

    pub fn start_with_debounce(
        model: Arc<dyn CopilotModel>,
        policy: NudgePolicy,
        partial_debounce: Duration,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
        let session_epoch = next_session_epoch();
        let runtime = Arc::new(Mutex::new(RunnerRuntime {
            state: CopilotState::Arming,
            provider: model.provider_name().into(),
            model: model.model_name().into(),
            current_request: None,
            current_cancel: None,
            session_epoch,
            latest_evidence_revision: None,
            latest_partial_identity: None,
            retracted_through_utterance_sequence: 0,
            latency: LatencyTracker::default(),
            last_error: None,
            nudge_expires_at: None,
            paused: false,
            stopped: false,
        }));
        let worker_runtime = runtime.clone();
        let worker_event_tx = event_tx.clone();
        let worker = std::thread::spawn(move || {
            run_worker(
                model,
                policy,
                partial_debounce,
                command_rx,
                worker_event_tx,
                worker_runtime,
            )
        });
        Self {
            command_tx,
            event_tx,
            event_rx: Mutex::new(event_rx),
            runtime,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn submit(&self, request: CopilotRequest) -> SubmitOutcome {
        self.submit_inner(request, None)
    }

    pub fn submit_with_latency(
        &self,
        request: CopilotRequest,
        latency: PartialLatencySeed,
    ) -> SubmitOutcome {
        self.submit_inner(request, Some(latency))
    }

    fn submit_inner(
        &self,
        request: CopilotRequest,
        latency: Option<PartialLatencySeed>,
    ) -> SubmitOutcome {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.stopped {
            return SubmitOutcome::IgnoredAfterStop;
        }
        if runtime.paused {
            return SubmitOutcome::IgnoredWhilePaused;
        }
        if request.session_epoch != runtime.session_epoch {
            return SubmitOutcome::IgnoredStaleSession;
        }

        let mut outcome = SubmitOutcome::Queued;
        if let Some(current) = runtime.current_request.as_ref() {
            if current.session_epoch == request.session_epoch {
                if !request.materially_newer_than(current) {
                    return SubmitOutcome::IgnoredNotMateriallyNewer;
                }
                if let Some(cancel) = runtime.current_cancel.as_ref() {
                    cancel.cancel();
                    outcome = SubmitOutcome::CancelledOlderRequest;
                }
            }
        }
        if runtime
            .latest_evidence_revision
            .is_some_and(|revision| request.evidence_revision <= revision)
        {
            return SubmitOutcome::IgnoredNotMateriallyNewer;
        }
        let invalidated_partial_through = if request.update_kind == TranscriptUpdateKind::Final {
            runtime.latest_partial_identity.take().map(|(sequence, _)| {
                runtime.retracted_through_utterance_sequence =
                    runtime.retracted_through_utterance_sequence.max(sequence);
                runtime.nudge_expires_at = None;
                if runtime.state == CopilotState::Nudge {
                    runtime.state = CopilotState::Listening;
                }
                sequence
            })
        } else {
            None
        };
        runtime.latest_evidence_revision = Some(request.evidence_revision);
        if request.update_kind == TranscriptUpdateKind::Partial {
            runtime.latest_partial_identity = Some((
                request.evidence_utterance_sequence,
                request.evidence_utterance_revision,
            ));
        }
        if let Some(seed) = latency {
            runtime.latency.begin(request.evidence_revision, seed);
        }
        drop(runtime);

        if let Some(through_utterance_sequence) = invalidated_partial_through {
            try_emit(
                &self.event_tx,
                RunnerEvent::EvidenceRetracted {
                    session_epoch: request.session_epoch,
                    through_utterance_sequence,
                },
            );
        }
        match self
            .command_tx
            .try_send(RunnerCommand::Request(Box::new(PendingRequest {
                request,
                invalidates_partials: invalidated_partial_through.is_some(),
            }))) {
            Ok(()) => outcome,
            Err(TrySendError::Full(_)) => SubmitOutcome::DroppedQueueFull,
            Err(TrySendError::Disconnected(_)) => SubmitOutcome::IgnoredAfterStop,
        }
    }

    pub fn session_epoch(&self) -> u64 {
        self.runtime.lock().unwrap().session_epoch
    }

    /// Start a new in-memory evidence epoch and cancel any prior session's
    /// request. A delayed model result from the old epoch is filtered even if
    /// the provider ignores cooperative cancellation.
    pub fn begin_session(&self) -> u64 {
        let epoch = next_session_epoch();
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.stopped {
                return runtime.session_epoch;
            }
            if let Some(cancel) = runtime.current_cancel.as_ref() {
                cancel.cancel();
            }
            runtime.session_epoch = epoch;
            runtime.latest_evidence_revision = None;
            runtime.latest_partial_identity = None;
            runtime.retracted_through_utterance_sequence = 0;
            runtime.nudge_expires_at = None;
            runtime.state = if runtime.paused {
                CopilotState::Paused
            } else {
                CopilotState::Listening
            };
        }
        let _ = self.command_tx.try_send(RunnerCommand::BeginSession);
        epoch
    }

    /// Observe producer freshness even when the bounded data ring dropped the
    /// newer text. This can only cancel stale advice; it never queues work.
    pub fn supersede_partial_revision(
        &self,
        session_epoch: u64,
        utterance_sequence: u64,
        revision: u64,
    ) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.stopped || runtime.session_epoch != session_epoch {
            return;
        }
        if runtime
            .latest_partial_identity
            .is_none_or(|identity| (utterance_sequence, revision) > identity)
        {
            runtime.latest_partial_identity = Some((utterance_sequence, revision));
        }
        if let Some(current) = runtime.current_request.as_ref() {
            if current
                .grounded_partial_identity()
                .is_some_and(|current_identity| (utterance_sequence, revision) > current_identity)
            {
                if let Some(cancel) = runtime.current_cancel.as_ref() {
                    cancel.cancel();
                }
            }
        }
    }

    pub fn retract_partials(&self, session_epoch: u64, through_utterance_sequence: u64) {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.stopped || runtime.session_epoch != session_epoch {
                return;
            }
            runtime.retracted_through_utterance_sequence = runtime
                .retracted_through_utterance_sequence
                .max(through_utterance_sequence);
            if runtime
                .latest_partial_identity
                .is_some_and(|(sequence, _)| sequence <= through_utterance_sequence)
            {
                runtime.latest_partial_identity = None;
            }
            runtime.nudge_expires_at = None;
            if runtime.state == CopilotState::Nudge {
                runtime.state = CopilotState::Listening;
            }
            if let Some(current) = runtime.current_request.as_ref() {
                if current
                    .grounded_partial_identity()
                    .is_some_and(|(sequence, _)| sequence <= through_utterance_sequence)
                {
                    if let Some(cancel) = runtime.current_cancel.as_ref() {
                        cancel.cancel();
                    }
                }
            }
        }
        let _ = self.command_tx.try_send(RunnerCommand::InvalidatePartials);
        try_emit(
            &self.event_tx,
            RunnerEvent::EvidenceRetracted {
                session_epoch,
                through_utterance_sequence,
            },
        );
    }

    pub fn pause(&self) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.stopped {
            return;
        }
        runtime.paused = true;
        runtime.state = CopilotState::Paused;
        runtime.nudge_expires_at = None;
        if let Some(cancel) = runtime.current_cancel.as_ref() {
            cancel.cancel();
        }
        drop(runtime);
        let _ = self.command_tx.try_send(RunnerCommand::Wake);
    }

    pub fn resume(&self) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.stopped {
            return;
        }
        runtime.paused = false;
        runtime.state = CopilotState::Listening;
        runtime.last_error = None;
        drop(runtime);
        let _ = self.command_tx.try_send(RunnerCommand::Wake);
    }

    pub fn tick(&self, now: DateTime<Utc>) {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.state == CopilotState::Nudge
            && runtime
                .nudge_expires_at
                .is_some_and(|expires_at| now >= expires_at)
        {
            runtime.state = CopilotState::Listening;
            runtime.nudge_expires_at = None;
        }
    }

    pub fn health(&self) -> CopilotHealth {
        let runtime = self.runtime.lock().unwrap();
        CopilotHealth {
            state: runtime.state,
            provider: runtime.provider.clone(),
            model: runtime.model.clone(),
            session_epoch: runtime.session_epoch,
            in_flight_revision: runtime
                .current_request
                .as_ref()
                .map(|request| request.evidence_revision),
            latest_evidence_revision: runtime.latest_evidence_revision,
            last_error: runtime.last_error.clone(),
            latency_records: runtime.latency.records(),
            updated_ts: Utc::now(),
        }
    }

    pub fn try_recv(&self) -> Option<RunnerEvent> {
        loop {
            let event = self.event_rx.lock().unwrap().try_recv().ok()?;
            if let RunnerEvent::Nudge(nudge) = &event {
                let runtime = self.runtime.lock().unwrap();
                if !nudge_is_current(&runtime, nudge) {
                    continue;
                }
            }
            return Some(event);
        }
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<RunnerEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = self.event_rx.lock().unwrap().recv_timeout(remaining).ok()?;
            if let RunnerEvent::Nudge(nudge) = &event {
                let runtime = self.runtime.lock().unwrap();
                if !nudge_is_current(&runtime, nudge) {
                    if Instant::now() >= deadline {
                        return None;
                    }
                    continue;
                }
            }
            return Some(event);
        }
    }

    /// Bounded process-local instrumentation. These records are never appended
    /// to the event log or transcript artifacts.
    pub fn latency_records(&self) -> Vec<LatencyRecord> {
        self.runtime.lock().unwrap().latency.records()
    }

    pub fn stop(&self) {
        {
            let mut runtime = self.runtime.lock().unwrap();
            if runtime.stopped {
                return;
            }
            runtime.stopped = true;
            runtime.state = CopilotState::Off;
            if let Some(cancel) = runtime.current_cancel.as_ref() {
                cancel.cancel();
            }
        }
        let _ = self.command_tx.try_send(RunnerCommand::Stop);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CopilotRunner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(
    model: Arc<dyn CopilotModel>,
    mut policy: NudgePolicy,
    partial_debounce: Duration,
    command_rx: Receiver<RunnerCommand>,
    event_tx: SyncSender<RunnerEvent>,
    runtime: Arc<Mutex<RunnerRuntime>>,
) {
    let prewarm_result = model.prewarm();
    let (stopped, paused) = {
        let runtime = runtime.lock().unwrap();
        (runtime.stopped, runtime.paused)
    };
    if stopped {
        set_state(&runtime, &event_tx, CopilotState::Off, None);
        return;
    }
    match prewarm_result {
        Ok(()) => set_state(
            &runtime,
            &event_tx,
            if paused {
                CopilotState::Paused
            } else {
                CopilotState::Listening
            },
            None,
        ),
        Err(error) => set_state(
            &runtime,
            &event_tx,
            if paused {
                CopilotState::Paused
            } else {
                CopilotState::Degraded
            },
            Some(error.message),
        ),
    }

    while let Ok(command) = command_rx.recv() {
        let mut pending = match command {
            RunnerCommand::Request(request) => *request,
            RunnerCommand::InvalidatePartials | RunnerCommand::BeginSession => {
                policy.clear();
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::Wake => {
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::Stop => break,
        };

        let mut should_stop = false;
        if pending.request.update_kind == TranscriptUpdateKind::Partial
            && !partial_debounce.is_zero()
        {
            let deadline = Instant::now() + partial_debounce;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    break;
                }
                match command_rx.recv_timeout(remaining) {
                    Ok(RunnerCommand::Request(candidate))
                        if request_replaces(&candidate.request, &pending.request) =>
                    {
                        let invalidates_partials =
                            pending.invalidates_partials || candidate.invalidates_partials;
                        pending = *candidate;
                        pending.invalidates_partials = invalidates_partials;
                    }
                    Ok(RunnerCommand::Request(_)) | Ok(RunnerCommand::Wake) => {}
                    Ok(RunnerCommand::InvalidatePartials) | Ok(RunnerCommand::BeginSession) => {
                        policy.clear()
                    }
                    Ok(RunnerCommand::Stop) => {
                        should_stop = true;
                        break;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => break,
                    Err(mpsc::RecvTimeoutError::Disconnected) => {
                        should_stop = true;
                        break;
                    }
                }
            }
        }

        while let Ok(next) = command_rx.try_recv() {
            match next {
                RunnerCommand::Request(candidate)
                    if request_replaces(&candidate.request, &pending.request) =>
                {
                    let invalidates_partials =
                        pending.invalidates_partials || candidate.invalidates_partials;
                    pending = *candidate;
                    pending.invalidates_partials = invalidates_partials;
                }
                RunnerCommand::Request(_) | RunnerCommand::Wake => {}
                RunnerCommand::InvalidatePartials | RunnerCommand::BeginSession => policy.clear(),
                RunnerCommand::Stop => should_stop = true,
            }
        }
        if should_stop {
            break;
        }
        if pending.invalidates_partials {
            policy.clear();
        }
        let request = pending.request;

        {
            let runtime = runtime.lock().unwrap();
            if runtime.stopped {
                break;
            }
            if runtime.paused || !request_is_current(&runtime, &request) {
                continue;
            }
        }

        let cancel = CancelToken::new();
        {
            let mut runtime = runtime.lock().unwrap();
            runtime.current_request = Some(request.clone());
            runtime.current_cancel = Some(cancel.clone());
            runtime.state = CopilotState::Thinking;
            runtime.nudge_expires_at = None;
            runtime.latency.mark_model_request(
                request.session_epoch,
                request.evidence_revision,
                Instant::now(),
            );
        }
        try_emit(&event_tx, RunnerEvent::StateChanged(CopilotState::Thinking));

        let stream_tx = event_tx.clone();
        let stream_runtime = Arc::clone(&runtime);
        let stream_epoch = request.session_epoch;
        let stream_revision = request.evidence_revision;
        let stream_sink = move |event| {
            stream_runtime.lock().unwrap().latency.mark_first_token(
                stream_epoch,
                stream_revision,
                Instant::now(),
            );
            try_emit(&stream_tx, RunnerEvent::Model(event));
        };
        let result = model.stream_structured(&request, &cancel, &stream_sink);

        {
            let mut runtime = runtime.lock().unwrap();
            runtime.current_request = None;
            runtime.current_cancel = None;
        }

        let evidence_is_stale = !request_is_current(&runtime.lock().unwrap(), &request);
        if cancel.is_cancelled()
            || evidence_is_stale
            || result
                .as_ref()
                .err()
                .is_some_and(|error| error.kind == ModelErrorKind::Cancelled)
        {
            try_emit(
                &event_tx,
                RunnerEvent::RequestCancelled {
                    evidence_revision: request.evidence_revision,
                },
            );
            let state = if runtime.lock().unwrap().paused {
                CopilotState::Paused
            } else {
                CopilotState::Listening
            };
            set_state(&runtime, &event_tx, state, None);
            continue;
        }

        match result {
            Ok(draft) => {
                if let Some(nudge) = policy.accept(draft, &request, Utc::now()) {
                    {
                        let mut runtime = runtime.lock().unwrap();
                        if !request_is_current(&runtime, &request) {
                            policy.clear();
                            continue;
                        }
                        runtime.state = CopilotState::Nudge;
                        runtime.last_error = None;
                        runtime.nudge_expires_at = Some(nudge.expires_at());
                        runtime.latency.mark_nudge(
                            request.session_epoch,
                            request.evidence_revision,
                            Instant::now(),
                        );
                    }
                    try_emit(&event_tx, RunnerEvent::Nudge(nudge));
                    try_emit(&event_tx, RunnerEvent::StateChanged(CopilotState::Nudge));
                } else {
                    set_state(&runtime, &event_tx, CopilotState::Listening, None);
                }
            }
            Err(error) => {
                policy.clear();
                set_state(
                    &runtime,
                    &event_tx,
                    CopilotState::Degraded,
                    Some(error.message),
                );
            }
        }
    }

    set_state(&runtime, &event_tx, CopilotState::Off, None);
}

fn request_is_current(runtime: &RunnerRuntime, request: &CopilotRequest) -> bool {
    if runtime.session_epoch != request.session_epoch
        || runtime.latest_evidence_revision != Some(request.evidence_revision)
    {
        return false;
    }
    request.grounded_partial_identity().is_none_or(|identity| {
        identity.0 > runtime.retracted_through_utterance_sequence
            && runtime.latest_partial_identity == Some(identity)
    })
}

fn nudge_is_current(runtime: &RunnerRuntime, nudge: &Nudge) -> bool {
    if runtime.session_epoch != nudge.session_epoch
        || runtime
            .latest_evidence_revision
            .is_some_and(|revision| revision > nudge.evidence_revision)
    {
        return false;
    }
    nudge.grounded_partial_identity().is_none_or(|identity| {
        identity.0 > runtime.retracted_through_utterance_sequence
            && runtime.latest_partial_identity == Some(identity)
    })
}

fn request_replaces(candidate: &CopilotRequest, pending: &CopilotRequest) -> bool {
    candidate.session_epoch != pending.session_epoch
        || candidate.evidence_revision >= pending.evidence_revision
}

fn try_emit(event_tx: &SyncSender<RunnerEvent>, event: RunnerEvent) {
    let _ = event_tx.try_send(event);
}

fn set_state(
    runtime: &Arc<Mutex<RunnerRuntime>>,
    event_tx: &SyncSender<RunnerEvent>,
    state: CopilotState,
    error: Option<String>,
) {
    {
        let mut runtime = runtime.lock().unwrap();
        runtime.state = state;
        runtime.last_error = error.clone();
        if state != CopilotState::Nudge {
            runtime.nudge_expires_at = None;
        }
    }
    if let Some(error) = error {
        try_emit(event_tx, RunnerEvent::Degraded { error });
    }
    try_emit(event_tx, RunnerEvent::StateChanged(state));
}

fn emit_current_state(runtime: &Arc<Mutex<RunnerRuntime>>, event_tx: &SyncSender<RunnerEvent>) {
    let state = runtime.lock().unwrap().state;
    try_emit(event_tx, RunnerEvent::StateChanged(state));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::{
        BattleCard, CopilotUtterance, ModelError, ModelEventSink, ModelHealth, ModelHealthStatus,
        NudgeDraft, NudgeKind, TranscriptUpdateKind,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CancellationModel {
        active: AtomicUsize,
        max_active: AtomicUsize,
        calls: AtomicUsize,
        started: Sender<u64>,
    }

    impl CopilotModel for CancellationModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "cancellation"
        }

        fn prewarm(&self) -> Result<(), crate::copilot::ModelError> {
            Ok(())
        }

        fn stream_structured(
            &self,
            request: &CopilotRequest,
            cancel: &CancelToken,
            _sink: &dyn ModelEventSink,
        ) -> Result<NudgeDraft, ModelError> {
            let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active.fetch_max(active, Ordering::SeqCst);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.started.send(request.evidence_revision);
            if call == 0 {
                while !cancel.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(5));
                }
                self.active.fetch_sub(1, Ordering::SeqCst);
                return Err(ModelError::cancelled());
            }
            self.active.fetch_sub(1, Ordering::SeqCst);
            Ok(NudgeDraft {
                kind: NudgeKind::Ask,
                text: "Ask for the owner.".into(),
                source_chip: "owner".into(),
            })
        }

        fn health(&self) -> ModelHealth {
            ModelHealth {
                provider: "test".into(),
                model: "cancellation".into(),
                status: ModelHealthStatus::Available,
                detail: "ok".into(),
                checked_ts: Utc::now(),
            }
        }
    }

    struct TimeoutModel;

    impl CopilotModel for TimeoutModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "timeout"
        }

        fn prewarm(&self) -> Result<(), ModelError> {
            Ok(())
        }

        fn stream_structured(
            &self,
            _request: &CopilotRequest,
            _cancel: &CancelToken,
            _sink: &dyn ModelEventSink,
        ) -> Result<NudgeDraft, ModelError> {
            Err(ModelError::timeout("fast lane exceeded 5 seconds"))
        }

        fn health(&self) -> ModelHealth {
            ModelHealth {
                provider: "test".into(),
                model: "timeout".into(),
                status: ModelHealthStatus::Degraded,
                detail: "timeout".into(),
                checked_ts: Utc::now(),
            }
        }
    }

    struct IgnoringCancellationModel {
        calls: AtomicUsize,
        started: Sender<u64>,
        release_first: Mutex<Receiver<()>>,
    }

    impl CopilotModel for IgnoringCancellationModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "ignores-cancellation"
        }

        fn prewarm(&self) -> Result<(), ModelError> {
            Ok(())
        }

        fn stream_structured(
            &self,
            request: &CopilotRequest,
            _cancel: &CancelToken,
            _sink: &dyn ModelEventSink,
        ) -> Result<NudgeDraft, ModelError> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let _ = self.started.send(request.evidence_revision);
            if call == 0 {
                self.release_first.lock().unwrap().recv().unwrap();
            }
            let text = request
                .utterances
                .last()
                .map(|utterance| utterance.text.clone())
                .unwrap_or_default();
            Ok(NudgeDraft {
                kind: NudgeKind::Say,
                text,
                source_chip: "correction".into(),
            })
        }

        fn health(&self) -> ModelHealth {
            ModelHealth {
                provider: "test".into(),
                model: "ignores-cancellation".into(),
                status: ModelHealthStatus::Available,
                detail: "ok".into(),
                checked_ts: Utc::now(),
            }
        }
    }

    struct StreamingModel;

    impl CopilotModel for StreamingModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "streaming"
        }

        fn prewarm(&self) -> Result<(), ModelError> {
            Ok(())
        }

        fn stream_structured(
            &self,
            _request: &CopilotRequest,
            _cancel: &CancelToken,
            sink: &dyn ModelEventSink,
        ) -> Result<NudgeDraft, ModelError> {
            sink.on_event(ModelStreamEvent::TextDelta("first".into()));
            Ok(NudgeDraft {
                kind: NudgeKind::Ask,
                text: "Ask what changed".into(),
                source_chip: "change".into(),
            })
        }

        fn health(&self) -> ModelHealth {
            ModelHealth {
                provider: "test".into(),
                model: "streaming".into(),
                status: ModelHealthStatus::Available,
                detail: "ok".into(),
                checked_ts: Utc::now(),
            }
        }
    }

    fn request(session_epoch: u64, revision: u64) -> CopilotRequest {
        CopilotRequest {
            goal: "secure next steps".into(),
            session_epoch,
            evidence_revision: revision,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: revision,
            update_kind: TranscriptUpdateKind::Final,
            utterances: vec![CopilotUtterance {
                utterance_sequence: 1,
                revision,
                update_kind: TranscriptUpdateKind::Final,
                source: "system".into(),
                text: format!("revision {revision}"),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 100,
            }],
            battle_card: BattleCard::empty(),
        }
    }

    fn partial_request(
        session_epoch: u64,
        evidence_revision: u64,
        utterance_revision: u64,
        text: &str,
    ) -> CopilotRequest {
        CopilotRequest {
            goal: "secure next steps".into(),
            session_epoch,
            evidence_revision,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: utterance_revision,
            update_kind: TranscriptUpdateKind::Partial,
            utterances: vec![CopilotUtterance {
                utterance_sequence: 1,
                revision: utterance_revision,
                update_kind: TranscriptUpdateKind::Partial,
                source: "in-process-live".into(),
                text: text.into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 100,
            }],
            battle_card: BattleCard::empty(),
        }
    }

    #[test]
    fn materially_newer_request_cancels_before_next_request_starts() {
        let (started_tx, started_rx) = mpsc::channel();
        let model = Arc::new(CancellationModel {
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
            started: started_tx,
        });
        let runner = CopilotRunner::start(model.clone(), NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();

        assert_eq!(runner.submit(request(epoch, 10)), SubmitOutcome::Queued);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 10);
        assert_eq!(
            runner.submit(request(epoch, 11)),
            SubmitOutcome::CancelledOlderRequest
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let mut cancelled = false;
        let mut nudged = false;
        while std::time::Instant::now() < deadline && !(cancelled && nudged) {
            if let Some(event) = runner.recv_timeout(Duration::from_millis(50)) {
                match event {
                    RunnerEvent::RequestCancelled {
                        evidence_revision: 10,
                    } => cancelled = true,
                    RunnerEvent::Nudge(nudge) if nudge.evidence_revision == 11 => nudged = true,
                    _ => {}
                }
            }
        }

        assert!(cancelled);
        assert!(nudged);
        assert_eq!(model.max_active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn provider_timeout_degrades_only_the_copilot_runner() {
        let runner = CopilotRunner::start(Arc::new(TimeoutModel), NudgePolicy::new(12_000));
        assert_eq!(
            runner.submit(request(runner.session_epoch(), 20)),
            SubmitOutcome::Queued
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut degraded = false;
        while std::time::Instant::now() < deadline && !degraded {
            if let Some(RunnerEvent::Degraded { error }) =
                runner.recv_timeout(Duration::from_millis(50))
            {
                assert!(error.contains("5 seconds"));
                degraded = true;
            }
        }
        assert!(degraded);
        assert_eq!(runner.health().state, CopilotState::Degraded);
        // No error is returned through `submit`; capture/transcript producers
        // are not referenced by the runner and therefore cannot be failed.
    }

    #[test]
    fn short_partial_correction_never_surfaces_superseded_nudge() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();

        assert_eq!(
            runner.submit(partial_request(epoch, 1, 1, "Approve")),
            SubmitOutcome::Queued
        );
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert_eq!(
            runner.submit(partial_request(epoch, 2, 2, "Reject")),
            SubmitOutcome::CancelledOlderRequest
        );
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut surfaced_revisions = Vec::new();
        while Instant::now() < deadline && !surfaced_revisions.contains(&2) {
            if let Some(RunnerEvent::Nudge(nudge)) = runner.recv_timeout(Duration::from_millis(50))
            {
                surfaced_revisions.push(nudge.evidence_revision);
                if nudge.evidence_revision == 2 {
                    assert_eq!(nudge.text, "Reject");
                }
            }
        }
        assert_eq!(surfaced_revisions, vec![2]);
    }

    #[test]
    fn partial_grounded_final_is_rejected_then_clean_final_runs() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();

        assert_eq!(
            runner.submit(partial_request(epoch, 1, 1, "Approve")),
            SubmitOutcome::Queued
        );
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

        let mut mixed_final = request(epoch, 2);
        mixed_final.utterances.insert(
            0,
            CopilotUtterance {
                utterance_sequence: 1,
                revision: 1,
                update_kind: TranscriptUpdateKind::Partial,
                source: "in-process-live".into(),
                text: "Approve".into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 100,
            },
        );
        mixed_final.utterances.last_mut().unwrap().text = "Reject".into();
        assert_eq!(
            runner.submit(mixed_final),
            SubmitOutcome::CancelledOlderRequest
        );
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut old_cancelled = false;
        while Instant::now() < deadline && !old_cancelled {
            if matches!(
                runner.recv_timeout(Duration::from_millis(25)),
                Some(RunnerEvent::RequestCancelled {
                    evidence_revision: 1
                })
            ) {
                old_cancelled = true;
            }
        }
        assert!(old_cancelled);
        assert!(
            started_rx.recv_timeout(Duration::from_millis(75)).is_err(),
            "a final prompt retaining superseded partial text reached the model"
        );

        let mut clean_final = request(epoch, 3);
        clean_final.utterances.last_mut().unwrap().text = "Reject".into();
        assert_eq!(runner.submit(clean_final), SubmitOutcome::Queued);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 3);

        let deadline = Instant::now() + Duration::from_secs(1);
        let mut surfaced = None;
        while Instant::now() < deadline && surfaced.is_none() {
            if let Some(RunnerEvent::Nudge(nudge)) = runner.recv_timeout(Duration::from_millis(25))
            {
                surfaced = Some((nudge.evidence_revision, nudge.text));
            }
        }
        assert_eq!(surfaced, Some((3, "Reject".into())));
    }

    #[test]
    fn debounce_coalesces_to_the_latest_partial_before_model_request() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start_with_debounce(
            model,
            NudgePolicy::new(12_000),
            Duration::from_millis(50),
        );
        let epoch = runner.session_epoch();
        assert_eq!(
            runner.submit(partial_request(epoch, 1, 1, "Approve")),
            SubmitOutcome::Queued
        );
        assert_eq!(
            runner.submit(partial_request(epoch, 2, 2, "Reject")),
            SubmitOutcome::Queued
        );
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
        assert!(started_rx.try_recv().is_err());
        release_tx.send(()).unwrap();
    }

    #[test]
    fn retracted_partial_never_surfaces_a_nudge() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();
        assert_eq!(
            runner.submit(partial_request(epoch, 1, 1, "Approve")),
            SubmitOutcome::Queued
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        runner.retract_partials(epoch, 1);
        release_tx.send(()).unwrap();

        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if let Some(event) = runner.recv_timeout(Duration::from_millis(25)) {
                assert!(!matches!(event, RunnerEvent::Nudge(_)));
            }
        }
    }

    #[test]
    fn new_session_epoch_rejects_prior_in_flight_nudge() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
        let old_epoch = runner.session_epoch();
        assert_eq!(
            runner.submit(partial_request(old_epoch, 1, 1, "old session")),
            SubmitOutcome::Queued
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let new_epoch = runner.begin_session();
        assert!(new_epoch > old_epoch);
        release_tx.send(()).unwrap();
        assert_eq!(
            runner.submit(partial_request(new_epoch, 1, 1, "new session")),
            SubmitOutcome::Queued
        );

        let deadline = Instant::now() + Duration::from_secs(2);
        let mut nudged_epoch = None;
        while Instant::now() < deadline && nudged_epoch.is_none() {
            if let Some(RunnerEvent::Nudge(nudge)) = runner.recv_timeout(Duration::from_millis(50))
            {
                nudged_epoch = Some(nudge.session_epoch);
            }
        }
        assert_eq!(nudged_epoch, Some(new_epoch));
    }

    #[test]
    fn debounce_replaces_old_epoch_revision_100_with_new_epoch_revision_1() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start_with_debounce(
            model,
            NudgePolicy::new(12_000),
            Duration::from_millis(75),
        );
        let old_epoch = runner.session_epoch();
        assert_eq!(
            runner.submit(partial_request(old_epoch, 100, 100, "old")),
            SubmitOutcome::Queued
        );
        let new_epoch = runner.begin_session();
        assert_eq!(
            runner.submit(partial_request(new_epoch, 1, 1, "new")),
            SubmitOutcome::Queued
        );

        assert_eq!(
            started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            1,
            "new epoch must replace a higher-revision stale debounce candidate"
        );
        release_tx.send(()).unwrap();
    }

    #[test]
    fn stalled_provider_cannot_grow_runner_command_queue_without_bound() {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let model = Arc::new(IgnoringCancellationModel {
            calls: AtomicUsize::new(0),
            started: started_tx,
            release_first: Mutex::new(release_rx),
        });
        let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();
        assert_eq!(
            runner.submit(partial_request(epoch, 1, 1, "stalled")),
            SubmitOutcome::Queued
        );
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let mut dropped = 0;
        for revision in 2..=(COMMAND_CHANNEL_CAPACITY as u64 + 10) {
            if runner.submit(partial_request(epoch, revision, revision, "new"))
                == SubmitOutcome::DroppedQueueFull
            {
                dropped += 1;
            }
        }
        assert!(
            dropped > 0,
            "bounded command queue never reported saturation"
        );
        assert_eq!(
            runner.health().latest_evidence_revision,
            Some(COMMAND_CHANNEL_CAPACITY as u64 + 10),
            "dropped work must still advance freshness and suppress stale advice"
        );

        release_tx.send(()).unwrap();
        runner.stop();
    }

    #[test]
    fn latency_status_records_every_in_process_stage_without_persistence() {
        let runner = CopilotRunner::start(Arc::new(StreamingModel), NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();
        let audio_received_at = Instant::now();
        let seed = PartialLatencySeed {
            session_epoch: epoch,
            utterance_sequence: 1,
            utterance_revision: 1,
            audio_received_at,
            partial_published_at: audio_received_at + Duration::from_millis(1),
            trigger_at: audio_received_at + Duration::from_millis(2),
            context_ready_at: audio_received_at + Duration::from_millis(3),
        };
        assert_eq!(
            runner.submit_with_latency(partial_request(epoch, 1, 1, "hello"), seed),
            SubmitOutcome::Queued
        );

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if matches!(
                runner.recv_timeout(Duration::from_millis(25)),
                Some(RunnerEvent::Nudge(_))
            ) {
                break;
            }
        }
        let records = runner.health().latency_records;
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert!(record.model_request_us.is_some());
        assert!(record.first_token_us.is_some());
        assert!(record.nudge_us.is_some());
        let serialized_health = serde_json::to_value(runner.health()).unwrap();
        assert!(serialized_health.get("latency_records").is_none());
    }
}
