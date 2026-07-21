//! Codex app-server adapter for the provider-neutral Sidekick contract.
//!
//! This module deliberately lives in the desktop host. `minutes-core` owns
//! the session reducer, evidence window, prompt, intervention policy, and
//! publish decision; this file only translates the generic persistent-turn
//! protocol to Codex JSONL.

use minutes_core::live_sidekick::{
    PersistentReasoningBackend, PersistentReasoningSession, ReasoningBackendDescriptor,
    ReasoningError, ReasoningErrorKind, ReasoningEventSink, ReasoningLatencyClass,
    ReasoningOutputContract, ReasoningPrivacyClass, ReasoningSessionConfig, ReasoningSessionId,
    ReasoningStreamEvent, ReasoningTurnId, ReasoningTurnRequest, ReasoningTurnResult,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_TAIL_BYTES: usize = 8_000;

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone)]
pub struct CodexReasoningBackend {
    executable: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    _isolated_dir: Option<Arc<tempfile::TempDir>>,
}

impl CodexReasoningBackend {
    #[cfg(test)]
    fn new(executable: PathBuf, args: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            executable,
            args,
            cwd,
            _isolated_dir: None,
        }
    }

    /// Build the production adapter with every model-callable tool lane off.
    /// The app-server keeps access only to Codex authentication and the
    /// bounded text/image input sent by Minutes.
    pub fn sidekick(
        executable: PathBuf,
        configured_mcp_servers: impl IntoIterator<Item = String>,
    ) -> Result<Self, ReasoningError> {
        let isolated_dir = Arc::new(tempfile::tempdir().map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!("Could not create isolated Sidekick workspace: {error}"),
                true,
            )
        })?);
        let mut args = vec![
            "--disable".into(),
            "apps".into(),
            "--disable".into(),
            "plugins".into(),
            "--disable".into(),
            "shell_tool".into(),
            "--disable".into(),
            "browser_use".into(),
            "--disable".into(),
            "computer_use".into(),
            "--disable".into(),
            "image_generation".into(),
            "--disable".into(),
            "multi_agent".into(),
            "--config".into(),
            "allow_login_shell=false".into(),
            "--config".into(),
            "shell_environment_policy.inherit=\"none\"".into(),
            "--config".into(),
            "mcp_servers={}".into(),
        ];
        let mut servers = configured_mcp_servers.into_iter().collect::<Vec<_>>();
        servers.sort();
        servers.dedup();
        for server in servers {
            if server
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "_-".contains(character))
            {
                args.extend([
                    "--config".into(),
                    format!("mcp_servers.{server}.enabled=false"),
                ]);
            }
        }
        args.extend(["--enable".into(), "fast_mode".into(), "app-server".into()]);
        Ok(Self {
            executable,
            args,
            cwd: isolated_dir.path().to_path_buf(),
            _isolated_dir: Some(isolated_dir),
        })
    }
}

impl PersistentReasoningBackend for CodexReasoningBackend {
    fn descriptor(&self) -> ReasoningBackendDescriptor {
        ReasoningBackendDescriptor {
            provider: "codex-app-server".into(),
            model: "codex-fast".into(),
            privacy: ReasoningPrivacyClass::Cloud,
            persistent: true,
            steerable: true,
            streaming: true,
            image_input: true,
        }
    }

    fn start_session(
        &self,
        config: ReasoningSessionConfig,
    ) -> Result<Box<dyn PersistentReasoningSession>, ReasoningError> {
        config.validate()?;
        let mut command = Command::new(&self.executable);
        command.args(&self.args).current_dir(&self.cwd).env_clear();
        for key in [
            "HOME",
            "PATH",
            "USER",
            "TMPDIR",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "SSL_CERT_FILE",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    format!("Could not start Codex: {error}"),
                    true,
                )
            })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            ReasoningError::new(
                ReasoningErrorKind::Protocol,
                "Codex app-server did not expose stdin",
                false,
            )
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            ReasoningError::new(
                ReasoningErrorKind::Protocol,
                "Codex app-server did not expose stdout",
                false,
            )
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            ReasoningError::new(
                ReasoningErrorKind::Protocol,
                "Codex app-server did not expose stderr",
                false,
            )
        })?;

        let state = Arc::new(Mutex::new(ProtocolState::default()));
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let reader = spawn_protocol_reader(stdout, Arc::clone(&state), Arc::clone(&stderr_tail));
        let stderr_reader = spawn_stderr_reader(stderr, Arc::clone(&stderr_tail));
        let mut session = CodexReasoningSession {
            id: ReasoningSessionId::new("pending-codex-thread"),
            config,
            cwd: self.cwd.clone(),
            stdin: Arc::new(Mutex::new(stdin)),
            child: Some(child),
            state,
            _stderr_tail: stderr_tail,
            reader: Some(reader),
            stderr_reader: Some(stderr_reader),
            next_request_id: 1,
            closed: false,
        };
        session.initialize()?;
        Ok(Box::new(session))
    }
}

