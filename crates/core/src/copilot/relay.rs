//! Transient, local-only attachment relay for capture consumers.
//!
//! The durable transcript/event log remains authoritative. This module only
//! mirrors live evidence and copilot nudges so another Minutes process can
//! attach without opening a second microphone.

use super::{CopilotEvidenceMode, CopilotUtterance, Nudge, TranscriptUpdateKind};
use crate::events::MinutesEvent;
use crate::live_partials::{LivePartialEvent, LivePartialSubscriber, SupersessionReason};
use chrono::{DateTime, Utc};
use interprocess::{
    local_socket::{
        prelude::*, ConnectOptions, GenericFilePath, Listener, ListenerNonblockingMode,
        ListenerOptions, Stream,
    },
    ConnectWaitMode,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const RELAY_PROTOCOL_VERSION: u32 = 1;
const DISCOVERY_FILE: &str = "capture-relay.json";
const OWNER_LOCK_FILE: &str = "capture-relay.lock";
const UNIX_SOCKET_FILE: &str = "capture-relay.sock";
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(1);
const HEARTBEAT_STALE_AFTER: Duration = Duration::from_secs(5);
const ESTABLISHMENT_RETRY_TIMEOUT: Duration = Duration::from_secs(1);
const ESTABLISHMENT_RETRY_DELAY: Duration = Duration::from_millis(25);
const ESTABLISHMENT_RETRY_LIMIT: u8 = 20;
const FRAME_CAPACITY: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RelayTransport {
    UnixSocket,
    WindowsNamedPipe,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRelayDiscovery {
    pub v: u32,
    pub session_id: String,
    pub transport: RelayTransport,
    pub endpoint: String,
    pub owner_pid: u32,
    pub evidence_mode: CopilotEvidenceMode,
    pub auth_token: String,
    pub started_at: DateTime<Utc>,
    pub heartbeat_at: DateTime<Utc>,
}

impl CaptureRelayDiscovery {
    pub fn heartbeat_is_fresh(&self, now: DateTime<Utc>) -> bool {
        now.signed_duration_since(self.heartbeat_at)
            <= chrono::Duration::from_std(HEARTBEAT_STALE_AFTER)
                .expect("relay heartbeat threshold fits chrono")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelayCursor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub transcript_seq: u64,
    #[serde(default)]
    pub nudge_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RelayTranscriptUpdate {
    Utterance {
        session_epoch: u64,
        utterance: CopilotUtterance,
        /// Capture-to-relay time for a partial. Finals use zero because the
        /// durable event contract does not expose a monotonic audio timestamp.
        producer_latency_ms: u64,
    },
    Superseded {
        session_epoch: u64,
        through_utterance_sequence: u64,
        last_revision: u64,
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RelayFrame {
    Attached {
        v: u32,
        session_id: String,
        owner_pid: u32,
        evidence_mode: CopilotEvidenceMode,
        transcript_seq: u64,
        nudge_seq: u64,
    },
    Transcript {
        seq: u64,
        update: RelayTranscriptUpdate,
    },
    Nudge {
        seq: u64,
        nudge: Nudge,
    },
    Heartbeat {
        owner_pid: u32,
        transcript_seq: u64,
        nudge_seq: u64,
        sent_at: DateTime<Utc>,
    },
    CursorReset {
        session_id: String,
        reason: String,
    },
    Gap {
        stream: String,
        requested_after: u64,
        available_from: u64,
    },
    Published {
        nudge_seq: u64,
    },
    Error {
        message: String,
    },
    Shutdown {
        reason: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureAttachPlan {
    Attach,
    StartExplicitCapture,
    WaitForCapture,
    RefuseDuplicate { message: String },
}

/// Decide whether a copilot process may open capture.
///
/// An existing capture owner always wins. If its relay cannot be reached, the
/// caller must explain that attachment failed and must not fall through to a
/// new microphone stream.
pub fn plan_capture_attachment(
    capture_active: bool,
    relay_available: bool,
    explicit_new_capture: bool,
) -> CaptureAttachPlan {
    if capture_active && relay_available {
        CaptureAttachPlan::Attach
    } else if capture_active {
        CaptureAttachPlan::RefuseDuplicate {
            message: "A recording is already using the microphone, but its secure attachment relay is unavailable. Copilot did not open a second microphone. Update or restart the process that owns the recording, then try again.".into(),
        }
    } else if explicit_new_capture {
        CaptureAttachPlan::StartExplicitCapture
    } else {
        CaptureAttachPlan::WaitForCapture
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureRelayError {
    #[error("no active capture relay was found; start a recording or Live Transcript first. Copilot did not open the microphone")]
    NotFound,
    #[error("the capture relay heartbeat is stale; Copilot did not open a second microphone")]
    Stale,
    #[error("the capture relay belongs to process {0}, which is no longer running")]
    OwnerExited(u32),
    #[error("capture relay protocol {found} is not supported by this build (expected {expected})")]
    Protocol { found: u32, expected: u32 },
    #[error("capture relay authentication failed")]
    Authentication,
    #[error("capture relay is already owned by process {0}")]
    AlreadyOwned(u32),
    #[error("another local process is starting or stopping the capture relay")]
    OwnershipBusy,
    #[error("capture relay I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("capture relay data was invalid: {0}")]
    InvalidData(String),
}

#[derive(Debug, Serialize, Deserialize)]
struct ClientHello {
    v: u32,
    auth_token: String,
    action: ClientAction,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientAction {
    Observe { cursor: RelayCursor },
    PublishNudge { nudge: Nudge },
}

#[derive(Debug)]
struct RelayState {
    session_id: String,
    owner_pid: u32,
    evidence_mode: CopilotEvidenceMode,
    transcript_seq: u64,
    nudge_seq: u64,
    transcript: VecDeque<(u64, RelayTranscriptUpdate)>,
    nudges: VecDeque<(u64, Nudge)>,
    shutdown_reason: Option<String>,
}

impl RelayState {
    fn push_transcript(&mut self, update: RelayTranscriptUpdate) -> u64 {
        self.transcript_seq = self.transcript_seq.saturating_add(1);
        let seq = self.transcript_seq;
        self.transcript.push_back((seq, update));
        trim_to_capacity(&mut self.transcript);
        seq
    }

    fn push_nudge(&mut self, nudge: Nudge) -> u64 {
        self.nudge_seq = self.nudge_seq.saturating_add(1);
        let seq = self.nudge_seq;
        self.nudges.push_back((seq, nudge));
        trim_to_capacity(&mut self.nudges);
        seq
    }
}

fn trim_to_capacity<T>(queue: &mut VecDeque<(u64, T)>) {
    while queue.len() > FRAME_CAPACITY {
        queue.pop_front();
    }
}

struct SharedRelayState {
    state: Mutex<RelayState>,
    changed: Condvar,
}

pub struct CaptureRelayServer {
    shared: Arc<SharedRelayState>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    _owner_lock: std::fs::File,
    discovery_path: PathBuf,
    discovery: CaptureRelayDiscovery,
}

impl CaptureRelayServer {
    pub fn start(
        evidence_mode: CopilotEvidenceMode,
        partials: Option<LivePartialSubscriber>,
    ) -> Result<Self, CaptureRelayError> {
        Self::start_inner(
            &crate::config::Config::minutes_dir(),
            evidence_mode,
            partials,
            true,
        )
    }

    fn start_inner(
        dir: &Path,
        evidence_mode: CopilotEvidenceMode,
        partials: Option<LivePartialSubscriber>,
        poll_durable_events: bool,
    ) -> Result<Self, CaptureRelayError> {
        std::fs::create_dir_all(dir)?;
        let discovery_path = capture_relay_discovery_path_in(dir);
        // Discovery is the platform-independent authority for refusing a live
        // owner. Check before locking so Windows lock error kinds cannot change
        // the public result, then re-check after acquiring the lock to close the
        // check/lock TOCTOU window. The retained lock still serializes claims and
        // makes a crashed owner's stale discovery reclaimable.
        if let Some(owner_pid) = live_fresh_owner_pid(&discovery_path) {
            return Err(CaptureRelayError::AlreadyOwned(owner_pid));
        }
        let owner_lock = acquire_owner_lock(dir, &discovery_path)?;
        if let Some(owner_pid) = live_fresh_owner_pid(&discovery_path) {
            return Err(CaptureRelayError::AlreadyOwned(owner_pid));
        }

        let session_id = random_hex(16)?;
        let auth_token = random_hex(32)?;
        let endpoint = relay_endpoint(dir, &session_id);
        #[cfg(unix)]
        remove_if_present(Path::new(&endpoint))?;
        let name = endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let listener: Listener = ListenerOptions::new()
            .name(name)
            .nonblocking(ListenerNonblockingMode::Accept)
            .create_sync()?;

        #[cfg(unix)]
        set_owner_only_permissions(Path::new(&endpoint))?;

        let now = Utc::now();
        let discovery = CaptureRelayDiscovery {
            v: RELAY_PROTOCOL_VERSION,
            session_id: session_id.clone(),
            transport: current_transport(),
            endpoint,
            owner_pid: std::process::id(),
            evidence_mode,
            auth_token,
            started_at: now,
            heartbeat_at: now,
        };
        write_discovery(&discovery_path, &discovery)?;

        let shared = Arc::new(SharedRelayState {
            state: Mutex::new(RelayState {
                session_id,
                owner_pid: discovery.owner_pid,
                evidence_mode,
                transcript_seq: 0,
                nudge_seq: 0,
                transcript: VecDeque::new(),
                nudges: VecDeque::new(),
                shutdown_reason: None,
            }),
            changed: Condvar::new(),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_shared = Arc::clone(&shared);
        let thread_stop = Arc::clone(&stop);
        let thread_discovery_path = discovery_path.clone();
        let thread_discovery = discovery.clone();
        let thread = thread::Builder::new()
            .name("capture-attach-relay".into())
            .spawn(move || {
                run_server(
                    listener,
                    thread_shared,
                    thread_stop,
                    thread_discovery_path,
                    thread_discovery,
                    partials,
                    poll_durable_events,
                );
            })?;

        Ok(Self {
            shared,
            stop,
            thread: Some(thread),
            _owner_lock: owner_lock,
            discovery_path,
            discovery,
        })
    }

    pub fn discovery(&self) -> &CaptureRelayDiscovery {
        &self.discovery
    }

    pub fn publish_nudge(&self, nudge: Nudge) -> u64 {
        let mut state = lock_state(&self.shared);
        let seq = state.push_nudge(nudge);
        self.shared.changed.notify_all();
        seq
    }

    pub fn shutdown(mut self, reason: impl Into<String>) {
        self.shutdown_in_place(reason.into());
    }

    fn shutdown_in_place(&mut self, reason: String) {
        {
            let mut state = lock_state(&self.shared);
            if state.shutdown_reason.is_none() {
                state.shutdown_reason = Some(reason);
            }
            self.shared.changed.notify_all();
        }
        self.stop.store(true, Ordering::Release);
        wake_listener(&self.discovery);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        remove_discovery_if_owned(&self.discovery_path, &self.discovery.session_id);
    }

    #[cfg(test)]
    fn start_for_test(dir: &Path) -> Result<Self, CaptureRelayError> {
        Self::start_inner(dir, CopilotEvidenceMode::InProcessPartials, None, false)
    }

    #[cfg(test)]
    fn publish_transcript_for_test(&self, update: RelayTranscriptUpdate) -> u64 {
        let mut state = lock_state(&self.shared);
        let seq = state.push_transcript(update);
        self.shared.changed.notify_all();
        seq
    }
}

impl Drop for CaptureRelayServer {
    fn drop(&mut self) {
        if self.thread.is_some() {
            self.shutdown_in_place("capture owner stopped".into());
        }
    }
}

pub struct CaptureRelayClient {
    discovery_path: PathBuf,
    discovery: CaptureRelayDiscovery,
    reader: BufReader<Stream>,
    pending_line: String,
    cursor: RelayCursor,
    // The attach acknowledgement is internal. Establishment completes only
    // after a frame has been delivered to the caller through `try_recv`.
    established: bool,
    establishment_retry: EstablishmentRetry,
}

#[derive(Clone, Copy)]
struct EstablishmentRetry {
    deadline: Option<Instant>,
    attempts_remaining: u8,
}

impl EstablishmentRetry {
    fn new() -> Self {
        Self {
            deadline: None,
            attempts_remaining: ESTABLISHMENT_RETRY_LIMIT,
        }
    }

    fn started(self) -> bool {
        self.deadline.is_some()
    }

    fn remaining(self) -> Option<Duration> {
        self.deadline
            .map(|deadline| deadline.saturating_duration_since(Instant::now()))
    }

    fn wait_before_retry(&mut self) -> bool {
        if self.attempts_remaining == 0 {
            return false;
        }
        let deadline = self
            .deadline
            .get_or_insert_with(|| Instant::now() + ESTABLISHMENT_RETRY_TIMEOUT);
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }

        thread::sleep(ESTABLISHMENT_RETRY_DELAY.min(remaining));
        self.attempts_remaining -= 1;
        Instant::now() < *deadline
    }
}

impl CaptureRelayClient {
    pub fn connect(cursor: RelayCursor) -> Result<Self, CaptureRelayError> {
        Self::connect_from(&capture_relay_discovery_path(), cursor)
    }

    fn connect_from(discovery_path: &Path, cursor: RelayCursor) -> Result<Self, CaptureRelayError> {
        let mut establishment_retry = EstablishmentRetry::new();
        loop {
            match Self::connect_once(discovery_path, cursor.clone(), establishment_retry) {
                Ok(client) => return Ok(client),
                Err(error)
                    if is_unexpected_eof(&error)
                        || (establishment_retry.started()
                            && is_retryable_establishment_error(&error)) =>
                {
                    if !establishment_retry.wait_before_retry() {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn connect_once(
        discovery_path: &Path,
        cursor: RelayCursor,
        establishment_retry: EstablishmentRetry,
    ) -> Result<Self, CaptureRelayError> {
        let discovery = read_discovery(discovery_path).ok_or(CaptureRelayError::NotFound)?;
        validate_discovery(&discovery)?;
        let name = discovery
            .endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut stream = match establishment_retry.remaining() {
            Some(timeout) => ConnectOptions::new()
                .name(name)
                .wait_mode(ConnectWaitMode::Timeout(timeout))
                .connect_sync()?,
            None => Stream::connect(name)?,
        };
        write_json_line(
            &mut stream,
            &ClientHello {
                v: RELAY_PROTOCOL_VERSION,
                auth_token: discovery.auth_token.clone(),
                action: ClientAction::Observe {
                    cursor: cursor.clone(),
                },
            },
        )?;
        let mut reader = BufReader::new(stream);
        let first = read_frame_blocking(&mut reader)?;
        match first {
            RelayFrame::Attached { .. } | RelayFrame::CursorReset { .. } => {}
            RelayFrame::Error { message } if message.contains("authentication") => {
                return Err(CaptureRelayError::Authentication);
            }
            RelayFrame::Error { message } => return Err(CaptureRelayError::InvalidData(message)),
            frame => {
                return Err(CaptureRelayError::InvalidData(format!(
                    "expected relay attachment acknowledgement, got {frame:?}"
                )));
            }
        }
        reader.get_ref().set_nonblocking(true)?;
        Ok(Self {
            discovery_path: discovery_path.to_path_buf(),
            discovery: discovery.clone(),
            reader,
            pending_line: String::new(),
            cursor: RelayCursor {
                session_id: Some(discovery.session_id),
                ..cursor
            },
            established: false,
            establishment_retry,
        })
    }

    pub fn discovery(&self) -> &CaptureRelayDiscovery {
        &self.discovery
    }

    pub fn cursor(&self) -> RelayCursor {
        self.cursor.clone()
    }

    pub fn try_recv(&mut self) -> Result<Option<RelayFrame>, CaptureRelayError> {
        loop {
            match self.reader.read_line(&mut self.pending_line) {
                Ok(0) if !self.established => {
                    self.reconnect_during_establishment(connection_closed_error())?;
                }
                Ok(0) => return Err(connection_closed_error()),
                Ok(_) if !self.pending_line.ends_with('\n') => return Ok(None),
                Ok(_) => {
                    let line = std::mem::take(&mut self.pending_line);
                    let frame: RelayFrame =
                        serde_json::from_str(line.trim_end()).map_err(|error| {
                            CaptureRelayError::InvalidData(format!("invalid relay frame: {error}"))
                        })?;
                    self.observe_cursor(&frame);
                    self.established = true;
                    return Ok(Some(frame));
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                Err(error) if !self.established && error.kind() == io::ErrorKind::UnexpectedEof => {
                    self.reconnect_during_establishment(CaptureRelayError::Io(error))?;
                }
                Err(error) => return Err(CaptureRelayError::Io(error)),
            }
        }
    }

    fn reconnect_during_establishment(
        &mut self,
        mut last_error: CaptureRelayError,
    ) -> Result<(), CaptureRelayError> {
        let discovery_path = self.discovery_path.clone();
        let cursor = self.cursor.clone();
        while self.establishment_retry.wait_before_retry() {
            match Self::connect_once(&discovery_path, cursor.clone(), self.establishment_retry) {
                Ok(client) => {
                    *self = client;
                    return Ok(());
                }
                Err(error) if is_retryable_establishment_error(&error) => {
                    last_error = error;
                }
                Err(error) => return Err(error),
            }
        }
        Err(last_error)
    }

    pub fn publish_nudge(nudge: Nudge) -> Result<u64, CaptureRelayError> {
        Self::publish_nudge_via(&capture_relay_discovery_path(), nudge)
    }

    fn publish_nudge_via(discovery_path: &Path, nudge: Nudge) -> Result<u64, CaptureRelayError> {
        let discovery = read_discovery(discovery_path).ok_or(CaptureRelayError::NotFound)?;
        validate_discovery(&discovery)?;
        let name = discovery
            .endpoint
            .as_str()
            .to_fs_name::<GenericFilePath>()
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        let mut stream = Stream::connect(name)?;
        write_json_line(
            &mut stream,
            &ClientHello {
                v: RELAY_PROTOCOL_VERSION,
                auth_token: discovery.auth_token,
                action: ClientAction::PublishNudge { nudge },
            },
        )?;
        let mut reader = BufReader::new(stream);
        match read_frame_blocking(&mut reader)? {
            RelayFrame::Published { nudge_seq } => Ok(nudge_seq),
            RelayFrame::Error { message } if message.contains("authentication") => {
                Err(CaptureRelayError::Authentication)
            }
            RelayFrame::Error { message } => Err(CaptureRelayError::InvalidData(message)),
            frame => Err(CaptureRelayError::InvalidData(format!(
                "unexpected publish response: {frame:?}"
            ))),
        }
    }

    fn observe_cursor(&mut self, frame: &RelayFrame) {
        match frame {
            RelayFrame::Transcript { seq, .. } => {
                self.cursor.transcript_seq = self.cursor.transcript_seq.max(*seq);
            }
            RelayFrame::Nudge { seq, .. } => {
                self.cursor.nudge_seq = self.cursor.nudge_seq.max(*seq);
            }
            RelayFrame::CursorReset { session_id, .. } => {
                self.cursor = RelayCursor {
                    session_id: Some(session_id.clone()),
                    ..RelayCursor::default()
                };
            }
            _ => {}
        }
    }
}

fn connection_closed_error() -> CaptureRelayError {
    CaptureRelayError::Io(io::Error::new(
        io::ErrorKind::UnexpectedEof,
        "capture relay closed the connection",
    ))
}

fn is_unexpected_eof(error: &CaptureRelayError) -> bool {
    matches!(error, CaptureRelayError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof)
}

fn is_retryable_establishment_error(error: &CaptureRelayError) -> bool {
    matches!(
        error,
        CaptureRelayError::Io(error)
            if !matches!(
                error.kind(),
                io::ErrorKind::InvalidInput
                    | io::ErrorKind::PermissionDenied
                    | io::ErrorKind::Unsupported
            )
    )
}

pub fn capture_relay_discovery_path() -> PathBuf {
    capture_relay_discovery_path_in(&crate::config::Config::minutes_dir())
}

fn capture_relay_discovery_path_in(dir: &Path) -> PathBuf {
    dir.join(DISCOVERY_FILE)
}

fn run_server(
    listener: Listener,
    shared: Arc<SharedRelayState>,
    stop: Arc<AtomicBool>,
    discovery_path: PathBuf,
    mut discovery: CaptureRelayDiscovery,
    mut partials: Option<LivePartialSubscriber>,
    poll_durable_events: bool,
) {
    let mut event_cursor = if poll_durable_events {
        crate::events::latest_event_seq()
    } else {
        0
    };
    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;

    while !stop.load(Ordering::Acquire) {
        loop {
            match listener.accept() {
                Ok(stream) => {
                    spawn_client(stream, Arc::clone(&shared), discovery.auth_token.clone())
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => {
                    tracing::warn!(error = %error, "capture relay accept failed");
                    break;
                }
            }
        }

        if let Some(subscriber) = partials.as_mut() {
            while let Some(event) = subscriber.try_recv() {
                let update = relay_partial_update(event);
                let mut state = lock_state(&shared);
                state.push_transcript(update);
                shared.changed.notify_all();
            }
        }

        if poll_durable_events {
            for envelope in crate::events::read_events_since_seq(event_cursor, None) {
                event_cursor = event_cursor.max(envelope.seq);
                let MinutesEvent::LiveUtteranceFinal {
                    source,
                    text,
                    speaker,
                    offset_ms,
                    duration_ms,
                    ..
                } = envelope.event
                else {
                    continue;
                };
                if text.trim().is_empty() {
                    continue;
                }
                let update = RelayTranscriptUpdate::Utterance {
                    session_epoch: 0,
                    utterance: CopilotUtterance {
                        utterance_sequence: envelope.seq,
                        revision: envelope.seq,
                        update_kind: TranscriptUpdateKind::Final,
                        source,
                        text,
                        speaker,
                        speaker_verified: false,
                        offset_ms,
                        duration_ms,
                    },
                    producer_latency_ms: 0,
                };
                let mut state = lock_state(&shared);
                state.push_transcript(update);
                shared.changed.notify_all();
            }
        }

        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            discovery.heartbeat_at = Utc::now();
            if let Err(error) = write_discovery(&discovery_path, &discovery) {
                tracing::warn!(error = %error, "capture relay heartbeat write failed");
            }
            last_heartbeat = Instant::now();
        }
        thread::sleep(Duration::from_millis(10));
    }

    {
        let mut state = lock_state(&shared);
        if state.shutdown_reason.is_none() {
            state.shutdown_reason = Some("capture owner stopped".into());
        }
        shared.changed.notify_all();
    }
    thread::sleep(Duration::from_millis(25));
    remove_discovery_if_owned(&discovery_path, &discovery.session_id);
    #[cfg(unix)]
    remove_if_present(Path::new(&discovery.endpoint)).ok();
}

fn spawn_client(stream: Stream, shared: Arc<SharedRelayState>, auth_token: String) {
    if let Err(error) = thread::Builder::new()
        .name("capture-relay-client".into())
        .spawn(move || {
            if let Err(error) = handle_client(stream, &shared, &auth_token) {
                tracing::debug!(error = %error, "capture relay client disconnected");
            }
        })
    {
        tracing::warn!(error = %error, "failed to spawn capture relay client");
    }
}

fn handle_client(
    mut stream: Stream,
    shared: &Arc<SharedRelayState>,
    auth_token: &str,
) -> Result<(), CaptureRelayError> {
    let read_stream = interprocess::TryClone::try_clone(&stream)?;
    let mut reader = BufReader::new(read_stream);
    let hello: ClientHello = read_json_line(&mut reader)?;
    if hello.v != RELAY_PROTOCOL_VERSION {
        write_frame(
            &mut stream,
            &RelayFrame::Error {
                message: format!(
                    "unsupported protocol {}; expected {}",
                    hello.v, RELAY_PROTOCOL_VERSION
                ),
            },
        )?;
        return Ok(());
    }
    if !constant_time_eq(hello.auth_token.as_bytes(), auth_token.as_bytes()) {
        write_frame(
            &mut stream,
            &RelayFrame::Error {
                message: "capture relay authentication failed".into(),
            },
        )?;
        return Ok(());
    }

    match hello.action {
        ClientAction::PublishNudge { nudge } => {
            let nudge_seq = {
                let mut state = lock_state(shared);
                state.push_nudge(nudge)
            };
            shared.changed.notify_all();
            write_frame(&mut stream, &RelayFrame::Published { nudge_seq })?;
            Ok(())
        }
        ClientAction::Observe { cursor } => observe_client(stream, shared, cursor),
    }
}

fn observe_client(
    mut stream: Stream,
    shared: &Arc<SharedRelayState>,
    mut cursor: RelayCursor,
) -> Result<(), CaptureRelayError> {
    let snapshot = lock_state(shared);
    if cursor.session_id.as_deref() != Some(snapshot.session_id.as_str()) {
        cursor = RelayCursor {
            session_id: Some(snapshot.session_id.clone()),
            ..RelayCursor::default()
        };
        write_frame(
            &mut stream,
            &RelayFrame::CursorReset {
                session_id: snapshot.session_id.clone(),
                reason: "capture owner session changed; replaying the current relay buffer".into(),
            },
        )?;
    } else {
        write_frame(
            &mut stream,
            &RelayFrame::Attached {
                v: RELAY_PROTOCOL_VERSION,
                session_id: snapshot.session_id.clone(),
                owner_pid: snapshot.owner_pid,
                evidence_mode: snapshot.evidence_mode,
                transcript_seq: snapshot.transcript_seq,
                nudge_seq: snapshot.nudge_seq,
            },
        )?;
    }
    drop(snapshot);

    let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
    loop {
        let state = lock_state(shared);
        if let Some(reason) = state.shutdown_reason.clone() {
            drop(state);
            write_frame(&mut stream, &RelayFrame::Shutdown { reason })?;
            return Ok(());
        }

        maybe_write_gap(
            &mut stream,
            "transcript",
            cursor.transcript_seq,
            state.transcript.front().map(|(seq, _)| *seq),
        )?;
        maybe_write_gap(
            &mut stream,
            "nudge",
            cursor.nudge_seq,
            state.nudges.front().map(|(seq, _)| *seq),
        )?;
        let transcript = state
            .transcript
            .iter()
            .filter(|(seq, _)| *seq > cursor.transcript_seq)
            .cloned()
            .collect::<Vec<_>>();
        let nudges = state
            .nudges
            .iter()
            .filter(|(seq, _)| *seq > cursor.nudge_seq)
            .cloned()
            .collect::<Vec<_>>();
        let heartbeat = (state.owner_pid, state.transcript_seq, state.nudge_seq);
        if transcript.is_empty()
            && nudges.is_empty()
            && last_heartbeat.elapsed() < HEARTBEAT_INTERVAL
        {
            let (next, _) = shared
                .changed
                .wait_timeout(state, Duration::from_millis(250))
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            drop(next);
            continue;
        }
        drop(state);

        for (seq, update) in transcript {
            write_frame(&mut stream, &RelayFrame::Transcript { seq, update })?;
            cursor.transcript_seq = seq;
        }
        for (seq, nudge) in nudges {
            write_frame(&mut stream, &RelayFrame::Nudge { seq, nudge })?;
            cursor.nudge_seq = seq;
        }
        if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
            write_frame(
                &mut stream,
                &RelayFrame::Heartbeat {
                    owner_pid: heartbeat.0,
                    transcript_seq: heartbeat.1,
                    nudge_seq: heartbeat.2,
                    sent_at: Utc::now(),
                },
            )?;
            last_heartbeat = Instant::now();
        }
    }
}

fn maybe_write_gap(
    stream: &mut Stream,
    stream_name: &str,
    cursor: u64,
    available_from: Option<u64>,
) -> Result<(), CaptureRelayError> {
    if let Some(available_from) = available_from {
        if cursor.saturating_add(1) < available_from {
            write_frame(
                stream,
                &RelayFrame::Gap {
                    stream: stream_name.into(),
                    requested_after: cursor,
                    available_from,
                },
            )?;
        }
    }
    Ok(())
}

fn relay_partial_update(event: LivePartialEvent) -> RelayTranscriptUpdate {
    match event {
        LivePartialEvent::Partial(partial) => RelayTranscriptUpdate::Utterance {
            session_epoch: partial.session_epoch,
            producer_latency_ms: partial
                .partial_published_at
                .saturating_duration_since(partial.audio_received_at)
                .as_millis()
                .min(u64::MAX as u128) as u64,
            utterance: CopilotUtterance {
                utterance_sequence: partial.utterance_sequence,
                revision: partial.revision,
                update_kind: TranscriptUpdateKind::Partial,
                source: "capture-relay".into(),
                text: partial.text,
                speaker: partial.speaker,
                speaker_verified: false,
                offset_ms: partial.offset_ms,
                duration_ms: 0,
            },
        },
        LivePartialEvent::Superseded(signal) => RelayTranscriptUpdate::Superseded {
            session_epoch: signal.session_epoch,
            through_utterance_sequence: signal.through_utterance_sequence,
            last_revision: signal.last_revision,
            reason: match signal.reason {
                SupersessionReason::Finalized => "finalized",
                SupersessionReason::Discarded => "discarded",
            }
            .into(),
        },
    }
}

fn validate_discovery(discovery: &CaptureRelayDiscovery) -> Result<(), CaptureRelayError> {
    if discovery.v != RELAY_PROTOCOL_VERSION {
        return Err(CaptureRelayError::Protocol {
            found: discovery.v,
            expected: RELAY_PROTOCOL_VERSION,
        });
    }
    if !crate::pid::is_process_alive(discovery.owner_pid) {
        return Err(CaptureRelayError::OwnerExited(discovery.owner_pid));
    }
    if !discovery.heartbeat_is_fresh(Utc::now()) {
        return Err(CaptureRelayError::Stale);
    }
    Ok(())
}

fn current_transport() -> RelayTransport {
    #[cfg(windows)]
    {
        RelayTransport::WindowsNamedPipe
    }
    #[cfg(not(windows))]
    {
        RelayTransport::UnixSocket
    }
}

fn relay_endpoint(dir: &Path, session_id: &str) -> String {
    #[cfg(windows)]
    {
        let _ = (dir, session_id);
        r"\\.\pipe\minutes-capture-relay".into()
    }
    #[cfg(not(windows))]
    {
        let _ = session_id;
        dir.join(UNIX_SOCKET_FILE).to_string_lossy().into_owned()
    }
}

fn acquire_owner_lock(
    dir: &Path,
    discovery_path: &Path,
) -> Result<std::fs::File, CaptureRelayError> {
    let path = dir.join(OWNER_LOCK_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path)?;
    #[cfg(unix)]
    set_owner_only_permissions(&path)?;
    match fs2::FileExt::try_lock_exclusive(&file) {
        Ok(()) => Ok(file),
        Err(_) => {
            if let Some(owner_pid) = live_fresh_owner_pid(discovery_path) {
                Err(CaptureRelayError::AlreadyOwned(owner_pid))
            } else {
                Err(CaptureRelayError::OwnershipBusy)
            }
        }
    }
}

fn live_fresh_owner_pid(discovery_path: &Path) -> Option<u32> {
    read_discovery(discovery_path).and_then(|discovery| {
        (crate::pid::is_process_alive(discovery.owner_pid)
            && discovery.heartbeat_is_fresh(Utc::now()))
        .then_some(discovery.owner_pid)
    })
}

fn random_hex(bytes: usize) -> Result<String, CaptureRelayError> {
    let mut value = vec![0_u8; bytes];
    getrandom::fill(&mut value)
        .map_err(|error| io::Error::other(format!("secure random source failed: {error}")))?;
    Ok(value.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn write_discovery(path: &Path, discovery: &CaptureRelayDiscovery) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(discovery).map_err(io::Error::other)?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    #[cfg(unix)]
    set_owner_only_permissions(&tmp)?;
    std::fs::rename(tmp, path)?;
    #[cfg(unix)]
    set_owner_only_permissions(path)?;
    Ok(())
}

fn read_discovery(path: &Path) -> Option<CaptureRelayDiscovery> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

fn remove_discovery_if_owned(path: &Path, session_id: &str) {
    if read_discovery(path)
        .as_ref()
        .map(|item| item.session_id.as_str())
        == Some(session_id)
    {
        remove_if_present(path).ok();
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

fn write_json_line(stream: &mut Stream, value: &impl Serialize) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, value).map_err(io::Error::other)?;
    stream.write_all(b"\n")?;
    stream.flush()
}

fn write_frame(stream: &mut Stream, frame: &RelayFrame) -> Result<(), CaptureRelayError> {
    write_json_line(stream, frame).map_err(CaptureRelayError::Io)
}

fn read_json_line<T: for<'de> Deserialize<'de>>(
    reader: &mut impl BufRead,
) -> Result<T, CaptureRelayError> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Err(CaptureRelayError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "capture relay peer closed before sending a request",
        )));
    }
    serde_json::from_str(line.trim_end())
        .map_err(|error| CaptureRelayError::InvalidData(error.to_string()))
}

fn read_frame_blocking(reader: &mut impl BufRead) -> Result<RelayFrame, CaptureRelayError> {
    read_json_line(reader)
}

fn lock_state(shared: &SharedRelayState) -> MutexGuard<'_, RelayState> {
    shared
        .state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
}

fn wake_listener(discovery: &CaptureRelayDiscovery) {
    let Ok(name) = discovery.endpoint.as_str().to_fs_name::<GenericFilePath>() else {
        return;
    };
    let _ = Stream::connect(name);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::copilot::{NudgeKind, OpportunityKind};
    use tempfile::TempDir;

    fn utterance(text: &str) -> RelayTranscriptUpdate {
        RelayTranscriptUpdate::Utterance {
            session_epoch: 7,
            utterance: CopilotUtterance {
                utterance_sequence: 3,
                revision: 4,
                update_kind: TranscriptUpdateKind::Partial,
                source: "test".into(),
                text: text.into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 100,
                duration_ms: 0,
            },
            producer_latency_ms: 12,
        }
    }

    fn nudge(id: &str) -> Nudge {
        Nudge {
            v: 1,
            id: id.into(),
            kind: NudgeKind::Ask,
            text: "Who owns this?".into(),
            source_chip: "owner".into(),
            opportunity: OpportunityKind::NextStep,
            confidence: 91,
            session_epoch: 7,
            evidence_revision: 4,
            evidence_utterance_sequence: 3,
            evidence_utterance_revision: 4,
            grounded_partial_utterance_sequence: Some(3),
            grounded_partial_utterance_revision: Some(4),
            update_kind: TranscriptUpdateKind::Partial,
            created_ts: Utc::now(),
            ttl_ms: 12_000,
            supersedes: None,
        }
    }

    fn wait_for_frame(
        client: &mut CaptureRelayClient,
        predicate: impl Fn(&RelayFrame) -> bool,
    ) -> RelayFrame {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Some(frame) = client.try_recv().unwrap() {
                if predicate(&frame) {
                    return frame;
                }
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for relay frame");
    }

    fn wait_for_error(client: &mut CaptureRelayClient) -> CaptureRelayError {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match client.try_recv() {
                Ok(_) => thread::sleep(Duration::from_millis(10)),
                Err(error) => return error,
            }
        }
        panic!("timed out waiting for relay error");
    }

    #[test]
    fn connect_reconnect_replays_only_after_both_cursors() {
        let dir = TempDir::new().unwrap();
        let server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        server.publish_transcript_for_test(utterance("first"));
        server.publish_nudge(nudge("nudge-1"));

        let discovery_path = capture_relay_discovery_path_in(dir.path());
        let mut first =
            CaptureRelayClient::connect_from(&discovery_path, RelayCursor::default()).unwrap();
        wait_for_frame(&mut first, |frame| {
            matches!(frame, RelayFrame::Transcript { seq: 1, .. })
        });
        wait_for_frame(&mut first, |frame| {
            matches!(frame, RelayFrame::Nudge { seq: 1, .. })
        });
        let cursor = first.cursor();
        drop(first);

        server.publish_transcript_for_test(utterance("second"));
        server.publish_nudge(nudge("nudge-2"));
        let mut second = CaptureRelayClient::connect_from(&discovery_path, cursor).unwrap();
        let transcript = wait_for_frame(&mut second, |frame| {
            matches!(frame, RelayFrame::Transcript { seq: 2, .. })
        });
        let replayed_nudge = wait_for_frame(&mut second, |frame| {
            matches!(frame, RelayFrame::Nudge { seq: 2, .. })
        });
        assert!(matches!(transcript, RelayFrame::Transcript { seq: 2, .. }));
        assert!(matches!(replayed_nudge, RelayFrame::Nudge { seq: 2, .. }));
        assert_eq!(second.cursor().transcript_seq, 2);
        assert_eq!(second.cursor().nudge_seq, 2);
    }

    #[test]
    fn repeated_reconnects_replay_each_cursor_advance() {
        const RECONNECT_CYCLES: u64 = 40;

        let dir = TempDir::new().unwrap();
        let server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let discovery_path = capture_relay_discovery_path_in(dir.path());
        let mut cursor = RelayCursor::default();

        for seq in 1..=RECONNECT_CYCLES {
            server.publish_transcript_for_test(utterance(&format!("transcript-{seq}")));
            server.publish_nudge(nudge(&format!("nudge-{seq}")));

            let mut client = CaptureRelayClient::connect_from(&discovery_path, cursor).unwrap();
            wait_for_frame(
                &mut client,
                |frame| matches!(frame, RelayFrame::Transcript { seq: frame_seq, .. } if *frame_seq == seq),
            );
            wait_for_frame(
                &mut client,
                |frame| matches!(frame, RelayFrame::Nudge { seq: frame_seq, .. } if *frame_seq == seq),
            );

            cursor = client.cursor();
            assert_eq!(cursor.transcript_seq, seq);
            assert_eq!(cursor.nudge_seq, seq);
            drop(client);
        }
    }

    #[test]
    fn publishes_nudges_from_an_attached_process() {
        let dir = TempDir::new().unwrap();
        let _server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let discovery_path = capture_relay_discovery_path_in(dir.path());
        let mut observer =
            CaptureRelayClient::connect_from(&discovery_path, RelayCursor::default()).unwrap();

        assert_eq!(
            CaptureRelayClient::publish_nudge_via(&discovery_path, nudge("remote")).unwrap(),
            1
        );
        let frame = wait_for_frame(&mut observer, |frame| {
            matches!(frame, RelayFrame::Nudge { seq: 1, .. })
        });
        assert!(matches!(frame, RelayFrame::Nudge { seq: 1, .. }));
    }

    #[test]
    fn heartbeat_and_explicit_shutdown_are_observable() {
        let dir = TempDir::new().unwrap();
        let server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let discovery_path = capture_relay_discovery_path_in(dir.path());
        let mut client =
            CaptureRelayClient::connect_from(&discovery_path, RelayCursor::default()).unwrap();

        let heartbeat = wait_for_frame(&mut client, |frame| {
            matches!(frame, RelayFrame::Heartbeat { .. })
        });
        assert!(matches!(heartbeat, RelayFrame::Heartbeat { .. }));
        assert!(client.established);

        server.shutdown("test owner stopped");
        let shutdown = wait_for_frame(&mut client, |frame| {
            matches!(frame, RelayFrame::Shutdown { .. })
        });
        assert_eq!(
            shutdown,
            RelayFrame::Shutdown {
                reason: "test owner stopped".into()
            }
        );
        assert!(matches!(
            wait_for_error(&mut client),
            CaptureRelayError::Io(error) if error.kind() == io::ErrorKind::UnexpectedEof
        ));
    }

    #[test]
    fn active_capture_without_relay_never_starts_a_second_capture() {
        let plan = plan_capture_attachment(true, false, true);
        let CaptureAttachPlan::RefuseDuplicate { message } = plan else {
            panic!("existing capture without relay must be refused");
        };
        assert!(message.contains("did not open a second microphone"));
    }

    #[test]
    fn second_relay_owner_is_refused_until_the_first_shuts_down() {
        let dir = TempDir::new().unwrap();
        let first = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let second = CaptureRelayServer::start_for_test(dir.path());
        assert!(matches!(
            second,
            Err(CaptureRelayError::AlreadyOwned(owner_pid))
                if owner_pid == std::process::id()
        ));

        first.shutdown("first owner stopped");
        let replacement = CaptureRelayServer::start_for_test(dir.path());
        assert!(replacement.is_ok());
    }

    #[test]
    fn contended_owner_lock_uses_live_discovery_for_error_classification() {
        let dir = TempDir::new().unwrap();
        let _first = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let discovery_path = capture_relay_discovery_path_in(dir.path());

        let second_lock = acquire_owner_lock(dir.path(), &discovery_path);
        assert!(matches!(
            second_lock,
            Err(CaptureRelayError::AlreadyOwned(owner_pid))
                if owner_pid == std::process::id()
        ));
    }

    #[test]
    fn stale_discovery_is_reclaimed_when_the_owner_lock_is_available() {
        let dir = TempDir::new().unwrap();
        let discovery_path = capture_relay_discovery_path_in(dir.path());
        let stale_at = Utc::now() - chrono::Duration::seconds(10);
        let stale_discovery = CaptureRelayDiscovery {
            v: RELAY_PROTOCOL_VERSION,
            session_id: "stale-session".into(),
            transport: current_transport(),
            endpoint: relay_endpoint(dir.path(), "stale-session"),
            owner_pid: std::process::id(),
            evidence_mode: CopilotEvidenceMode::InProcessPartials,
            auth_token: "stale-token".into(),
            started_at: stale_at,
            heartbeat_at: stale_at,
        };
        write_discovery(&discovery_path, &stale_discovery).unwrap();

        let replacement = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        assert_ne!(
            replacement.discovery().session_id,
            stale_discovery.session_id
        );
        assert_eq!(replacement.discovery().owner_pid, std::process::id());
    }

    #[test]
    fn explicit_capture_is_only_allowed_when_no_owner_exists() {
        assert_eq!(
            plan_capture_attachment(false, false, true),
            CaptureAttachPlan::StartExplicitCapture
        );
        assert_eq!(
            plan_capture_attachment(false, false, false),
            CaptureAttachPlan::WaitForCapture
        );
        assert_eq!(
            plan_capture_attachment(true, true, true),
            CaptureAttachPlan::Attach
        );
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_and_discovery_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        let socket_mode = std::fs::metadata(&server.discovery.endpoint)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let discovery_mode = std::fs::metadata(capture_relay_discovery_path_in(dir.path()))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(socket_mode, 0o600);
        assert_eq!(discovery_mode, 0o600);
    }

    #[test]
    fn discovery_uses_platform_transport_contract() {
        let dir = TempDir::new().unwrap();
        let server = CaptureRelayServer::start_for_test(dir.path()).unwrap();
        #[cfg(windows)]
        {
            assert_eq!(server.discovery.transport, RelayTransport::WindowsNamedPipe);
            assert!(server.discovery.endpoint.starts_with(r"\\.\pipe\"));
        }
        #[cfg(unix)]
        {
            assert_eq!(server.discovery.transport, RelayTransport::UnixSocket);
            assert!(server.discovery.endpoint.ends_with(UNIX_SOCKET_FILE));
        }
    }
}
