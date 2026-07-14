use super::{
    CancelToken, CopilotModel, CopilotRequest, ModelError, ModelErrorKind, ModelEventSink,
    ModelHealth, ModelHealthStatus, NudgeDraft,
};
#[cfg(any(test, target_os = "macos"))]
use super::{ModelStreamEvent, NudgeKind};
use crate::config::CopilotConfig;
use chrono::Utc;
#[cfg(any(test, target_os = "macos"))]
use serde::Deserialize;
#[cfg(any(test, target_os = "macos"))]
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;
#[cfg(any(test, target_os = "macos"))]
use std::time::Instant;

#[cfg(target_os = "macos")]
use serde::Serialize;
#[cfg(target_os = "macos")]
use std::io::{BufRead, BufReader, Write};
#[cfg(target_os = "macos")]
use std::process::{Child, ChildStdin, Command, Stdio};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "macos")]
use std::sync::{mpsc, Arc, Mutex};
#[cfg(target_os = "macos")]
use std::thread::JoinHandle;

pub const APPLE_FM_COPILOT_MODEL: &str = "system-language-model";
const HELPER_PROTOCOL_VERSION: u32 = 1;
#[cfg(target_os = "macos")]
const PREWARM_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(test, target_os = "macos"))]
const CANCEL_GRACE: Duration = Duration::from_secs(2);
#[cfg(any(test, target_os = "macos"))]
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Prompt/schema revision included in the Apple FM replay-gate key.
///
/// Apple can update the on-device model with the OS. A future replay harness
/// should require an accepted fixture run for each previously unseen
/// `(prompt version, helper protocol, OS/model runtime)` key before enabling
/// that runtime by default. This PR deliberately provides the seam, not the
/// full evaluation harness.
pub const APPLE_FM_COPILOT_PROMPT_VERSION: u32 = 1;

pub fn replay_gate_key(os_version: Option<&str>) -> String {
    format!(
        "apple-fm-copilot/prompt-v{APPLE_FM_COPILOT_PROMPT_VERSION}/protocol-v{HELPER_PROTOCOL_VERSION}/macos-{}",
        os_version.unwrap_or("unknown")
    )
}

#[derive(Clone)]
pub struct AppleFoundationCopilotModel {
    model: String,
    timeout: Duration,
    #[cfg(target_os = "macos")]
    runtime: Arc<MacRuntime>,
}

impl std::fmt::Debug for AppleFoundationCopilotModel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AppleFoundationCopilotModel")
            .field("model", &self.model)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl AppleFoundationCopilotModel {
    pub fn new(model: impl Into<String>) -> Self {
        Self::with_timeout(model, Duration::from_secs(5))
    }

    pub fn with_timeout(model: impl Into<String>, timeout: Duration) -> Self {
        Self {
            model: model.into(),
            timeout,
            #[cfg(target_os = "macos")]
            runtime: Arc::new(MacRuntime::default()),
        }
    }

    pub fn from_config(config: &CopilotConfig) -> Self {
        Self::with_timeout(
            APPLE_FM_COPILOT_MODEL,
            Duration::from_millis(config.target_latency_ms.max(250)),
        )
    }

    #[cfg(not(target_os = "macos"))]
    fn unavailable_error() -> ModelError {
        ModelError::new(
            ModelErrorKind::Unavailable,
            "Apple Foundation Models requires macOS 26 or newer with Apple Intelligence enabled",
        )
    }

    #[cfg(target_os = "macos")]
    fn checked_availability() -> Result<crate::apple_fm::AppleFmAvailability, ModelError> {
        let availability = crate::apple_fm::availability();
        if availability.available {
            Ok(availability)
        } else {
            Err(ModelError::new(
                ModelErrorKind::Unavailable,
                availability.detail,
            ))
        }
    }
}