enum PendingKind {
    Ordinary,
    TurnStart {
        sink: Arc<dyn ReasoningEventSink>,
        started_at: Instant,
        invocation: minutes_core::live_sidekick::InvocationIdentity,
    },
}

struct PendingRequest {
    method: String,
    kind: PendingKind,
    sender: SyncSender<Result<Value, ReasoningError>>,
}

struct ActiveTurn {
    id: ReasoningTurnId,
    invocation: minutes_core::live_sidekick::InvocationIdentity,
    sink: Arc<dyn ReasoningEventSink>,
    started_at: Instant,
    first_token_at: Option<Instant>,
    text: String,
}

#[derive(Default)]
struct ProtocolState {
    pending: HashMap<u64, PendingRequest>,
    turns: HashMap<String, ActiveTurn>,
}

pub struct CodexReasoningSession {
    id: ReasoningSessionId,
    config: ReasoningSessionConfig,
    cwd: PathBuf,
    stdin: Arc<Mutex<ChildStdin>>,
    child: Option<Child>,
    state: Arc<Mutex<ProtocolState>>,
    _stderr_tail: Arc<Mutex<String>>,
    reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<()>>,
    next_request_id: u64,
    closed: bool,
}

impl CodexReasoningSession {
    fn initialize(&mut self) -> Result<(), ReasoningError> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "minutes_sidekick",
                    "title": "Minutes Sidekick",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": { "experimentalApi": true }
            }),
            PendingKind::Ordinary,
        )?;
        self.notify("initialized", json!({}))?;
        let service_tier = match self.config.latency_class {
            ReasoningLatencyClass::Realtime => "fast",
            ReasoningLatencyClass::Deliberate => "flex",
        };
        let result = self.request(
            "thread/start",
            json!({
                "cwd": self.cwd,
                "approvalPolicy": "never",
                "sandbox": "read-only",
                "serviceTier": service_tier,
                "ephemeral": self.config.ephemeral,
                "baseInstructions": self.config.base_instructions,
                "developerInstructions": self.config.developer_instructions,
                "environments": []
            }),
            PendingKind::Ordinary,
        )?;
        let thread_id = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "Codex thread/start response did not include thread.id",
                    false,
                )
            })?;
        self.id = ReasoningSessionId::new(thread_id);
        Ok(())
    }

    fn request(
        &mut self,
        method: &str,
        params: Value,
        kind: PendingKind,
    ) -> Result<Value, ReasoningError> {
        if self.closed {
            return Err(ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "Codex reasoning session is closed",
                false,
            ));
        }
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.checked_add(1).ok_or_else(|| {
            ReasoningError::new(
                ReasoningErrorKind::Protocol,
                "Codex request id exhausted",
                false,
            )
        })?;
        let (sender, receiver) = mpsc::sync_channel(1);
        lock_unpoisoned(&self.state).pending.insert(
            id,
            PendingRequest {
                method: method.into(),
                kind,
                sender,
            },
        );
        if let Err(error) = self.write_message(&json!({
            "method": method,
            "id": id,
            "params": params,
        })) {
            lock_unpoisoned(&self.state).pending.remove(&id);
            return Err(error);
        }
        match receiver.recv_timeout(REQUEST_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                lock_unpoisoned(&self.state).pending.remove(&id);
                let error = ReasoningError::new(
                    ReasoningErrorKind::Timeout,
                    format!("Codex {method} timed out"),
                    true,
                );
                self.close();
                Err(error)
            }
        }
    }

    fn notify(&self, method: &str, params: Value) -> Result<(), ReasoningError> {
        self.write_message(&json!({ "method": method, "params": params }))
    }

    fn write_message(&self, message: &Value) -> Result<(), ReasoningError> {
        let serialized = serde_json::to_vec(message).map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Protocol,
                format!("Could not serialize Codex request: {error}"),
                false,
            )
        })?;
        let mut stdin = lock_unpoisoned(&self.stdin);
        stdin
            .write_all(&serialized)
            .and_then(|_| stdin.write_all(b"\n"))
            .and_then(|_| stdin.flush())
            .map_err(|error| {
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    format!("Could not write to Codex app-server: {error}"),
                    true,
                )
            })
    }

    fn input_for(request: &ReasoningTurnRequest) -> Vec<Value> {
        let mut input = vec![json!({
            "type": "text",
            "text": request.render_prompt(),
            "text_elements": []
        })];
        if let Some(image) = &request.window.latest_image {
            input.push(json!({
                "type": "localImage",
                "path": image.path,
                "detail": "high"
            }));
        }
        input
    }

    fn output_schema_for(contract: ReasoningOutputContract) -> Value {
        match contract {
            ReasoningOutputContract::InterventionCandidateV1 => json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "decision": { "type": "string", "enum": ["silent", "speak"] },
                    "kind": {
                        "type": ["string", "null"],
                        "enum": ["insight", "question", "risk", "opening", "answer", "strategy", null]
                    },
                    "text": { "type": ["string", "null"] },
                    "evidence_ids": { "type": "array", "items": { "type": "string" } },
                    "visual_evidence_ids": { "type": "array", "items": { "type": "string" } },
                    "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
                },
                "required": [
                    "decision", "kind", "text", "evidence_ids", "visual_evidence_ids", "confidence"
                ]
            }),
        }
    }
}

