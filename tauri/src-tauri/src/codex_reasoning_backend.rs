//! Codex app-server adapter for the provider-neutral Sidekick contract.
//!
//! This module deliberately lives in the desktop host. `minutes-core` owns
//! the session reducer, evidence window, prompt, intervention policy, and
//! publish decision; this file only translates the generic persistent-turn
//! protocol to Codex JSONL.

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use minutes_core::live_sidekick::{
    PersistentReasoningBackend, PersistentReasoningSession, ReasoningBackendDescriptor,
    ReasoningError, ReasoningErrorKind, ReasoningEventSink, ReasoningLatencyClass,
    ReasoningOutputContract, ReasoningPrivacyClass, ReasoningSessionConfig, ReasoningSessionId,
    ReasoningStreamEvent, ReasoningTurnId, ReasoningTurnKind, ReasoningTurnRequest,
    ReasoningTurnResult,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const STDERR_TAIL_BYTES: usize = 8_000;
// The Codex adapter owns this provider-specific mapping. Minutes core asks for
// the provider-neutral Realtime latency class and never names a vendor model.
fn codex_realtime_model() -> &'static str {
    include_str!("../../../resources/live_sidekick/codex_realtime_model.txt").trim()
}

fn codex_verifier_model() -> &'static str {
    include_str!("../../../resources/live_sidekick/codex_verifier_model.txt").trim()
}

fn codex_realtime_effort() -> &'static str {
    include_str!("../../../resources/live_sidekick/codex_realtime_effort.txt").trim()
}

fn codex_verifier_effort() -> &'static str {
    include_str!("../../../resources/live_sidekick/codex_verifier_effort.txt").trim()
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone)]
pub struct CodexReasoningBackend {
    executable: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    home: PathBuf,
    codex_home: PathBuf,
    model: String,
    effort: String,
    _isolated_dir: Option<Arc<tempfile::TempDir>>,
}

impl CodexReasoningBackend {
    #[cfg(test)]
    fn new(executable: PathBuf, args: Vec<String>, cwd: PathBuf) -> Self {
        let home = cwd.join("home");
        let codex_home = cwd.join("codex-home");
        Self {
            executable,
            args,
            cwd,
            home,
            codex_home,
            model: codex_realtime_model().into(),
            effort: codex_realtime_effort().into(),
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
        Self::sidekick_with_model_and_effort(
            executable,
            configured_mcp_servers,
            codex_realtime_model(),
            codex_realtime_effort(),
        )
    }

    pub fn sidekick_verifier(
        executable: PathBuf,
        configured_mcp_servers: impl IntoIterator<Item = String>,
    ) -> Result<Self, ReasoningError> {
        Self::sidekick_with_model_and_effort(
            executable,
            configured_mcp_servers,
            codex_verifier_model(),
            codex_verifier_effort(),
        )
    }

    fn sidekick_with_model_and_effort(
        executable: PathBuf,
        _configured_mcp_servers: impl IntoIterator<Item = String>,
        model: &str,
        effort: &str,
    ) -> Result<Self, ReasoningError> {
        let isolated_dir = Arc::new(tempfile::tempdir().map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!("Could not create isolated Sidekick workspace: {error}"),
                true,
            )
        })?);
        let home = isolated_dir.path().join("home");
        let codex_home = isolated_dir.path().join("codex-home");
        fs::create_dir_all(&home)
            .and_then(|_| fs::create_dir_all(&codex_home))
            .map_err(|error| {
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    format!("Could not create isolated Codex home: {error}"),
                    true,
                )
            })?;
        copy_codex_authentication(source_codex_home().as_deref(), &codex_home).map_err(
            |error| {
                ReasoningError::new(
                    ReasoningErrorKind::Unavailable,
                    format!("Could not copy Codex authentication into isolation: {error}"),
                    true,
                )
            },
        )?;
        let mut args = vec![
            "--strict-config".into(),
            "--disable".into(),
            "apps".into(),
            "--disable".into(),
            "plugins".into(),
            "--disable".into(),
            "hooks".into(),
            "--disable".into(),
            "auth_elicitation".into(),
            "--disable".into(),
            "tool_call_mcp_elicitation".into(),
            "--disable".into(),
            "workspace_dependencies".into(),
            "--disable".into(),
            "skill_mcp_dependency_install".into(),
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
            "--config".into(),
            "service_tier=\"fast\"".into(),
            "--config".into(),
            format!("model_reasoning_effort=\"{effort}\""),
        ];
        // CODEX_HOME contains authentication only, so there are no inherited
        // MCP definitions to disable. Do not synthesize partial per-server
        // tables here: strict config correctly rejects entries with no
        // transport before app-server can start.
        args.extend(["--enable".into(), "fast_mode".into(), "app-server".into()]);
        Ok(Self {
            executable,
            args,
            cwd: isolated_dir.path().to_path_buf(),
            home,
            codex_home,
            model: model.into(),
            effort: effort.into(),
            _isolated_dir: Some(isolated_dir),
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command
            .args(&self.args)
            .current_dir(&self.cwd)
            .env_clear()
            .env("HOME", &self.home)
            .env("CODEX_HOME", &self.codex_home);
        for key in [
            "PATH",
            "USER",
            "TMPDIR",
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "ALL_PROXY",
            "NO_PROXY",
            "SSL_CERT_FILE",
            "OPENAI_API_KEY",
        ] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command
    }
}

