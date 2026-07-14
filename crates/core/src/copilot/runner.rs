use super::latency::LatencyTracker;
use super::{
    is_decisive_final, topic_keywords, CancelToken, CopilotClock, CopilotFeedback, CopilotHealth,
    CopilotModel, CopilotRequest, CopilotState, GroundingSource, LatencyRecord, ModelErrorKind,
    ModelStreamEvent, Nudge, NudgePolicy, PartialLatencySeed, PolicySnapshot,
    StrategyRefreshReason, StrategyRequest, StrategyState, SystemCopilotClock, TopicShiftDetector,
    TranscriptUpdateKind,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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
const DEPTH_CHANNEL_CAPACITY: usize = 16;

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
    TopicShiftDetected {
        evidence_revision: u64,
    },
    GroundingRefreshed {
        evidence_revision: u64,
    },
    StrategyUpdated {
        evidence_revision: u64,
        reason: StrategyRefreshReason,
    },
    PolicyAdjusted(PolicySnapshot),
    DepthDegraded {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedbackOutcome {
    Queued,
    IgnoredAfterStop,
    DroppedQueueFull,
}

#[derive(Debug, Clone, Copy)]
pub struct DepthLaneConfig {
    pub strategy_interval: Duration,
    pub grounding_interval: Duration,
}

impl DepthLaneConfig {
    pub fn new(strategy_interval: Duration, grounding_interval: Duration) -> Self {
        Self {
            strategy_interval: strategy_interval
                .clamp(Duration::from_secs(30), Duration::from_secs(90)),
            grounding_interval: grounding_interval.max(Duration::from_secs(1)),
        }
    }
}

impl Default for DepthLaneConfig {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), Duration::from_secs(15))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepthLaneSnapshot {
    pub latest_strategy: StrategyState,
    pub strategy_updates: u32,
    pub grounding_refreshes: u32,
    pub topic_shifts: u32,
    pub decisive_finals: u32,
    pub latest_processed_revision: Option<u64>,
    pub last_strategy_reason: Option<StrategyRefreshReason>,
    pub strategy_update_reasons: Vec<StrategyRefreshReason>,
    pub strategy_update_revisions: Vec<u64>,
    pub grounding_refresh_revisions: Vec<u64>,
    pub topic_shift_revisions: Vec<u64>,
    pub last_grounding_error: Option<String>,
    pub last_strategy_error: Option<String>,
}

impl Default for DepthLaneSnapshot {
    fn default() -> Self {
        Self {
            latest_strategy: StrategyState::empty(),
            strategy_updates: 0,
            grounding_refreshes: 0,
            topic_shifts: 0,
            decisive_finals: 0,
            latest_processed_revision: None,
            last_strategy_reason: None,
            strategy_update_reasons: Vec::new(),
            strategy_update_revisions: Vec::new(),
            grounding_refresh_revisions: Vec::new(),
            topic_shift_revisions: Vec::new(),
            last_grounding_error: None,
            last_strategy_error: None,
        }
    }
}

struct DepthShared {
    session_epoch: u64,
    snapshot: DepthLaneSnapshot,
    battle_card: Option<super::BattleCard>,
}

struct FastWorkerContext {
    model: Arc<dyn CopilotModel>,
    policy: NudgePolicy,
    partial_debounce: Duration,
    command_rx: Receiver<RunnerCommand>,
    event_tx: SyncSender<RunnerEvent>,
    runtime: Arc<Mutex<RunnerRuntime>>,
    clock: Arc<dyn CopilotClock>,
    depth_tx: SyncSender<DepthCommand>,
    depth_shared: Arc<Mutex<DepthShared>>,
}

struct DepthWorkerContext {
    model: Arc<dyn CopilotModel>,
    grounding: Option<Arc<dyn GroundingSource>>,
    config: DepthLaneConfig,
    command_rx: Receiver<DepthCommand>,
    event_tx: SyncSender<RunnerEvent>,
    clock: Arc<dyn CopilotClock>,
    active_cancel: Arc<Mutex<Option<CancelToken>>>,
    shared: Arc<Mutex<DepthShared>>,
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
    Feedback {
        nudge_id: String,
        feedback: CopilotFeedback,
    },
    Stop,
}

enum DepthCommand {
    Observe(Box<CopilotRequest>),
    Reset,
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
    policy: PolicySnapshot,
}

/// Fast request runner with an isolated slow strategy/retrieval worker.
///
/// The worker is the only code that invokes `stream_structured`, so two fast
/// requests can never overlap. Submitters can still cancel the current token
/// immediately when materially newer evidence arrives; the worker then drains
/// queued requests and runs only the newest revision.
pub struct CopilotRunner {
    command_tx: SyncSender<RunnerCommand>,
    depth_tx: SyncSender<DepthCommand>,
    event_tx: SyncSender<RunnerEvent>,
    event_rx: Mutex<Receiver<RunnerEvent>>,
    runtime: Arc<Mutex<RunnerRuntime>>,
    clock: Arc<dyn CopilotClock>,
    worker: Mutex<Option<JoinHandle<()>>>,
    depth_worker: Mutex<Option<JoinHandle<()>>>,
    depth_cancel: Arc<Mutex<Option<CancelToken>>>,
    depth_shared: Arc<Mutex<DepthShared>>,
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
        Self::start_with_clock(
            model,
            policy,
            partial_debounce,
            Arc::new(SystemCopilotClock),
        )
    }

    /// Start the runner with an injected time source.
    ///
    /// This is the deterministic replay seam. Runtime callers should normally
    /// use [`Self::start`] or [`Self::start_with_debounce`].
    pub fn start_with_clock(
        model: Arc<dyn CopilotModel>,
        policy: NudgePolicy,
        partial_debounce: Duration,
        clock: Arc<dyn CopilotClock>,
    ) -> Self {
        Self::start_internal(
            model,
            policy,
            partial_debounce,
            clock,
            None,
            DepthLaneConfig::default(),
        )
    }

    pub fn start_with_depth(
        model: Arc<dyn CopilotModel>,
        policy: NudgePolicy,
        partial_debounce: Duration,
        grounding: Option<Arc<dyn GroundingSource>>,
        depth_config: DepthLaneConfig,
    ) -> Self {
        Self::start_internal(
            model,
            policy,
            partial_debounce,
            Arc::new(SystemCopilotClock),
            grounding,
            depth_config,
        )
    }

    /// Deterministic replay seam for exercising both lanes with an injected
    /// clock and grounding source.
    pub fn start_with_clock_and_depth(
        model: Arc<dyn CopilotModel>,
        policy: NudgePolicy,
        partial_debounce: Duration,
        clock: Arc<dyn CopilotClock>,
        grounding: Option<Arc<dyn GroundingSource>>,
        depth_config: DepthLaneConfig,
    ) -> Self {
        Self::start_internal(
            model,
            policy,
            partial_debounce,
            clock,
            grounding,
            depth_config,
        )
    }

    fn start_internal(
        model: Arc<dyn CopilotModel>,
        policy: NudgePolicy,
        partial_debounce: Duration,
        clock: Arc<dyn CopilotClock>,
        grounding: Option<Arc<dyn GroundingSource>>,
        depth_config: DepthLaneConfig,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CHANNEL_CAPACITY);
        let (depth_tx, depth_rx) = mpsc::sync_channel(DEPTH_CHANNEL_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CHANNEL_CAPACITY);
        let session_epoch = next_session_epoch();
        let policy_snapshot = policy.snapshot();
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
            policy: policy_snapshot,
        }));
        let depth_cancel = Arc::new(Mutex::new(None));
        let depth_shared = Arc::new(Mutex::new(DepthShared {
            session_epoch,
            snapshot: DepthLaneSnapshot::default(),
            battle_card: None,
        }));
        let worker_runtime = runtime.clone();
        let worker_event_tx = event_tx.clone();
        let worker_clock = Arc::clone(&clock);
        let worker_depth_tx = depth_tx.clone();
        let worker_depth_shared = Arc::clone(&depth_shared);
        let fast_model = Arc::clone(&model);
        let worker = std::thread::spawn(move || {
            run_worker(FastWorkerContext {
                model: fast_model,
                policy,
                partial_debounce,
                command_rx,
                event_tx: worker_event_tx,
                runtime: worker_runtime,
                clock: worker_clock,
                depth_tx: worker_depth_tx,
                depth_shared: worker_depth_shared,
            })
        });
        let slow_event_tx = event_tx.clone();
        let slow_clock = Arc::clone(&clock);
        let slow_cancel = Arc::clone(&depth_cancel);
        let slow_shared = Arc::clone(&depth_shared);
        let depth_worker = std::thread::spawn(move || {
            run_depth_worker(DepthWorkerContext {
                model,
                grounding,
                config: depth_config,
                command_rx: depth_rx,
                event_tx: slow_event_tx,
                clock: slow_clock,
                active_cancel: slow_cancel,
                shared: slow_shared,
            })
        });
        Self {
            command_tx,
            depth_tx,
            event_tx,
            event_rx: Mutex::new(event_rx),
            runtime,
            clock,
            worker: Mutex::new(Some(worker)),
            depth_worker: Mutex::new(Some(depth_worker)),
            depth_cancel,
            depth_shared,
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

        if let Ok(active_depth) = self.depth_cancel.try_lock() {
            if let Some(cancel) = active_depth.as_ref() {
                cancel.cancel();
            }
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
        if let Ok(active_depth) = self.depth_cancel.lock() {
            if let Some(cancel) = active_depth.as_ref() {
                cancel.cancel();
            }
        }
        {
            let mut depth = self.depth_shared.lock().unwrap();
            depth.session_epoch = epoch;
            depth.snapshot = DepthLaneSnapshot::default();
            depth.battle_card = None;
        }
        let _ = self.command_tx.try_send(RunnerCommand::BeginSession);
        let _ = self.depth_tx.try_send(DepthCommand::Reset);
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

    pub fn record_feedback(
        &self,
        nudge_id: impl Into<String>,
        feedback: CopilotFeedback,
    ) -> FeedbackOutcome {
        if self.runtime.lock().unwrap().stopped {
            return FeedbackOutcome::IgnoredAfterStop;
        }
        match self.command_tx.try_send(RunnerCommand::Feedback {
            nudge_id: nudge_id.into(),
            feedback,
        }) {
            Ok(()) => FeedbackOutcome::Queued,
            Err(TrySendError::Full(_)) => FeedbackOutcome::DroppedQueueFull,
            Err(TrySendError::Disconnected(_)) => FeedbackOutcome::IgnoredAfterStop,
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
            policy: runtime.policy.clone(),
            latency_records: runtime.latency.records(),
            updated_ts: self.clock.utc_now(),
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

    /// Process-local slow-lane instrumentation used by status surfaces and
    /// deterministic eval. It is never appended to meeting artifacts.
    pub fn depth_snapshot(&self) -> DepthLaneSnapshot {
        self.depth_shared.lock().unwrap().snapshot.clone()
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
        if let Ok(active_depth) = self.depth_cancel.lock() {
            if let Some(cancel) = active_depth.as_ref() {
                cancel.cancel();
            }
        }
        let _ = self.command_tx.try_send(RunnerCommand::Stop);
        let _ = self.depth_tx.try_send(DepthCommand::Stop);
        if let Some(worker) = self.worker.lock().unwrap().take() {
            let _ = worker.join();
        }
        if let Some(worker) = self.depth_worker.lock().unwrap().take() {
            let _ = worker.join();
        }
    }
}

impl Drop for CopilotRunner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn run_worker(context: FastWorkerContext) {
    let FastWorkerContext {
        model,
        mut policy,
        partial_debounce,
        command_rx,
        event_tx,
        runtime,
        clock,
        depth_tx,
        depth_shared,
    } = context;
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
            RunnerCommand::InvalidatePartials => {
                policy.clear();
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::BeginSession => {
                policy.reset_session();
                runtime.lock().unwrap().policy = policy.snapshot();
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::Wake => {
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::Feedback { nudge_id, feedback } => {
                apply_feedback(&mut policy, &runtime, &event_tx, &nudge_id, feedback);
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
                    Ok(RunnerCommand::Feedback { nudge_id, feedback }) => {
                        apply_feedback(&mut policy, &runtime, &event_tx, &nudge_id, feedback)
                    }
                    Ok(RunnerCommand::InvalidatePartials) => policy.clear(),
                    Ok(RunnerCommand::BeginSession) => {
                        policy.reset_session();
                        runtime.lock().unwrap().policy = policy.snapshot();
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
                RunnerCommand::Feedback { nudge_id, feedback } => {
                    apply_feedback(&mut policy, &runtime, &event_tx, &nudge_id, feedback)
                }
                RunnerCommand::InvalidatePartials => policy.clear(),
                RunnerCommand::BeginSession => {
                    policy.reset_session();
                    runtime.lock().unwrap().policy = policy.snapshot();
                }
                RunnerCommand::Stop => should_stop = true,
            }
        }
        if should_stop {
            break;
        }
        if pending.invalidates_partials {
            policy.clear();
        }
        let mut request = pending.request;

        // A failed try_lock means the depth worker is publishing a new compact
        // snapshot. The fast lane proceeds immediately with its prior request
        // context; it never waits for retrieval or strategy.
        if let Ok(depth) = depth_shared.try_lock() {
            if depth.session_epoch == request.session_epoch {
                request.strategy_state = depth.snapshot.latest_strategy.clone();
                if let Some(battle_card) = depth.battle_card.as_ref() {
                    request.battle_card = battle_card.clone();
                }
            }
        }

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
                clock.monotonic_now(),
            );
        }
        try_emit(&event_tx, RunnerEvent::StateChanged(CopilotState::Thinking));

        let stream_tx = event_tx.clone();
        let stream_runtime = Arc::clone(&runtime);
        let stream_epoch = request.session_epoch;
        let stream_revision = request.evidence_revision;
        let stream_clock = Arc::clone(&clock);
        let stream_sink = move |event| {
            stream_runtime.lock().unwrap().latency.mark_first_token(
                stream_epoch,
                stream_revision,
                stream_clock.monotonic_now(),
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

        if request.update_kind == TranscriptUpdateKind::Final {
            let _ = depth_tx.try_send(DepthCommand::Observe(Box::new(request.clone())));
        }

        match result {
            Ok(draft) => {
                let accepted = policy.accept(draft, &request, clock.utc_now());
                runtime.lock().unwrap().policy = policy.snapshot();
                if let Some(nudge) = accepted {
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
                            clock.monotonic_now(),
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

fn apply_feedback(
    policy: &mut NudgePolicy,
    runtime: &Arc<Mutex<RunnerRuntime>>,
    event_tx: &SyncSender<RunnerEvent>,
    nudge_id: &str,
    feedback: CopilotFeedback,
) {
    if !policy.record_feedback(nudge_id, feedback) {
        return;
    }
    let snapshot = policy.snapshot();
    {
        let mut runtime = runtime.lock().unwrap();
        runtime.policy = snapshot.clone();
        if feedback == CopilotFeedback::Dismissed && runtime.state == CopilotState::Nudge {
            runtime.state = CopilotState::Listening;
            runtime.nudge_expires_at = None;
        }
    }
    try_emit(event_tx, RunnerEvent::PolicyAdjusted(snapshot));
}

fn run_depth_worker(context: DepthWorkerContext) {
    let DepthWorkerContext {
        model,
        grounding,
        config,
        command_rx,
        event_tx,
        clock,
        active_cancel,
        shared,
    } = context;
    let mut detector = TopicShiftDetector::default();
    let mut last_strategy_at: Option<Instant> = None;
    let mut last_grounding_at: Option<Instant> = None;

    while let Ok(command) = command_rx.recv() {
        let request = match command {
            DepthCommand::Observe(request) => *request,
            DepthCommand::Reset => {
                detector.reset();
                last_strategy_at = None;
                last_grounding_at = None;
                let mut shared = shared.lock().unwrap();
                shared.snapshot = DepthLaneSnapshot::default();
                shared.battle_card = None;
                continue;
            }
            DepthCommand::Stop => break,
        };
        if request.update_kind != TranscriptUpdateKind::Final {
            continue;
        }
        if shared.lock().unwrap().session_epoch != request.session_epoch {
            continue;
        }

        let trigger_text = request
            .utterances
            .iter()
            .find(|utterance| {
                utterance.utterance_sequence == request.evidence_utterance_sequence
                    && utterance.revision == request.evidence_utterance_revision
            })
            .map(|utterance| utterance.text.as_str())
            .unwrap_or_default();
        let topic_shift = detector.observe_final(trigger_text);
        let decisive = is_decisive_final(trigger_text);
        let now = clock.monotonic_now();
        if topic_shift.is_some() {
            let mut state = shared.lock().unwrap();
            state.snapshot.topic_shifts = state.snapshot.topic_shifts.saturating_add(1);
            push_revision(
                &mut state.snapshot.topic_shift_revisions,
                request.evidence_revision,
            );
            drop(state);
            try_emit(
                &event_tx,
                RunnerEvent::TopicShiftDetected {
                    evidence_revision: request.evidence_revision,
                },
            );
        }
        if decisive {
            let mut state = shared.lock().unwrap();
            state.snapshot.decisive_finals = state.snapshot.decisive_finals.saturating_add(1);
        }

        let grounding_due = last_grounding_at.is_none()
            || topic_shift.is_some()
            || last_grounding_at.is_some_and(|last| {
                now.saturating_duration_since(last) >= config.grounding_interval
            });
        if grounding_due {
            if let Some(source) = grounding.as_ref() {
                let query = grounding_query(&request);
                match source.refresh(&query) {
                    Ok(card) => {
                        let mut state = shared.lock().unwrap();
                        if state.session_epoch != request.session_epoch {
                            continue;
                        }
                        state.battle_card = Some(card);
                        state.snapshot.grounding_refreshes =
                            state.snapshot.grounding_refreshes.saturating_add(1);
                        state.snapshot.last_grounding_error = None;
                        push_revision(
                            &mut state.snapshot.grounding_refresh_revisions,
                            request.evidence_revision,
                        );
                        drop(state);
                        try_emit(
                            &event_tx,
                            RunnerEvent::GroundingRefreshed {
                                evidence_revision: request.evidence_revision,
                            },
                        );
                    }
                    Err(error) => {
                        let message = error.to_string();
                        let mut state = shared.lock().unwrap();
                        if state.session_epoch == request.session_epoch {
                            state.snapshot.last_grounding_error = Some(message.clone());
                            drop(state);
                            try_emit(
                                &event_tx,
                                RunnerEvent::DepthDegraded {
                                    error: format!("grounding refresh: {message}"),
                                },
                            );
                        }
                    }
                }
            }
            last_grounding_at = Some(now);
        }

        let strategy_reason = if last_strategy_at.is_none() {
            Some(StrategyRefreshReason::Initial)
        } else if topic_shift.is_some() {
            Some(StrategyRefreshReason::TopicShift)
        } else if decisive {
            Some(StrategyRefreshReason::DecisiveFinal)
        } else if last_strategy_at
            .is_some_and(|last| now.saturating_duration_since(last) >= config.strategy_interval)
        {
            Some(StrategyRefreshReason::Cadence)
        } else {
            None
        };

        if let Some(reason) = strategy_reason {
            let (prior_state, battle_card) = {
                let state = shared.lock().unwrap();
                (
                    state.snapshot.latest_strategy.clone(),
                    state
                        .battle_card
                        .clone()
                        .unwrap_or_else(|| request.battle_card.clone()),
                )
            };
            let strategy_request = StrategyRequest {
                goal: request.goal.clone(),
                mode: request.mode,
                evidence_revision: request.evidence_revision,
                reason,
                utterances: request.utterances.clone(),
                battle_card,
                prior_state,
            };
            let cancel = CancelToken::new();
            *active_cancel.lock().unwrap() = Some(cancel.clone());
            let result = model.refresh_strategy(&strategy_request, &cancel);
            active_cancel.lock().unwrap().take();
            if !cancel.is_cancelled() {
                match result {
                    Ok(strategy) => {
                        let mut state = shared.lock().unwrap();
                        if state.session_epoch != request.session_epoch {
                            continue;
                        }
                        state.snapshot.latest_strategy = strategy;
                        state.snapshot.strategy_updates =
                            state.snapshot.strategy_updates.saturating_add(1);
                        state.snapshot.last_strategy_reason = Some(reason);
                        state.snapshot.strategy_update_reasons.push(reason);
                        if state.snapshot.strategy_update_reasons.len() > 64 {
                            state.snapshot.strategy_update_reasons.remove(0);
                        }
                        state.snapshot.last_strategy_error = None;
                        push_revision(
                            &mut state.snapshot.strategy_update_revisions,
                            request.evidence_revision,
                        );
                        drop(state);
                        try_emit(
                            &event_tx,
                            RunnerEvent::StrategyUpdated {
                                evidence_revision: request.evidence_revision,
                                reason,
                            },
                        );
                    }
                    Err(error) if error.kind == ModelErrorKind::Cancelled => {}
                    Err(error) => {
                        let mut state = shared.lock().unwrap();
                        if state.session_epoch == request.session_epoch {
                            state.snapshot.last_strategy_error = Some(error.message.clone());
                            drop(state);
                            try_emit(
                                &event_tx,
                                RunnerEvent::DepthDegraded {
                                    error: format!("strategy refresh: {}", error.message),
                                },
                            );
                        }
                    }
                }
            }
            last_strategy_at = Some(now);
        }
        let mut state = shared.lock().unwrap();
        if state.session_epoch == request.session_epoch {
            state.snapshot.latest_processed_revision = Some(request.evidence_revision);
        }
    }
}

fn grounding_query(request: &CopilotRequest) -> String {
    let mut terms = request
        .utterances
        .iter()
        .filter(|utterance| utterance.update_kind == TranscriptUpdateKind::Final)
        .rev()
        .take(6)
        .flat_map(|utterance| topic_keywords(&utterance.text))
        .take(18)
        .collect::<Vec<_>>();
    terms.sort();
    terms.dedup();
    if terms.is_empty() {
        request.goal.chars().take(240).collect()
    } else {
        terms.join(" ")
    }
}

fn push_revision(revisions: &mut Vec<u64>, revision: u64) {
    const REVISION_HISTORY_LIMIT: usize = 64;
    if revisions.len() == REVISION_HISTORY_LIMIT {
        revisions.remove(0);
    }
    revisions.push(revision);
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
        BattleCard, CopilotUtterance, MeetingMode, ModelError, ModelEventSink, ModelHealth,
        ModelHealthStatus, NudgeDraft, NudgeKind, OpportunityKind, StrategyState,
        TranscriptUpdateKind,
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
                opportunity: OpportunityKind::General,
                confidence: 100,
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
                opportunity: OpportunityKind::General,
                confidence: 100,
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

    struct BlockingDepthModel {
        fast_started: Sender<u64>,
        depth_started: Sender<u64>,
        depth_cancellations: AtomicUsize,
    }

    impl CopilotModel for BlockingDepthModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "blocking-depth"
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
            let _ = self.fast_started.send(request.evidence_revision);
            Ok(NudgeDraft {
                kind: NudgeKind::Hold,
                text: String::new(),
                source_chip: String::new(),
                opportunity: OpportunityKind::General,
                confidence: 100,
            })
        }

        fn refresh_strategy(
            &self,
            request: &StrategyRequest,
            cancel: &CancelToken,
        ) -> Result<StrategyState, ModelError> {
            let _ = self.depth_started.send(request.evidence_revision);
            while !cancel.is_cancelled() {
                std::thread::sleep(Duration::from_millis(2));
            }
            self.depth_cancellations.fetch_add(1, Ordering::SeqCst);
            Err(ModelError::cancelled())
        }

        fn health(&self) -> ModelHealth {
            ModelHealth {
                provider: "test".into(),
                model: "blocking-depth".into(),
                status: ModelHealthStatus::Available,
                detail: "ok".into(),
                checked_ts: Utc::now(),
            }
        }
    }

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
                opportunity: OpportunityKind::General,
                confidence: 100,
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
            mode: MeetingMode::Generic,
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
            strategy_state: StrategyState::empty(),
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
            mode: MeetingMode::Generic,
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
            strategy_state: StrategyState::empty(),
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

    #[test]
    fn blocked_depth_lane_is_cancelled_without_delaying_next_fast_request() {
        let (fast_tx, fast_rx) = mpsc::channel();
        let (depth_tx, depth_rx) = mpsc::channel();
        let model = Arc::new(BlockingDepthModel {
            fast_started: fast_tx,
            depth_started: depth_tx,
            depth_cancellations: AtomicUsize::new(0),
        });
        let runner = CopilotRunner::start(model.clone(), NudgePolicy::new(12_000));
        let epoch = runner.session_epoch();
        assert_eq!(runner.submit(request(epoch, 1)), SubmitOutcome::Queued);
        assert_eq!(fast_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);
        assert_eq!(depth_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

        let submitted_at = Instant::now();
        assert_eq!(runner.submit(request(epoch, 2)), SubmitOutcome::Queued);
        assert_eq!(fast_rx.recv_timeout(Duration::from_millis(250)).unwrap(), 2);
        assert!(submitted_at.elapsed() < Duration::from_millis(250));

        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline && model.depth_cancellations.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        assert!(model.depth_cancellations.load(Ordering::SeqCst) > 0);
        runner.stop();
    }
}