impl PersistentReasoningSession for CodexReasoningSession {
    fn id(&self) -> &ReasoningSessionId {
        &self.id
    }

    fn start_turn(
        &mut self,
        request: ReasoningTurnRequest,
        sink: Arc<dyn ReasoningEventSink>,
    ) -> Result<ReasoningTurnId, ReasoningError> {
        self.config.validate_request(&request)?;
        let service_tier = match self.config.latency_class {
            ReasoningLatencyClass::Realtime => "fast",
            ReasoningLatencyClass::Deliberate => "flex",
        };
        let result = self.request(
            "turn/start",
            json!({
                "threadId": self.id.as_str(),
                "input": Self::input_for(&request),
                "outputSchema": Self::output_schema_for(request.output_contract),
                "serviceTier": service_tier,
                "effort": "low",
                "environments": []
            }),
            PendingKind::TurnStart {
                sink,
                started_at: Instant::now(),
                invocation: request.invocation,
            },
        )?;
        let turn_id = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ReasoningError::new(
                    ReasoningErrorKind::Protocol,
                    "Codex turn/start response did not include turn.id",
                    false,
                )
            })?;
        Ok(ReasoningTurnId::new(turn_id))
    }

    fn steer_turn(
        &mut self,
        turn_id: &ReasoningTurnId,
        request: ReasoningTurnRequest,
    ) -> Result<(), ReasoningError> {
        self.config.validate_request(&request)?;
        if !turn_id.is_valid() {
            return Err(ReasoningError::invalid_request("turn id is empty"));
        }
        self.request(
            "turn/steer",
            json!({
                "threadId": self.id.as_str(),
                "expectedTurnId": turn_id.as_str(),
                "input": Self::input_for(&request)
            }),
            PendingKind::Ordinary,
        )?;
        let mut state = lock_unpoisoned(&self.state);
        let Some(turn) = state.turns.get_mut(turn_id.as_str()) else {
            return Err(ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                "reasoning turn completed before steering was committed",
                true,
            ));
        };
        turn.invocation = request.invocation;
        Ok(())
    }

    fn interrupt_turn(&mut self, turn_id: &ReasoningTurnId) -> Result<(), ReasoningError> {
        if !turn_id.is_valid() {
            return Err(ReasoningError::invalid_request("turn id is empty"));
        }
        self.request(
            "turn/interrupt",
            json!({
                "threadId": self.id.as_str(),
                "turnId": turn_id.as_str()
            }),
            PendingKind::Ordinary,
        )?;
        Ok(())
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.child = None;
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

impl Drop for CodexReasoningSession {
    fn drop(&mut self) {
        self.close();
    }
}

fn spawn_protocol_reader(
    stdout: impl Read + Send + 'static,
    state: Arc<Mutex<ProtocolState>>,
    stderr_tail: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => match serde_json::from_str::<Value>(&line) {
                    Ok(message) => handle_message(message, &state),
                    Err(error) => {
                        fail_protocol(
                            &state,
                            ReasoningError::new(
                                ReasoningErrorKind::Protocol,
                                format!("Codex emitted invalid JSON: {error}"),
                                false,
                            ),
                        );
                        return;
                    }
                },
                Err(error) => {
                    fail_protocol(
                        &state,
                        ReasoningError::new(
                            ReasoningErrorKind::Unavailable,
                            format!("Could not read Codex app-server: {error}"),
                            true,
                        ),
                    );
                    return;
                }
            }
        }
        let detail = lock_unpoisoned(&stderr_tail).trim().to_string();
        let suffix = if detail.is_empty() {
            String::new()
        } else {
            format!(": {detail}")
        };
        fail_protocol(
            &state,
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!("Codex app-server exited{suffix}"),
                true,
            ),
        );
    })
}