fn source_codex_home() -> Option<PathBuf> {
    std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".codex")))
}

fn copy_codex_authentication(
    source_home: Option<&Path>,
    target_home: &Path,
) -> std::io::Result<()> {
    let Some(source_auth) = source_home.map(|path| path.join("auth.json")) else {
        return Ok(());
    };
    if source_auth.is_file() {
        fs::copy(source_auth, target_home.join("auth.json"))?;
    }
    Ok(())
}

impl PersistentReasoningBackend for CodexReasoningBackend {
    fn descriptor(&self) -> ReasoningBackendDescriptor {
        ReasoningBackendDescriptor {
            provider: "codex-app-server".into(),
            model: self.model.clone(),
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
        let mut command = self.command();
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

        let stdin = Arc::new(Mutex::new(stdin));
        let state = Arc::new(Mutex::new(ProtocolState::default()));
        let stderr_tail = Arc::new(Mutex::new(String::new()));
        let reader = spawn_protocol_reader(
            stdout,
            Arc::clone(&stdin),
            Arc::clone(&state),
            Arc::clone(&stderr_tail),
        );
        let stderr_reader = spawn_stderr_reader(stderr, Arc::clone(&stderr_tail));
        let mut session = CodexReasoningSession {
            id: ReasoningSessionId::new("pending-codex-thread"),
            config,
            model: self.model.clone(),
            effort: self.effort.clone(),
            cwd: self.cwd.clone(),
            stdin,
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
    TurnSteer {
        turn_id: ReasoningTurnId,
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
    model: String,
    effort: String,
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
                "model": self.model,
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
        write_protocol_message(&self.stdin, message)
    }

    fn input_for(request: &ReasoningTurnRequest) -> Vec<Value> {
        let mut input = vec![json!({
            "type": "text",
            "text": request.render_prompt(),
            "text_elements": []
        })];
        if let Some(image) = &request.window.latest_image {
            input.push(json!({
                "type": "image",
                "url": format!(
                    "data:image/png;base64,{}",
                    BASE64_STANDARD.encode(&image.png_bytes)
                ),
                "detail": "high"
            }));
        }
        input
    }

    fn output_schema_for(contract: ReasoningOutputContract, kind: ReasoningTurnKind) -> Value {
        match contract {
            ReasoningOutputContract::InterventionCandidateV1 => {
                // A background turn can be promoted through turn/steer, which
                // cannot replace its output schema. Give every started turn
                // foreground-capable character headroom; Minutes core still
                // enforces the stricter 50-word background publication cap.
                let max_length = 700;
                let turn_name = match kind {
                    ReasoningTurnKind::Background => "steerable background",
                    ReasoningTurnKind::Foreground => "foreground",
                };
                let target_words = match kind {
                    ReasoningTurnKind::Background => 36,
                    ReasoningTurnKind::Foreground => 44,
                };
                json!({
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "decision": { "type": "string", "enum": ["silent", "speak"] },
                        "kind": {
                            "type": ["string", "null"],
                            "enum": ["insight", "question", "risk", "opening", "answer", "strategy", null]
                        },
                        "text": {
                            "type": ["string", "null"],
                            "maxLength": max_length,
                            "description": format!("Visible answer. For customer-side automation protections, explicitly name a written confidence-threshold SLA, auditable case-level error reporting with access to underlying records rather than aggregate-only dashboards, the customer's unilateral human-reversion right without vendor permission, and each monetary remedy as a directionally complete obligation: vendor owes the stated remedy to the customer for the exact evidenced failure class. Target at most {target_words} words and never exceed {max_length} characters for this {turn_name} turn.")
                        },
                        "evidence_ids": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "Exact transcript evidence IDs supporting every visible factual claim, number, remedy condition, and fallback control. Include all distinct items needed for a synthesis; do not cite an item merely because it is available. If recommending human reversion when the evidence contains a human-in-loop versus automation decision, cite that decision item as well as any contract-remedy item."
                        },
                        "visual_evidence_ids": { "type": "array", "items": { "type": "string" } },
                        "claims_visual_observation": {
                            "type": "boolean",
                            "description": "True iff the visible response relies on pixels from the supplied exact-session image; false otherwise."
                        },
                        "confidence": { "type": "integer", "minimum": 0, "maximum": 100 }
                    },
                    "required": [
                        "decision", "kind", "text", "evidence_ids", "visual_evidence_ids",
                        "claims_visual_observation", "confidence"
                    ]
                })
            }
            ReasoningOutputContract::EvidenceVerificationV1 => json!({
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "decision": { "type": "string", "enum": ["allow", "reject"] },
                    "reason_code": {
                        "type": "string",
                        "enum": ["supported", "unsupported_fact", "unsupported_visual", "contradiction", "uncertain"]
                    }
                },
                "required": ["decision", "reason_code"]
            }),
        }
    }
}