impl CopilotModel for AppleFoundationCopilotModel {
    fn provider_name(&self) -> &str {
        "apple-fm"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn prewarm(&self) -> Result<(), ModelError> {
        #[cfg(target_os = "macos")]
        {
            Self::checked_availability()?;
            let result = self.runtime.prewarm(CopilotRequest::system_prompt());
            self.runtime.record_result(&result);
            result
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(Self::unavailable_error())
        }
    }

    fn stream_structured(
        &self,
        request: &CopilotRequest,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        if cancel.is_cancelled() {
            return Err(ModelError::cancelled());
        }

        #[cfg(target_os = "macos")]
        {
            Self::checked_availability()?;
            let result = self.runtime.stream_structured(
                CopilotRequest::system_prompt(),
                &request.untrusted_payload(),
                self.timeout,
                cancel,
                sink,
            );
            self.runtime.record_result(&result);
            result
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (request, sink);
            Err(Self::unavailable_error())
        }
    }

    fn health(&self) -> ModelHealth {
        #[cfg(target_os = "macos")]
        {
            let availability = crate::apple_fm::availability();
            let (status, detail) = if !availability.available {
                (ModelHealthStatus::Unavailable, availability.detail)
            } else if let Some(error) = self.runtime.last_error() {
                (
                    ModelHealthStatus::Degraded,
                    format!("Apple Foundation Models helper error: {error}"),
                )
            } else {
                let warm_state = if self.runtime.is_prewarmed() {
                    "session prewarmed"
                } else {
                    "session not yet prewarmed"
                };
                let gate = availability
                    .replay_gate_key
                    .unwrap_or_else(|| replay_gate_key(availability.os_version.as_deref()));
                (
                    ModelHealthStatus::Available,
                    format!("{}; {warm_state}; replay gate {gate}", availability.detail),
                )
            };
            ModelHealth {
                provider: self.provider_name().into(),
                model: self.model_name().into(),
                status,
                detail,
                checked_ts: Utc::now(),
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            ModelHealth {
                provider: self.provider_name().into(),
                model: self.model_name().into(),
                status: ModelHealthStatus::Unavailable,
                detail: Self::unavailable_error().message,
                checked_ts: Utc::now(),
            }
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperEvent {
    kind: String,
    schema_version: u32,
    id: Option<String>,
    snapshot: Option<HelperSnapshot>,
    error: Option<String>,
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HelperSnapshot {
    kind: Option<NudgeKind>,
    text: Option<String>,
    source_chip: Option<String>,
}

#[cfg(any(test, target_os = "macos"))]
impl HelperSnapshot {
    fn complete(self) -> Result<NudgeDraft, ModelError> {
        let kind = self.kind.ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::InvalidResponse,
                "Apple Foundation Models completed without a nudge kind",
            )
        })?;
        let text = required_snapshot_text(self.text, "text")?;
        let source_chip = required_snapshot_text(self.source_chip, "source_chip")?;
        Ok(NudgeDraft {
            kind,
            text,
            source_chip,
        })
    }

    fn complete_if_ready(self) -> Option<NudgeDraft> {
        Some(NudgeDraft {
            kind: self.kind?,
            text: nonempty(self.text?)?,
            source_chip: nonempty(self.source_chip?)?,
        })
    }
}

#[cfg(any(test, target_os = "macos"))]
fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

#[cfg(any(test, target_os = "macos"))]
fn required_snapshot_text(value: Option<String>, field: &str) -> Result<String, ModelError> {
    value.and_then(nonempty).ok_or_else(|| {
        ModelError::new(
            ModelErrorKind::InvalidResponse,
            format!("Apple Foundation Models completed without nudge {field}"),
        )
    })
}

#[cfg(any(test, target_os = "macos"))]
#[derive(Debug)]
struct ClientError {
    error: ModelError,
    restart_helper: bool,
}

#[cfg(any(test, target_os = "macos"))]
impl ClientError {
    fn new(error: ModelError, restart_helper: bool) -> Self {
        Self {
            error,
            restart_helper,
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
fn parse_helper_event(line: Result<String, String>) -> Result<HelperEvent, ClientError> {
    let line = line.map_err(|error| {
        ClientError::new(
            ModelError::new(
                ModelErrorKind::Unavailable,
                format!("Apple Foundation Models helper stream failed: {error}"),
            ),
            true,
        )
    })?;
    let event: HelperEvent = serde_json::from_str(&line).map_err(|error| {
        ClientError::new(
            ModelError::new(
                ModelErrorKind::InvalidResponse,
                format!("Apple Foundation Models helper returned invalid NDJSON: {error}"),
            ),
            true,
        )
    })?;
    if event.schema_version != HELPER_PROTOCOL_VERSION {
        return Err(ClientError::new(
            ModelError::new(
                ModelErrorKind::InvalidResponse,
                format!(
                    "Apple Foundation Models helper protocol {} is unsupported (expected {})",
                    event.schema_version, HELPER_PROTOCOL_VERSION
                ),
            ),
            true,
        ));
    }
    Ok(event)
}

#[cfg(any(test, target_os = "macos"))]
fn helper_error(message: Option<String>) -> ModelError {
    let message = message.unwrap_or_else(|| "Apple Foundation Models helper failed".into());
    let lowercase = message.to_ascii_lowercase();
    let kind = if lowercase.contains("unavailable")
        || lowercase.contains("requires macos")
        || lowercase.contains("apple intelligence")
    {
        ModelErrorKind::Unavailable
    } else {
        ModelErrorKind::InvalidResponse
    };
    ModelError::new(kind, message)
}

#[cfg(any(test, target_os = "macos"))]
fn consume_stream(
    events: &Receiver<Result<String, String>>,
    request_id: &str,
    cancel: &CancelToken,
    timeout: Duration,
    sink: &dyn ModelEventSink,
    mut send_cancel: impl FnMut() -> Result<(), ModelError>,
) -> Result<NudgeDraft, ClientError> {
    #[derive(Clone, Copy)]
    enum AbortReason {
        Cancelled,
        Timeout,
    }

    let started = Instant::now();
    let mut abort: Option<(AbortReason, Instant)> = None;
    let mut last_emitted: Option<NudgeDraft> = None;

    loop {
        if abort.is_none() {
            let reason = if cancel.is_cancelled() {
                Some(AbortReason::Cancelled)
            } else if started.elapsed() >= timeout {
                Some(AbortReason::Timeout)
            } else {
                None
            };
            if let Some(reason) = reason {
                send_cancel().map_err(|error| ClientError::new(error, true))?;
                abort = Some((reason, Instant::now()));
            }
        }

        if let Some((reason, sent_at)) = abort {
            if sent_at.elapsed() >= CANCEL_GRACE {
                let error = match reason {
                    AbortReason::Cancelled => ModelError::cancelled(),
                    AbortReason::Timeout => ModelError::timeout(format!(
                        "Apple Foundation Models fast lane exceeded {} ms",
                        timeout.as_millis()
                    )),
                };
                return Err(ClientError::new(error, true));
            }
        }

        let line = match events.recv_timeout(EVENT_POLL_INTERVAL) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(ClientError::new(
                    ModelError::new(
                        ModelErrorKind::Unavailable,
                        "Apple Foundation Models helper closed its output stream",
                    ),
                    true,
                ));
            }
        };
        let event = parse_helper_event(line)?;
        if event.id.as_deref() != Some(request_id) {
            continue;
        }

        match event.kind.as_str() {
            "snapshot" if abort.is_none() => {
                if let Some(draft) = event.snapshot.and_then(HelperSnapshot::complete_if_ready) {
                    if last_emitted.as_ref() != Some(&draft) {
                        sink.on_event(ModelStreamEvent::Structured(draft.clone()));
                        last_emitted = Some(draft);
                    }
                }
            }
            "completed" => {
                let aborted = abort
                    .map(|(reason, _)| reason)
                    .or_else(|| cancel.is_cancelled().then_some(AbortReason::Cancelled));
                if let Some(reason) = aborted {
                    let error = match reason {
                        AbortReason::Cancelled => ModelError::cancelled(),
                        AbortReason::Timeout => ModelError::timeout(format!(
                            "Apple Foundation Models fast lane exceeded {} ms",
                            timeout.as_millis()
                        )),
                    };
                    return Err(ClientError::new(error, false));
                }
                let draft = event
                    .snapshot
                    .ok_or_else(|| {
                        ClientError::new(
                            ModelError::new(
                                ModelErrorKind::InvalidResponse,
                                "Apple Foundation Models completed without a snapshot",
                            ),
                            false,
                        )
                    })?
                    .complete()
                    .map_err(|error| ClientError::new(error, false))?;
                if last_emitted.as_ref() != Some(&draft) {
                    sink.on_event(ModelStreamEvent::Structured(draft.clone()));
                }
                return Ok(draft);
            }
            "cancelled" => {
                let error = match abort.map(|(reason, _)| reason) {
                    Some(AbortReason::Timeout) => ModelError::timeout(format!(
                        "Apple Foundation Models fast lane exceeded {} ms",
                        timeout.as_millis()
                    )),
                    Some(AbortReason::Cancelled) | None => ModelError::cancelled(),
                };
                return Err(ClientError::new(error, false));
            }
            "error" => {
                if let Some((reason, _)) = abort {
                    let error = match reason {
                        AbortReason::Cancelled => ModelError::cancelled(),
                        AbortReason::Timeout => ModelError::timeout(format!(
                            "Apple Foundation Models fast lane exceeded {} ms",
                            timeout.as_millis()
                        )),
                    };
                    return Err(ClientError::new(error, false));
                }
                // A generation error may leave the reusable session at its
                // context limit or with a rejected transcript entry. Restart
                // before the next request so the fast lane can recover with a
                // fresh prewarmed session instead of repeatedly failing.
                return Err(ClientError::new(helper_error(event.error), true));
            }
            _ => {}
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HelperCommand<'a> {
    kind: &'a str,
    schema_version: u32,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_prompt: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt: Option<&'a str>,
}

#[cfg(target_os = "macos")]
struct RunningHelper {
    child: Child,
    stdin: ChildStdin,
    events: Receiver<Result<String, String>>,
    reader: Option<JoinHandle<()>>,
    prewarmed: bool,
}

#[cfg(target_os = "macos")]
impl RunningHelper {
    fn spawn() -> Result<Self, ModelError> {
        let path = crate::apple_fm::copilot_helper_path().ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                "Apple Foundation Models helper is unavailable",
            )
        })?;
        let mut child = Command::new(path)
            .arg("copilot-server")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                ModelError::new(
                    ModelErrorKind::Unavailable,
                    format!("failed to start Apple Foundation Models helper: {error}"),
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                "Apple Foundation Models helper stdin was not connected",
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                "Apple Foundation Models helper stdout was not connected",
            )
        })?;
        let (event_tx, events) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.map_err(|error| error.to_string());
                let failed = line.is_err();
                if event_tx.send(line).is_err() || failed {
                    return;
                }
            }
            let _ = event_tx.send(Err("helper reached end of file".into()));
        });
        Ok(Self {
            child,
            stdin,
            events,
            reader: Some(reader),
            prewarmed: false,
        })
    }

    fn write_command(&mut self, command: &HelperCommand<'_>) -> Result<(), ModelError> {
        serde_json::to_writer(&mut self.stdin, command).map_err(|error| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                format!("failed to encode Apple Foundation Models command: {error}"),
            )
        })?;
        self.stdin.write_all(b"\n").map_err(|error| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                format!("failed to write Apple Foundation Models command: {error}"),
            )
        })?;
        self.stdin.flush().map_err(|error| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                format!("failed to flush Apple Foundation Models command: {error}"),
            )
        })
    }

    fn is_running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }
}