fn spawn_stderr_reader(
    mut stderr: impl Read + Send + 'static,
    tail: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        let mut buffer = [0_u8; 1_024];
        loop {
            match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(read) => {
                    let chunk = String::from_utf8_lossy(&buffer[..read]);
                    let mut current = lock_unpoisoned(&tail);
                    current.push_str(&chunk);
                    if current.len() > STDERR_TAIL_BYTES {
                        let split = current.len() - STDERR_TAIL_BYTES;
                        *current = current.split_off(split);
                    }
                }
            }
        }
    })
}

fn handle_message(message: Value, state: &Mutex<ProtocolState>) {
    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let pending = lock_unpoisoned(state).pending.remove(&id);
        if let Some(pending) = pending {
            if let Some(error) = message.get("error") {
                let _ = pending
                    .sender
                    .send(Err(protocol_error(&pending.method, error)));
                return;
            }
            let result = message.get("result").cloned().unwrap_or(Value::Null);
            if let PendingKind::TurnStart {
                sink,
                started_at,
                invocation,
            } = pending.kind
            {
                if let Some(turn_id) = result.pointer("/turn/id").and_then(Value::as_str) {
                    lock_unpoisoned(state).turns.insert(
                        turn_id.into(),
                        ActiveTurn {
                            id: ReasoningTurnId::new(turn_id),
                            invocation,
                            sink,
                            started_at,
                            first_token_at: None,
                            text: String::new(),
                        },
                    );
                }
            }
            let _ = pending.sender.send(Ok(result));
        }
        return;
    }

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return;
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    let turn_id = params
        .get("turnId")
        .and_then(Value::as_str)
        .or_else(|| params.pointer("/turn/id").and_then(Value::as_str));
    let Some(turn_id) = turn_id else {
        return;
    };

    match method {
        "item/agentMessage/delta" => {
            let delta = params.get("delta").and_then(Value::as_str).unwrap_or("");
            let event = {
                let mut state = lock_unpoisoned(state);
                state.turns.get_mut(turn_id).map(|turn| {
                    turn.first_token_at.get_or_insert_with(Instant::now);
                    turn.text.push_str(delta);
                    ReasoningStreamEvent::TextDelta {
                        turn_id: turn.id.clone(),
                        invocation: turn.invocation,
                        text: delta.into(),
                    }
                })
            };
            if let Some(event) = event {
                if let Some(sink) = lock_unpoisoned(state)
                    .turns
                    .get(turn_id)
                    .map(|turn| Arc::clone(&turn.sink))
                {
                    sink.on_event(event);
                }
            }
        }
        "item/completed" => {
            if params.pointer("/item/type").and_then(Value::as_str) == Some("agentMessage") {
                if let Some(text) = params.pointer("/item/text").and_then(Value::as_str) {
                    if let Some(turn) = lock_unpoisoned(state).turns.get_mut(turn_id) {
                        turn.text = text.into();
                    }
                }
            }
        }
        "turn/completed" => {
            if let Some(turn) = lock_unpoisoned(state).turns.remove(turn_id) {
                let now = Instant::now();
                let first_token_ms = turn.first_token_at.map(|first| {
                    first
                        .saturating_duration_since(turn.started_at)
                        .as_millis()
                        .min(u128::from(u64::MAX)) as u64
                });
                let total_ms = now
                    .saturating_duration_since(turn.started_at)
                    .as_millis()
                    .min(u128::from(u64::MAX)) as u64;
                let status = params
                    .pointer("/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                if status == "completed" {
                    turn.sink.on_event(ReasoningStreamEvent::Completed {
                        turn_id: turn.id,
                        invocation: turn.invocation,
                        result: ReasoningTurnResult {
                            text: turn.text,
                            first_token_ms,
                            total_ms,
                        },
                    });
                } else {
                    let kind = if matches!(status, "interrupted" | "cancelled") {
                        ReasoningErrorKind::Cancelled
                    } else {
                        ReasoningErrorKind::Protocol
                    };
                    let detail = params
                        .pointer("/turn/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or(status);
                    turn.sink.on_event(ReasoningStreamEvent::Failed {
                        turn_id: turn.id,
                        invocation: turn.invocation,
                        error: ReasoningError::new(
                            kind,
                            format!("Codex turn ended as {status}: {detail}"),
                            kind != ReasoningErrorKind::Cancelled,
                        ),
                    });
                }
            }
        }
        _ => {}
    }
}