fn write_protocol_message<W: Write>(
    writer: &Mutex<W>,
    message: &Value,
) -> Result<(), ReasoningError> {
    let serialized = serde_json::to_vec(message).map_err(|error| {
        ReasoningError::new(
            ReasoningErrorKind::Protocol,
            format!("Could not serialize Codex request: {error}"),
            false,
        )
    })?;
    let mut writer = lock_unpoisoned(writer);
    writer
        .write_all(&serialized)
        .and_then(|_| writer.write_all(b"\n"))
        .and_then(|_| writer.flush())
        .map_err(|error| {
            ReasoningError::new(
                ReasoningErrorKind::Unavailable,
                format!("Could not write to Codex app-server: {error}"),
                true,
            )
        })
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
                "outputSchema": Self::output_schema_for(request.output_contract, request.kind),
                "serviceTier": service_tier,
                "effort": self.effort.as_str(),
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
            PendingKind::TurnSteer {
                turn_id: turn_id.clone(),
                invocation: request.invocation,
            },
        )?;
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
    stdin: Arc<Mutex<ChildStdin>>,
    state: Arc<Mutex<ProtocolState>>,
    stderr_tail: Arc<Mutex<String>>,
) -> JoinHandle<()> {
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) => match serde_json::from_str::<Value>(&line) {
                    Ok(message) => {
                        if let Some(response) = handle_message(message, &state) {
                            if let Err(error) = write_protocol_message(&stdin, &response) {
                                fail_protocol(&state, error);
                                return;
                            }
                        }
                    }
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

fn handle_message(message: Value, state: &Mutex<ProtocolState>) -> Option<Value> {
    // App-server can initiate JSON-RPC requests over the same stream. Those
    // IDs are in a different namespace from our client request IDs, so a
    // method-bearing message must be classified before response correlation.
    // Sidekick has no interactive approval/auth/tool lane: deny every server
    // request explicitly and leave any same-numbered pending request intact.
    if message.get("method").is_some() {
        if let Some(id) = message.get("id") {
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("<invalid>");
            return Some(json!({
                "id": id,
                "error": {
                    "code": -32601,
                    "message": format!("Minutes Sidekick denies server request `{method}`")
                }
            }));
        }
    }

    if let Some(id) = message.get("id").and_then(Value::as_u64) {
        let pending = lock_unpoisoned(state).pending.remove(&id);
        if let Some(pending) = pending {
            if let Some(error) = message.get("error") {
                let _ = pending
                    .sender
                    .send(Err(protocol_error(&pending.method, error)));
                return None;
            }
            let result = message.get("result").cloned().unwrap_or(Value::Null);
            match pending.kind {
                PendingKind::TurnStart {
                    sink,
                    started_at,
                    invocation,
                } => {
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
                PendingKind::TurnSteer {
                    turn_id,
                    invocation,
                } => {
                    let updated = {
                        let mut state = lock_unpoisoned(state);
                        state.turns.get_mut(turn_id.as_str()).map(|turn| {
                            turn.invocation = invocation;
                        })
                    };
                    if updated.is_none() {
                        let _ = pending.sender.send(Err(ReasoningError::new(
                            ReasoningErrorKind::Unavailable,
                            "reasoning turn completed before steering was committed",
                            true,
                        )));
                        return None;
                    }
                }
                PendingKind::Ordinary => {}
            }
            let _ = pending.sender.send(Ok(result));
        }
        return None;
    }

    let Some(method) = message.get("method").and_then(Value::as_str) else {
        return None;
    };
    let params = message.get("params").unwrap_or(&Value::Null);
    let turn_id = params
        .get("turnId")
        .and_then(Value::as_str)
        .or_else(|| params.pointer("/turn/id").and_then(Value::as_str));
    let Some(turn_id) = turn_id else {
        return None;
    };

    match method {
        "item/agentMessage/delta" => {
            let delta = params.get("delta").and_then(Value::as_str).unwrap_or("");
            if delta.is_empty() {
                return None;
            }
            let starts_visible_latency = !delta.trim().is_empty();
            let event = {
                let mut state = lock_unpoisoned(state);
                state.turns.get_mut(turn_id).map(|turn| {
                    if starts_visible_latency {
                        turn.first_token_at.get_or_insert_with(Instant::now);
                    }
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
    None
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
    use std::sync::mpsc::{RecvTimeoutError, TryRecvError};

    const FAKE_SERVER: &str = r#"
const readline = require('node:readline');
const rl = readline.createInterface({ input: process.stdin });
function send(value) { process.stdout.write(JSON.stringify(value) + '\n'); }
rl.on('line', (line) => {
  const msg = JSON.parse(line);
  if (msg.method === 'initialize') send({ id: msg.id, result: { userAgent: 'fake' } });
  else if (msg.method === 'thread/start') {
    if (msg.params.model !== 'gpt-5.6-terra') send({ id: msg.id, error: { code: -32602, message: 'wrong model' } });
    else send({ id: msg.id, result: { thread: { id: 'thread-1' } } });
  }
  else if (msg.method === 'turn/start') {
    if (msg.params.effort !== 'none') send({ id: msg.id, error: { code: -32602, message: 'wrong effort' } });
    else send({ id: msg.id, result: { turn: { id: 'turn-1' } } });
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
            candidate_to_verify: None,
        }
    }

    #[test]
    fn image_input_dispatches_the_exact_selected_bytes_inline() {
        let png = b"\x89PNG\r\n\x1a\nnonce-only".to_vec();
        let mut request = request();
        request.window.latest_image = Some(minutes_core::live_sidekick::ReasoningImageEvidence {
            evidence_id: "screen-1".into(),
            capture_session_id: "capture-a".into(),
            path: PathBuf::from("/tmp/provider-screen.png"),
            png_bytes: png.clone(),
            sha256: "f".repeat(64),
        });

        let input = CodexReasoningSession::input_for(&request);

        assert_eq!(input.len(), 2);
        assert_eq!(input[1]["type"], "image");
        assert_eq!(input[1]["detail"], "high");
        assert_eq!(
            input[1]["url"],
            format!("data:image/png;base64,{}", BASE64_STANDARD.encode(png))
        );
        assert!(input[1].get("path").is_none());
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
    fn stream_deltas_preserve_whitespace_and_start_latency_on_visible_content() {
        let state = Mutex::new(ProtocolState::default());
        let (sender, receiver) = mpsc::sync_channel(8);
        lock_unpoisoned(&state).turns.insert(
            "turn-empty-delta".into(),
            ActiveTurn {
                id: "turn-empty-delta".into(),
                invocation: request().invocation,
                sink: Arc::new(move |event| {
                    let _ = sender.send(event);
                }),
                started_at: Instant::now(),
                first_token_at: None,
                text: String::new(),
            },
        );

        assert!(handle_message(
            json!({
                "method": "item/agentMessage/delta",
                "params": { "turnId": "turn-empty-delta", "delta": "" }
            }),
            &state,
        )
        .is_none());
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        handle_message(
            json!({
                "method": "item/agentMessage/delta",
                "params": { "turnId": "turn-empty-delta", "delta": "   " }
            }),
            &state,
        );
        {
            let state = lock_unpoisoned(&state);
            let turn = state.turns.get("turn-empty-delta").unwrap();
            assert!(turn.first_token_at.is_none());
            assert_eq!(turn.text, "   ");
        }
        match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            ReasoningStreamEvent::TextDelta { text, .. } => assert_eq!(text, "   "),
            event => panic!("whitespace delta emitted wrong event: {event:?}"),
        }

        for delta in ["Hello", " ", "world"] {
            handle_message(
                json!({
                    "method": "item/agentMessage/delta",
                    "params": { "turnId": "turn-empty-delta", "delta": delta }
                }),
                &state,
            );
        }
        {
            let state = lock_unpoisoned(&state);
            let turn = state.turns.get("turn-empty-delta").unwrap();
            assert!(turn.first_token_at.is_some());
            assert_eq!(turn.text, "   Hello world");
        }
        let streamed = (0..3)
            .map(
                |_| match receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
                    ReasoningStreamEvent::TextDelta { text, .. } => text,
                    event => panic!("content delta emitted wrong event: {event:?}"),
                },
            )
            .collect::<String>();
        assert_eq!(streamed, "Hello world");
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
    }

    #[test]
    fn server_request_with_colliding_id_is_denied_without_consuming_pending_response() {
        let state = Mutex::new(ProtocolState::default());
        let (sender, receiver) = mpsc::sync_channel(1);
        lock_unpoisoned(&state).pending.insert(
            7,
            PendingRequest {
                method: "turn/start".into(),
                kind: PendingKind::Ordinary,
                sender,
            },
        );

        let denial = handle_message(
            json!({
                "method": "item/tool/requestUserInput",
                "id": 7,
                "params": { "prompt": "approve this" }
            }),
            &state,
        )
        .expect("server request must receive a fail-closed response");
        assert_eq!(denial["id"], 7);
        assert_eq!(
            denial.pointer("/error/code").and_then(Value::as_i64),
            Some(-32601)
        );
        assert!(denial
            .pointer("/error/message")
            .and_then(Value::as_str)
            .unwrap()
            .contains("denies server request"));
        assert!(lock_unpoisoned(&state).pending.contains_key(&7));
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));

        assert!(
            handle_message(json!({ "id": 7, "result": { "accepted": true } }), &state).is_none()
        );
        assert_eq!(
            receiver
                .recv_timeout(Duration::from_secs(1))
                .unwrap()
                .unwrap()["accepted"],
            true
        );
    }

    #[test]
    fn back_to_back_server_requests_are_each_denied() {
        let state = Mutex::new(ProtocolState::default());
        let first = handle_message(
            json!({ "method": "account/login/start", "id": "server-a", "params": {} }),
            &state,
        )
        .expect("first denial");
        let second = handle_message(
            json!({ "method": "item/tool/call", "id": "server-b", "params": {} }),
            &state,
        )
        .expect("second denial");
        assert_eq!(first["id"], "server-a");
        assert_eq!(second["id"], "server-b");
        assert_eq!(first.pointer("/error/code"), second.pointer("/error/code"));
        assert!(lock_unpoisoned(&state).pending.is_empty());
    }

    #[test]
    fn steer_response_commits_invocation_before_back_to_back_completion() {
        let state = Mutex::new(ProtocolState::default());
        let (event_sender, event_receiver) = mpsc::sync_channel(1);
        lock_unpoisoned(&state).turns.insert(
            "turn-steered".into(),
            ActiveTurn {
                id: "turn-steered".into(),
                invocation: request().invocation,
                sink: Arc::new(move |event| {
                    let _ = event_sender.send(event);
                }),
                started_at: Instant::now(),
                first_token_at: None,
                text: "steered answer".into(),
            },
        );
        let new_invocation = InvocationIdentity {
            sequence: 2,
            source_policy_generation: 0,
            user_generation: 1,
        };
        let (request_sender, request_receiver) = mpsc::sync_channel(1);
        lock_unpoisoned(&state).pending.insert(
            11,
            PendingRequest {
                method: "turn/steer".into(),
                kind: PendingKind::TurnSteer {
                    turn_id: "turn-steered".into(),
                    invocation: new_invocation,
                },
                sender: request_sender,
            },
        );

        handle_message(
            json!({ "id": 11, "result": { "turnId": "turn-steered" } }),
            &state,
        );
        handle_message(
            json!({
                "method": "turn/completed",
                "params": { "turn": { "id": "turn-steered", "status": "completed" } }
            }),
            &state,
        );

        request_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap()
            .unwrap();
        match event_receiver.recv_timeout(Duration::from_secs(1)).unwrap() {
            ReasoningStreamEvent::Completed { invocation, .. } => {
                assert_eq!(invocation, new_invocation);
            }
            event => panic!("steered turn emitted wrong terminal event: {event:?}"),
        }
    }

    #[test]
    fn intervention_schema_requires_visual_claim_provenance() {
        let schema = CodexReasoningSession::output_schema_for(
            ReasoningOutputContract::InterventionCandidateV1,
            ReasoningTurnKind::Foreground,
        );
        assert_eq!(
            schema.pointer("/properties/claims_visual_observation/type"),
            Some(&json!("boolean"))
        );
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("claims_visual_observation")));
        assert!(schema
            .pointer("/properties/text/description")
            .and_then(Value::as_str)
            .unwrap()
            .contains("directionally complete obligation"));
        assert_eq!(
            schema.pointer("/properties/text/maxLength"),
            Some(&json!(700))
        );
        let background_schema = CodexReasoningSession::output_schema_for(
            ReasoningOutputContract::InterventionCandidateV1,
            ReasoningTurnKind::Background,
        );
        assert_eq!(
            background_schema.pointer("/properties/text/maxLength"),
            Some(&json!(700))
        );
    }

    #[test]
    fn evidence_verifier_schema_is_structured_and_fail_closed() {
        let schema = CodexReasoningSession::output_schema_for(
            ReasoningOutputContract::EvidenceVerificationV1,
            ReasoningTurnKind::Foreground,
        );
        assert_eq!(
            schema.pointer("/properties/decision/enum"),
            Some(&json!(["allow", "reject"]))
        );
        assert_eq!(
            schema.pointer("/properties/reason_code/enum"),
            Some(&json!([
                "supported",
                "unsupported_fact",
                "unsupported_visual",
                "contradiction",
                "uncertain"
            ]))
        );
        assert!(schema["required"]
            .as_array()
            .unwrap()
            .contains(&json!("reason_code")));
        assert_eq!(schema["additionalProperties"], json!(false));
    }

    #[test]
    fn authentication_is_copied_without_user_config_or_hooks() {
        let source = tempfile::tempdir().unwrap();
        let target = tempfile::tempdir().unwrap();
        fs::write(source.path().join("auth.json"), r#"{"token":"synthetic"}"#).unwrap();
        fs::write(source.path().join("config.toml"), "model = 'user-model'").unwrap();
        fs::write(
            source.path().join("hooks.json"),
            r#"{"hooks":["dangerous"]}"#,
        )
        .unwrap();

        copy_codex_authentication(Some(source.path()), target.path()).unwrap();

        assert_eq!(
            fs::read_to_string(target.path().join("auth.json")).unwrap(),
            r#"{"token":"synthetic"}"#
        );
        assert!(!target.path().join("config.toml").exists());
        assert!(!target.path().join("hooks.json").exists());
    }

    #[test]
    fn production_factory_disables_tool_and_mcp_lanes() {
        let backend = CodexReasoningBackend::sidekick(
            PathBuf::from("/usr/bin/codex"),
            vec!["github".into(), "slack".into(), "github".into()],
        )
        .unwrap();
        let joined = backend.args.join(" ");
        assert!(backend.args.iter().any(|arg| arg == "--strict-config"));
        for required in [
            "--disable shell_tool",
            "--disable apps",
            "--disable plugins",
            "--disable hooks",
            "--disable auth_elicitation",
            "--disable tool_call_mcp_elicitation",
            "--disable workspace_dependencies",
            "--disable skill_mcp_dependency_install",
            "--disable browser_use",
            "--disable computer_use",
            "mcp_servers={}",
            "service_tier=\"fast\"",
            "model_reasoning_effort=\"none\"",
        ] {
            assert!(joined.contains(required), "missing {required}: {joined}");
        }
        assert!(!joined.contains("mcp_servers.github"));
        assert!(!joined.contains("mcp_servers.slack"));
        assert!(backend.cwd.is_absolute());
        assert!(backend.home.starts_with(&backend.cwd));
        assert!(backend.codex_home.starts_with(&backend.cwd));
        if let Some(user_home) = std::env::var_os("HOME").map(PathBuf::from) {
            assert_ne!(backend.home, user_home);
        }
        assert!(!backend.codex_home.join("config.toml").exists());
        assert!(!backend.codex_home.join("hooks.json").exists());

        let command = backend.command();
        let environment = command
            .get_envs()
            .map(|(key, value)| (key.to_owned(), value.map(ToOwned::to_owned)))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            environment.get(std::ffi::OsStr::new("HOME")),
            Some(&Some(backend.home.as_os_str().to_owned()))
        );
        assert_eq!(
            environment.get(std::ffi::OsStr::new("CODEX_HOME")),
            Some(&Some(backend.codex_home.as_os_str().to_owned()))
        );
        assert_eq!(command.get_current_dir(), Some(backend.cwd.as_path()));

        let verifier = CodexReasoningBackend::sidekick_verifier(
            PathBuf::from("/usr/bin/codex"),
            Vec::<String>::new(),
        )
        .unwrap();
        assert_eq!(backend.descriptor().model, "gpt-5.6-terra");
        assert_eq!(verifier.descriptor().model, "gpt-5.6-terra");
        assert_eq!(backend.effort, "none");
        assert_eq!(verifier.effort, "low");
        assert!(verifier
            .args
            .join(" ")
            .contains("model_reasoning_effort=\"low\""));
    }
}
