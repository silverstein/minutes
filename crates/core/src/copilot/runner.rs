use super::{
    CancelToken, CopilotHealth, CopilotModel, CopilotRequest, CopilotState, ModelErrorKind,
    ModelStreamEvent, Nudge, NudgePolicy,
};
use chrono::{DateTime, Utc};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
pub enum RunnerEvent {
    StateChanged(CopilotState),
    Model(ModelStreamEvent),
    Nudge(Nudge),
    RequestCancelled { evidence_revision: u64 },
    Degraded { error: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitOutcome {
    Queued,
    CancelledOlderRequest,
    IgnoredNotMateriallyNewer,
    IgnoredWhilePaused,
    IgnoredAfterStop,
}

enum RunnerCommand {
    Request(Box<CopilotRequest>),
    Wake,
    Stop,
}

struct RunnerRuntime {
    state: CopilotState,
    provider: String,
    model: String,
    current_request: Option<CopilotRequest>,
    current_cancel: Option<CancelToken>,
    latest_evidence_revision: Option<u64>,
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
    command_tx: Sender<RunnerCommand>,
    event_rx: Mutex<Receiver<RunnerEvent>>,
    runtime: Arc<Mutex<RunnerRuntime>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl CopilotRunner {
    pub fn start(model: Arc<dyn CopilotModel>, policy: NudgePolicy) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let runtime = Arc::new(Mutex::new(RunnerRuntime {
            state: CopilotState::Arming,
            provider: model.provider_name().into(),
            model: model.model_name().into(),
            current_request: None,
            current_cancel: None,
            latest_evidence_revision: None,
            last_error: None,
            nudge_expires_at: None,
            paused: false,
            stopped: false,
        }));
        let worker_runtime = runtime.clone();
        let worker = std::thread::spawn(move || {
            run_worker(model, policy, command_rx, event_tx, worker_runtime)
        });
        Self {
            command_tx,
            event_rx: Mutex::new(event_rx),
            runtime,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub fn submit(&self, request: CopilotRequest) -> SubmitOutcome {
        let mut runtime = self.runtime.lock().unwrap();
        if runtime.stopped {
            return SubmitOutcome::IgnoredAfterStop;
        }
        if runtime.paused {
            return SubmitOutcome::IgnoredWhilePaused;
        }

        let mut outcome = SubmitOutcome::Queued;
        if let Some(current) = runtime.current_request.as_ref() {
            if !request.materially_newer_than(current) {
                return SubmitOutcome::IgnoredNotMateriallyNewer;
            }
            if let Some(cancel) = runtime.current_cancel.as_ref() {
                cancel.cancel();
                outcome = SubmitOutcome::CancelledOlderRequest;
            }
        }
        runtime.latest_evidence_revision = Some(
            runtime
                .latest_evidence_revision
                .unwrap_or(0)
                .max(request.evidence_revision),
        );
        drop(runtime);

        if self
            .command_tx
            .send(RunnerCommand::Request(Box::new(request)))
            .is_err()
        {
            return SubmitOutcome::IgnoredAfterStop;
        }
        outcome
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
        let _ = self.command_tx.send(RunnerCommand::Wake);
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
        let _ = self.command_tx.send(RunnerCommand::Wake);
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
            in_flight_revision: runtime
                .current_request
                .as_ref()
                .map(|request| request.evidence_revision),
            latest_evidence_revision: runtime.latest_evidence_revision,
            last_error: runtime.last_error.clone(),
            updated_ts: Utc::now(),
        }
    }

    pub fn try_recv(&self) -> Option<RunnerEvent> {
        self.event_rx.lock().unwrap().try_recv().ok()
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<RunnerEvent> {
        self.event_rx.lock().unwrap().recv_timeout(timeout).ok()
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
        let _ = self.command_tx.send(RunnerCommand::Stop);
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
    command_rx: Receiver<RunnerCommand>,
    event_tx: Sender<RunnerEvent>,
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
        let mut request = match command {
            RunnerCommand::Request(request) => *request,
            RunnerCommand::Wake => {
                emit_current_state(&runtime, &event_tx);
                continue;
            }
            RunnerCommand::Stop => break,
        };

        let mut should_stop = false;
        while let Ok(next) = command_rx.try_recv() {
            match next {
                RunnerCommand::Request(candidate)
                    if candidate.evidence_revision >= request.evidence_revision =>
                {
                    request = *candidate;
                }
                RunnerCommand::Request(_) | RunnerCommand::Wake => {}
                RunnerCommand::Stop => should_stop = true,
            }
        }
        if should_stop {
            break;
        }

        {
            let runtime = runtime.lock().unwrap();
            if runtime.stopped {
                break;
            }
            if runtime.paused {
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
        }
        let _ = event_tx.send(RunnerEvent::StateChanged(CopilotState::Thinking));

        let stream_tx = event_tx.clone();
        let stream_sink = move |event| {
            let _ = stream_tx.send(RunnerEvent::Model(event));
        };
        let result = model.stream_structured(&request, &cancel, &stream_sink);

        {
            let mut runtime = runtime.lock().unwrap();
            runtime.current_request = None;
            runtime.current_cancel = None;
        }

        let superseded_by_newer_evidence = runtime
            .lock()
            .unwrap()
            .latest_evidence_revision
            .is_some_and(|revision| revision > request.evidence_revision);
        if cancel.is_cancelled()
            || superseded_by_newer_evidence
            || result
                .as_ref()
                .err()
                .is_some_and(|error| error.kind == ModelErrorKind::Cancelled)
        {
            let _ = event_tx.send(RunnerEvent::RequestCancelled {
                evidence_revision: request.evidence_revision,
            });
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
                if let Some(nudge) = policy.accept(draft, request.evidence_revision, Utc::now()) {
                    {
                        let mut runtime = runtime.lock().unwrap();
                        runtime.state = CopilotState::Nudge;
                        runtime.last_error = None;
                        runtime.nudge_expires_at = Some(nudge.expires_at());
                    }
                    let _ = event_tx.send(RunnerEvent::Nudge(nudge));
                    let _ = event_tx.send(RunnerEvent::StateChanged(CopilotState::Nudge));
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

fn set_state(
    runtime: &Arc<Mutex<RunnerRuntime>>,
    event_tx: &Sender<RunnerEvent>,
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
        let _ = event_tx.send(RunnerEvent::Degraded { error });
    }
    let _ = event_tx.send(RunnerEvent::StateChanged(state));
}

fn emit_current_state(runtime: &Arc<Mutex<RunnerRuntime>>, event_tx: &Sender<RunnerEvent>) {
    let state = runtime.lock().unwrap().state;
    let _ = event_tx.send(RunnerEvent::StateChanged(state));
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

    fn request(revision: u64) -> CopilotRequest {
        CopilotRequest {
            goal: "secure next steps".into(),
            evidence_revision: revision,
            update_kind: TranscriptUpdateKind::Final,
            utterances: vec![CopilotUtterance {
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

        assert_eq!(runner.submit(request(10)), SubmitOutcome::Queued);
        assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 10);
        assert_eq!(
            runner.submit(request(11)),
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
        assert_eq!(runner.submit(request(20)), SubmitOutcome::Queued);

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
}