#[cfg(target_os = "macos")]
impl Drop for RunningHelper {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct MacRuntime {
    helper: Mutex<Option<RunningHelper>>,
    next_request_id: AtomicU64,
    prewarmed: AtomicBool,
    last_error: Mutex<Option<String>>,
}

#[cfg(target_os = "macos")]
impl MacRuntime {
    fn next_id(&self) -> String {
        let sequence = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        format!("{}-{sequence}", std::process::id())
    }

    fn lock_helper(&self) -> std::sync::MutexGuard<'_, Option<RunningHelper>> {
        self.helper
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn ensure_helper<'a>(
        &self,
        helper: &'a mut Option<RunningHelper>,
    ) -> Result<&'a mut RunningHelper, ModelError> {
        if helper.as_mut().is_some_and(|running| !running.is_running()) {
            *helper = None;
            self.prewarmed.store(false, Ordering::Release);
        }
        if helper.is_none() {
            *helper = Some(RunningHelper::spawn()?);
        }
        helper.as_mut().ok_or_else(|| {
            ModelError::new(
                ModelErrorKind::Unavailable,
                "Apple Foundation Models helper failed to start",
            )
        })
    }

    fn prewarm(&self, system_prompt: &str) -> Result<(), ModelError> {
        let mut helper_slot = self.lock_helper();
        let result = self.prewarm_locked(&mut helper_slot, system_prompt);
        if result.is_err() {
            *helper_slot = None;
            self.prewarmed.store(false, Ordering::Release);
        }
        result
    }

    fn prewarm_locked(
        &self,
        helper_slot: &mut Option<RunningHelper>,
        system_prompt: &str,
    ) -> Result<(), ModelError> {
        let request_id = self.next_id();
        let helper = self.ensure_helper(helper_slot)?;
        if helper.prewarmed {
            self.prewarmed.store(true, Ordering::Release);
            return Ok(());
        }
        helper.write_command(&HelperCommand {
            kind: "prewarm",
            schema_version: HELPER_PROTOCOL_VERSION,
            id: &request_id,
            system_prompt: Some(system_prompt),
            prompt: None,
        })?;
        let deadline = Instant::now() + PREWARM_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                *helper_slot = None;
                self.prewarmed.store(false, Ordering::Release);
                return Err(ModelError::timeout(
                    "Apple Foundation Models session prewarm timed out",
                ));
            }
            let line = match helper
                .events
                .recv_timeout(remaining.min(EVENT_POLL_INTERVAL))
            {
                Ok(line) => line,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    *helper_slot = None;
                    self.prewarmed.store(false, Ordering::Release);
                    return Err(ModelError::new(
                        ModelErrorKind::Unavailable,
                        "Apple Foundation Models helper closed during prewarm",
                    ));
                }
            };
            let event = match parse_helper_event(line) {
                Ok(event) => event,
                Err(error) => {
                    if error.restart_helper {
                        *helper_slot = None;
                    }
                    self.prewarmed.store(false, Ordering::Release);
                    return Err(error.error);
                }
            };
            if event.id.as_deref() != Some(request_id.as_str()) {
                continue;
            }
            match event.kind.as_str() {
                "prewarmed" => {
                    helper.prewarmed = true;
                    self.prewarmed.store(true, Ordering::Release);
                    return Ok(());
                }
                "error" => {
                    self.prewarmed.store(false, Ordering::Release);
                    return Err(helper_error(event.error));
                }
                _ => {}
            }
        }
    }

    fn stream_structured(
        &self,
        system_prompt: &str,
        prompt: &str,
        timeout: Duration,
        cancel: &CancelToken,
        sink: &dyn ModelEventSink,
    ) -> Result<NudgeDraft, ModelError> {
        let mut helper_slot = self.lock_helper();
        if let Err(error) = self.prewarm_locked(&mut helper_slot, system_prompt) {
            *helper_slot = None;
            self.prewarmed.store(false, Ordering::Release);
            return Err(error);
        }
        let request_id = self.next_id();
        let helper = self.ensure_helper(&mut helper_slot)?;
        if let Err(error) = helper.write_command(&HelperCommand {
            kind: "generate",
            schema_version: HELPER_PROTOCOL_VERSION,
            id: &request_id,
            system_prompt: None,
            prompt: Some(prompt),
        }) {
            *helper_slot = None;
            self.prewarmed.store(false, Ordering::Release);
            return Err(error);
        }

        let events = &helper.events;
        let stdin = &mut helper.stdin;
        let result = consume_stream(events, &request_id, cancel, timeout, sink, || {
            let command = HelperCommand {
                kind: "cancel",
                schema_version: HELPER_PROTOCOL_VERSION,
                id: &request_id,
                system_prompt: None,
                prompt: None,
            };
            serde_json::to_writer(&mut *stdin, &command).map_err(|error| {
                ModelError::new(
                    ModelErrorKind::Unavailable,
                    format!("failed to encode Apple Foundation Models cancellation: {error}"),
                )
            })?;
            stdin.write_all(b"\n").map_err(|error| {
                ModelError::new(
                    ModelErrorKind::Unavailable,
                    format!("failed to send Apple Foundation Models cancellation: {error}"),
                )
            })?;
            stdin.flush().map_err(|error| {
                ModelError::new(
                    ModelErrorKind::Unavailable,
                    format!("failed to flush Apple Foundation Models cancellation: {error}"),
                )
            })
        });

        match result {
            Ok(draft) => Ok(draft),
            Err(error) => {
                if error.restart_helper {
                    *helper_slot = None;
                    self.prewarmed.store(false, Ordering::Release);
                }
                Err(error.error)
            }
        }
    }

    fn record_result<T>(&self, result: &Result<T, ModelError>) {
        let mut last_error = self
            .last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match result {
            Ok(_) => *last_error = None,
            Err(error) if error.kind == ModelErrorKind::Cancelled => {}
            Err(error) => *last_error = Some(error.message.clone()),
        }
    }

    fn last_error(&self) -> Option<String> {
        self.last_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn is_prewarmed(&self) -> bool {
        self.prewarmed.load(Ordering::Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};

    #[test]
    fn replay_gate_changes_with_os_and_prompt_contract() {
        let key = replay_gate_key(Some("26.1.0"));
        assert!(key.contains("prompt-v1"));
        assert!(key.contains("protocol-v1"));
        assert!(key.ends_with("macos-26.1.0"));
    }

    #[test]
    fn structured_snapshots_stream_and_complete() {
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(
            r#"{"kind":"snapshot","schemaVersion":1,"id":"r1","snapshot":{"kind":"Ask","text":"Ask who owns launch.","sourceChip":null},"error":null}"#
                .into(),
        ))
        .unwrap();
        tx.send(Ok(
            r#"{"kind":"snapshot","schemaVersion":1,"id":"r1","snapshot":{"kind":"Ask","text":"Ask who owns launch.","sourceChip":"launch owner"},"error":null}"#
                .into(),
        ))
        .unwrap();
        tx.send(Ok(
            r#"{"kind":"completed","schemaVersion":1,"id":"r1","snapshot":{"kind":"Ask","text":"Ask who owns launch.","sourceChip":"launch owner"},"error":null}"#
                .into(),
        ))
        .unwrap();
        let streamed = std::sync::Mutex::new(Vec::new());
        let sink = |event| streamed.lock().unwrap().push(event);
        let draft = consume_stream(
            &rx,
            "r1",
            &CancelToken::new(),
            Duration::from_secs(1),
            &sink,
            || Ok(()),
        )
        .unwrap();

        assert_eq!(draft.kind, NudgeKind::Ask);
        assert_eq!(draft.source_chip, "launch owner");
        assert_eq!(
            streamed.into_inner().unwrap(),
            vec![ModelStreamEvent::Structured(draft)]
        );
    }

    #[test]
    fn cancellation_sends_helper_command_and_waits_for_ack() {
        let (tx, rx) = mpsc::channel();
        let cancel = CancelToken::new();
        let cancel_clone = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            cancel_clone.cancel();
        });
        let cancel_sent = Arc::new(AtomicBool::new(false));
        let sent_for_command = cancel_sent.clone();
        let result = consume_stream(
            &rx,
            "r2",
            &cancel,
            Duration::from_secs(1),
            &|_| {},
            move || {
                sent_for_command.store(true, Ordering::Release);
                tx.send(Ok(
                    r#"{"kind":"cancelled","schemaVersion":1,"id":"r2","snapshot":null,"error":null}"#
                        .into(),
                ))
                .map_err(|error| {
                    ModelError::new(ModelErrorKind::Unavailable, error.to_string())
                })
            },
        )
        .unwrap_err();

        assert_eq!(result.error.kind, ModelErrorKind::Cancelled);
        assert!(!result.restart_helper);
        assert!(cancel_sent.load(Ordering::Acquire));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_provider_reports_unavailable_without_panicking() {
        let model = AppleFoundationCopilotModel::new(APPLE_FM_COPILOT_MODEL);
        let health = model.health();
        assert_eq!(health.status, ModelHealthStatus::Unavailable);
        assert!(health.detail.contains("requires macOS 26"));
        assert_eq!(
            model.prewarm().unwrap_err().kind,
            ModelErrorKind::Unavailable
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn pre_cancelled_linux_request_reports_cancellation_first() {
        let model = AppleFoundationCopilotModel::new(APPLE_FM_COPILOT_MODEL);
        let cancel = CancelToken::new();
        cancel.cancel();
        let request = CopilotRequest {
            goal: "close next steps".into(),
            evidence_revision: 1,
            update_kind: super::super::TranscriptUpdateKind::Final,
            utterances: Vec::new(),
            battle_card: super::super::BattleCard::empty(),
        };
        let error = model
            .stream_structured(&request, &cancel, &|_| {})
            .unwrap_err();
        assert_eq!(error.kind, ModelErrorKind::Cancelled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_health_matches_runtime_capability() {
        let model = AppleFoundationCopilotModel::new(APPLE_FM_COPILOT_MODEL);
        let availability = crate::apple_fm::availability();
        let expected = if availability.available {
            ModelHealthStatus::Available
        } else {
            ModelHealthStatus::Unavailable
        };
        assert_eq!(model.health().status, expected);
    }
}