fn protocol_error(method: &str, error: &Value) -> ReasoningError {
    let code = error.get("code").and_then(Value::as_i64);
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown app-server error");
    let (kind, retryable) = match code {
        Some(-32001) => (ReasoningErrorKind::Overloaded, true),
        _ if message.to_ascii_lowercase().contains("auth") => {
            (ReasoningErrorKind::Authentication, false)
        }
        _ => (ReasoningErrorKind::Protocol, false),
    };
    ReasoningError::new(kind, format!("Codex {method} failed: {message}"), retryable)
}

fn fail_protocol(state: &Mutex<ProtocolState>, error: ReasoningError) {
    let (pending, turns) = {
        let mut state = lock_unpoisoned(state);
        (
            state
                .pending
                .drain()
                .map(|(_, item)| item)
                .collect::<Vec<_>>(),
            state
                .turns
                .drain()
                .map(|(_, item)| item)
                .collect::<Vec<_>>(),
        )
    };
    for request in pending {
        let _ = request.sender.send(Err(error.clone()));
    }
    for turn in turns {
        turn.sink.on_event(ReasoningStreamEvent::Failed {
            turn_id: turn.id,
            invocation: turn.invocation,
            error: error.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minutes_core::live_sidekick::{
        BoundedReasoningWindow, InvocationIdentity, ReasoningTranscriptEvidence, ReasoningTurnKind,
    };
    use std::sync::mpsc::RecvTimeoutError;

    const FAKE_SERVER: &str = r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });
function send(value) { process.stdout.write(JSON.stringify(value) + '\n'); }
rl.on('line', (line) => {
  const msg = JSON.parse(line);
  if (msg.method === 'initialize') send({ id: msg.id, result: { userAgent: 'fake' } });
  else if (msg.method === 'thread/start') send({ id: msg.id, result: { thread: { id: 'thread-1' } } });
  else if (msg.method === 'turn/start') {
    send({ id: msg.id, result: { turn: { id: 'turn-1' } } });
    send({ method: 'item/agentMessage/delta', params: { turnId: 'turn-1', delta: '{"decision":' } });
    send({ method: 'item/agentMessage/delta', params: { turnId: 'turn-1', delta: '"silent"}' } });
    send({ method: 'turn/completed', params: { turn: { id: 'turn-1', status: 'completed' } } });
  } else if (msg.method === 'turn/steer') send({ id: msg.id, result: { turnId: msg.params.expectedTurnId } });
  else if (msg.method === 'turn/interrupt') send({ id: msg.id, result: {} });
});
"#;

    fn config() -> ReasoningSessionConfig {
        ReasoningSessionConfig {
            base_instructions: "base".into(),
            developer_instructions: "developer".into(),
            latency_class: ReasoningLatencyClass::Realtime,
            max_window_chars: 4_096,
            ephemeral: true,
            evidence_scope: minutes_core::live_sidekick::ReasoningEvidenceScope {
                capture_session_id: "capture-a".into(),
                source_policy_generation: 0,
            },
        }
    }

    fn request() -> ReasoningTurnRequest {
        ReasoningTurnRequest {
            kind: ReasoningTurnKind::Background,
            invocation: InvocationIdentity {
                sequence: 1,
                source_policy_generation: 0,
                user_generation: 0,
            },
            window: BoundedReasoningWindow {
                capture_session_id: "capture-a".into(),
                transcript: vec![ReasoningTranscriptEvidence {
                    evidence_id: "evidence-a".into(),
                    text: "routine evidence".into(),
                    speaker_label: None,
                    speaker_verified: false,
                    offset_ms: 0,
                    duration_ms: 100,
                }],
                latest_image: None,
                prepared_context: String::new(),
            },
            authoritative_memory: Vec::new(),
            typed_user_message: None,
            output_contract: ReasoningOutputContract::InterventionCandidateV1,
        }
    }

    #[test]
    fn app_server_adapter_streams_through_the_provider_neutral_contract() {
        let Ok(node) = which::which("node") else {
            return;
        };
        let backend = CodexReasoningBackend::new(
            node,
            vec!["-e".into(), FAKE_SERVER.into()],
            std::env::temp_dir(),
        );
        let mut session = backend
            .start_session(config())
            .expect("start fake Codex session");
        assert_eq!(session.id().as_str(), "thread-1");
        let (sender, receiver) = mpsc::sync_channel(8);
        let turn_id = session
            .start_turn(
                request(),
                Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
            )
            .expect("start turn");
        assert_eq!(turn_id.as_str(), "turn-1");
        let mut text = String::new();
        loop {
            match receiver.recv_timeout(Duration::from_secs(2)) {
                Ok(ReasoningStreamEvent::TextDelta {
                    invocation,
                    text: delta,
                    ..
                }) => {
                    assert_eq!(invocation.sequence, 1);
                    text.push_str(&delta);
                }
                Ok(ReasoningStreamEvent::Completed {
                    invocation, result, ..
                }) => {
                    assert_eq!(invocation.sequence, 1);
                    assert_eq!(result.text, text);
                    assert_eq!(result.text, "{\"decision\":\"silent\"}");
                    break;
                }
                Ok(ReasoningStreamEvent::Failed { error, .. }) => {
                    panic!("unexpected failure: {error}")
                }
                Err(RecvTimeoutError::Timeout) => panic!("timed out waiting for fake Codex"),
                Err(RecvTimeoutError::Disconnected) => panic!("fake Codex event channel closed"),
            }
        }
        session.close();
    }

    #[test]
    fn failed_terminal_status_never_becomes_a_completion() {
        let state = Mutex::new(ProtocolState::default());
        let (sender, receiver) = mpsc::sync_channel(2);
        lock_unpoisoned(&state).turns.insert(
            "turn-failed".into(),
            ActiveTurn {
                id: "turn-failed".into(),
                invocation: request().invocation,
                sink: Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
                started_at: Instant::now(),
                first_token_at: None,
                text: "partial text must not publish".into(),
            },
        );
        handle_message(
            json!({
                "method": "turn/completed",
                "params": {
                    "turn": {
                        "id": "turn-failed",
                        "status": "failed",
                        "error": { "message": "synthetic overload" }
                    }
                }
            }),
            &state,
        );
        match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            ReasoningStreamEvent::Failed { error, .. } => {
                assert_eq!(error.kind, ReasoningErrorKind::Protocol);
            }
            event => panic!("failed turn was misclassified: {event:?}"),
        }
    }

    #[test]
    fn production_factory_disables_tool_and_mcp_lanes() {
        let backend = CodexReasoningBackend::sidekick(
            PathBuf::from("/usr/bin/codex"),
            vec!["github".into(), "slack".into(), "github".into()],
        )
        .unwrap();
        let joined = backend.args.join(" ");
        for required in [
            "--disable shell_tool",
            "--disable apps",
            "--disable plugins",
            "--disable browser_use",
            "--disable computer_use",
            "mcp_servers.github.enabled=false",
            "mcp_servers.slack.enabled=false",
        ] {
            assert!(joined.contains(required), "missing {required}: {joined}");
        }
        assert!(backend.cwd.is_absolute());
    }
}
