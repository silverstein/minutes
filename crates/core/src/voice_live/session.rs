//! The session runner: mic in, socket, playback, tools, transcript log.
//!
//! One session thread multiplexes three channels: server events from the IO
//! thread, mic chunks from `AudioStream`, and control messages from the host.
//! Tool calls run on a single worker thread so corpus reads never overlap (the
//! active-corpus budget guard rejects concurrent readers) and never block audio.

use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use chrono::Local;
use crossbeam_channel::{bounded, select, Receiver, Sender};
use serde::Serialize;

use crate::config::Config;
use crate::streaming::{AudioChunk, AudioStream};

use super::audio_out::Playback;
use super::names::NameIndex;
use super::protocol::{FunctionCall, LiveClient, ServerEvent, SessionSetup};
use super::tools::ToolContext;
#[cfg(target_os = "macos")]
use super::voice_io::VoiceIo;
use super::work_runtime::{Review, WorkRuntime};
use super::{system_prompt, VoiceLiveError};
use crate::live_sidekick::live_model::{LiveActivity, LiveModel, ModelActivity};
use crate::live_sidekick::work::AuthorizedAction;

/// How the user talks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TalkMode {
    /// Hold to talk. We send activity markers; barge-in happens on press.
    PushToTalk,
    /// Always listening with the provider's voice activity detection; barge-in by speaking.
    OpenMic,
}

/// Coarse session state for a HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VoiceLiveState {
    Connecting,
    Ready,
    Listening,
    Thinking,
    Speaking,
    Closed,
}

/// Events delivered to the host.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceLiveEvent {
    State {
        state: VoiceLiveState,
    },
    Status {
        text: String,
    },
    UserTranscript {
        text: String,
        partial: bool,
    },
    AssistantTranscript {
        text: String,
        partial: bool,
    },
    ToolCall {
        name: String,
        args: serde_json::Value,
    },
    ToolResult {
        name: String,
        ms: u128,
        chars: usize,
        error: bool,
    },
    ApprovalRequired {
        review: Review,
    },
    Level {
        rms: f32,
    },
    Closed {
        reason: String,
    },
}

/// Options for one session.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    pub mode: TalkMode,
    /// Input device override; `None` uses `config.recording.device`.
    pub device: Option<String>,
    /// Skip speaker playback (headless tests).
    pub mute_playback: bool,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            mode: TalkMode::PushToTalk,
            device: None,
            mute_playback: false,
        }
    }
}

/// The microphone and speaker for one session.
enum AudioIo {
    /// Separate capture and playback streams. No echo cancellation: on speakers
    /// the microphone hears the assistant, so open mic is only reliable on
    /// headphones.
    Split {
        mic: AudioStream,
        playback: Option<Playback>,
    },
    /// One voice-processing unit doing both, with the speaker signal cancelled
    /// out of the microphone.
    #[cfg(target_os = "macos")]
    Processed(VoiceIo),
}

impl AudioIo {
    fn receiver(&self) -> &Receiver<AudioChunk> {
        match self {
            AudioIo::Split { mic, .. } => &mic.receiver,
            #[cfg(target_os = "macos")]
            AudioIo::Processed(io) => &io.receiver,
        }
    }

    fn push_pcm16(&mut self, bytes: &[u8]) {
        match self {
            AudioIo::Split { playback, .. } => {
                if let Some(p) = playback.as_mut() {
                    p.push_pcm16(bytes);
                }
            }
            #[cfg(target_os = "macos")]
            AudioIo::Processed(io) => io.push_pcm16(bytes),
        }
    }

    fn flush(&mut self) {
        match self {
            AudioIo::Split { playback, .. } => {
                if let Some(p) = playback.as_mut() {
                    p.flush();
                }
            }
            #[cfg(target_os = "macos")]
            AudioIo::Processed(io) => io.flush(),
        }
    }

    /// True when the microphone cannot hear the speaker.
    ///
    /// Matters for more than comfort: without cancellation the assistant's own
    /// voice is transcribed as user input, and anything treating that as a
    /// person speaking is trusting the model's echo.
    fn cancels_echo(&self) -> bool {
        match self {
            AudioIo::Split { .. } => false,
            #[cfg(target_os = "macos")]
            AudioIo::Processed(_) => true,
        }
    }

    /// True when nothing is queued for the speaker (or there is no speaker).
    fn is_idle(&self) -> bool {
        match self {
            AudioIo::Split { playback, .. } => {
                playback.as_ref().map(|p| p.is_idle()).unwrap_or(true)
            }
            #[cfg(target_os = "macos")]
            AudioIo::Processed(io) => io.is_idle(),
        }
    }

    fn stop(&mut self) {
        match self {
            AudioIo::Split { mic, .. } => mic.stop(),
            #[cfg(target_os = "macos")]
            AudioIo::Processed(io) => io.stop(),
        }
    }
}

/// Open audio for a session. Prefers the echo-cancelled unit when playback is
/// on and the platform has one; otherwise separate streams, with a status line
/// saying why so a self-interrupting session is explainable from the log.
fn open_audio(
    config: &Config,
    options: &SessionOptions,
    device: Option<&str>,
    emit: &dyn Fn(VoiceLiveEvent),
) -> Result<AudioIo, VoiceLiveError> {
    let want_cancellation = !options.mute_playback && config.voice_live.echo_cancellation;
    #[cfg(target_os = "macos")]
    if want_cancellation {
        match VoiceIo::start() {
            Ok(io) => {
                if let Some(d) = device {
                    emit(VoiceLiveEvent::Status {
                        text: format!(
                            "device override {d:?} ignored: echo cancellation follows the system default input and output"
                        ),
                    });
                }
                emit(VoiceLiveEvent::Status {
                    text: format!(
                        "mic: {} (echo cancellation on), speaker: {}",
                        io.input_name, io.output_name
                    ),
                });
                return Ok(AudioIo::Processed(io));
            }
            Err(e) => emit(VoiceLiveEvent::Status {
                text: format!(
                    "echo cancellation unavailable ({e}); using plain capture, expect self-interruption on speakers"
                ),
            }),
        }
    }
    #[cfg(not(target_os = "macos"))]
    if want_cancellation {
        emit(VoiceLiveEvent::Status {
            text: "echo cancellation is macOS-only for now; using plain capture, use headphones on open mic"
                .into(),
        });
    }

    let mic = AudioStream::start(device).map_err(|e| VoiceLiveError::Audio(e.to_string()))?;
    emit(VoiceLiveEvent::Status {
        text: format!("mic: {}", mic.device_name),
    });
    let playback = if options.mute_playback {
        None
    } else {
        match Playback::open() {
            Ok(p) => {
                emit(VoiceLiveEvent::Status {
                    text: format!("speaker: {}", p.device_name),
                });
                Some(p)
            }
            Err(e) => {
                emit(VoiceLiveEvent::Status {
                    text: format!("no speaker playback: {e}"),
                });
                None
            }
        }
    };
    Ok(AudioIo::Split { mic, playback })
}

/// Ceiling on reconnects in one session, so a provider that closes instantly
/// cannot become a reconnect loop.
const MAX_RESUMES: u32 = 12;

/// How often to report that a slow tool is still going.
const TOOL_PROGRESS_EVERY: Duration = Duration::from_secs(8);

/// Sent with a screen frame, as the user turn the model answers.
const SCREEN_CAPTION: &str = "This is my screen at this exact moment, captured for the look_at_screen you just ran. Answer my question from this image and nothing else. If I dispute what you report, look at the image again and tell me what is actually there, even if that means disagreeing with me.";

enum Control {
    PttStart,
    PttEnd,
    Text(String),
    Approve(Review),
    Reject(u64),
    CancelTools,
    Stop,
}

/// Handle to a running session. Dropping it stops the session.
pub struct VoiceLiveSession {
    control: Sender<Control>,
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    pub log_path: Option<PathBuf>,
    pub work: Arc<WorkRuntime>,
}

impl VoiceLiveSession {
    /// Host-only gestures; never model tools.
    pub fn approve_local(&self, review: Review) {
        let _ = self.control.try_send(Control::Approve(review));
    }
    pub fn reject_local(&self, id: u64) {
        let _ = self.control.try_send(Control::Reject(id));
    }
    pub fn cancel_tools(&self) {
        self.work.cancel_all();
        let _ = self.control.try_send(Control::CancelTools);
    }

    /// Start talking (push-to-talk mode).
    pub fn ptt_start(&self) {
        let _ = self.control.try_send(Control::PttStart);
    }

    /// Stop talking (push-to-talk mode).
    pub fn ptt_end(&self) {
        let _ = self.control.try_send(Control::PttEnd);
    }

    /// Send a typed turn.
    pub fn send_text(&self, text: &str) {
        if text.is_empty() || text.len() > 40_000 {
            return;
        }
        let _ = self.control.try_send(Control::Text(text.to_string()));
    }

    /// Signal shutdown without waiting; safe for the app's capture-critical exit path.
    pub fn request_stop(&self) {
        self.work.cancel_all();
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.control.try_send(Control::Stop);
    }

    /// Stop and wait for the session thread.
    pub fn stop(mut self) {
        self.work.cancel_all();
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.control.try_send(Control::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }

    /// True while the session thread is alive.
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for VoiceLiveSession {
    fn drop(&mut self) {
        self.work.cancel_all();
        self.stop_flag.store(true, Ordering::SeqCst);
        let _ = self.control.try_send(Control::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Start a session. Returns once the socket is open and the mic is streaming;
/// `setupComplete` arrives as a `Ready` state event.
pub fn start<F>(
    config: &Config,
    options: SessionOptions,
    on_event: F,
) -> Result<VoiceLiveSession, VoiceLiveError>
where
    F: Fn(VoiceLiveEvent) + Send + Sync + 'static,
{
    let work = WorkRuntime::new("Conversation with Minutes").map_err(VoiceLiveError::Connect)?;
    start_with_work(config, options, work, on_event)
}

pub fn start_with_work<F>(
    config: &Config,
    options: SessionOptions,
    work: Arc<WorkRuntime>,
    on_event: F,
) -> Result<VoiceLiveSession, VoiceLiveError>
where
    F: Fn(VoiceLiveEvent) + Send + Sync + 'static,
{
    start_with_work_cancellable(
        config,
        options,
        work,
        Arc::new(AtomicBool::new(false)),
        on_event,
    )
}

pub fn start_with_work_cancellable<F>(
    config: &Config,
    options: SessionOptions,
    work: Arc<WorkRuntime>,
    stop_flag: Arc<AtomicBool>,
    on_event: F,
) -> Result<VoiceLiveSession, VoiceLiveError>
where
    F: Fn(VoiceLiveEvent) + Send + Sync + 'static,
{
    if stop_flag.load(Ordering::SeqCst) {
        return Err(VoiceLiveError::Closed);
    }
    super::preflight(config)?;
    let api_key = super::api_key(config)?;
    super::refuse_if_recording()?;
    let on_event: Arc<dyn Fn(VoiceLiveEvent) + Send + Sync> = Arc::new(on_event);
    let emit = |e: VoiceLiveEvent| on_event(e);
    emit(VoiceLiveEvent::State {
        state: VoiceLiveState::Connecting,
    });

    let names = Arc::new(NameIndex::load(config, config.voice_live.known_people));
    let tools = Arc::new(ToolContext::new_with_work(
        config.clone(),
        Arc::clone(&names),
        Arc::clone(&work),
    ));
    for problem in &tools.mcp_problems {
        emit(VoiceLiveEvent::Status {
            text: format!("mcp server unavailable, {problem}"),
        });
    }
    let declarations = tools.declarations();
    let prompt = format!(
        "{}\n{}",
        system_prompt(config, &names, tools.brain_root.is_some()),
        work.context().map_err(VoiceLiveError::Connect)?
    );
    emit(VoiceLiveEvent::Status {
        text: format!(
            "{} known people, {} tools{}",
            names.people.len(),
            declarations.len(),
            if tools.brain_root.is_some() {
                ", brain on"
            } else {
                ""
            }
        ),
    });

    let setup = SessionSetup {
        model: config.voice_live.model.clone(),
        api_key,
        system_instruction: prompt,
        function_declarations: declarations,
        language: config.voice_live.language.clone(),
        manual_activity: options.mode == TalkMode::PushToTalk,
        proactive_audio: config.voice_live.proactive_audio,
        start_sensitivity: config.voice_live.speech_start_sensitivity.clone(),
        end_sensitivity: config.voice_live.speech_end_sensitivity.clone(),
        resume_handle: None,
    };
    if stop_flag.load(Ordering::SeqCst) {
        return Err(VoiceLiveError::Closed);
    }
    let connected = LiveClient::connect(&setup)?;
    let inbox = connected.inbox.clone();
    let client = Arc::new(Mutex::new(Arc::new(connected)));

    let device = options
        .device
        .clone()
        .or_else(|| config.recording.device.clone());
    let preflight = crate::capture::preflight_microphone_only();
    if let Some(reason) = preflight.blocking_reason {
        return Err(VoiceLiveError::Audio(reason));
    }
    if stop_flag.load(Ordering::SeqCst) {
        return Err(VoiceLiveError::Closed);
    }
    super::refuse_if_recording()?;
    let audio = open_audio(config, &options, device.as_deref(), &emit)?;

    let log = if config.voice_live.log_sessions {
        SessionLog::open(&config.voice_live.model, options.mode).ok()
    } else {
        None
    };
    let log_path = log.as_ref().map(|l| l.path.clone());
    if let Some(p) = &log_path {
        emit(VoiceLiveEvent::Status {
            text: format!("log: {}", p.display()),
        });
    }

    let (control_tx, control_rx) = bounded::<Control>(128);
    let scheduling = match config
        .voice_live
        .tool_scheduling
        .to_ascii_lowercase()
        .as_str()
    {
        "interrupt" => "INTERRUPT",
        "silent" => "SILENT",
        _ => "WHEN_IDLE",
    }
    .to_string();

    let runner = Runner {
        client,
        inbox,
        setup,
        audio,
        tools,
        control_rx,
        on_event: Arc::clone(&on_event),
        log,
        mode: options.mode,
        scheduling,
        stop_flag: Arc::clone(&stop_flag),
    };
    let thread = std::thread::Builder::new()
        .name("voice-live-session".into())
        .spawn(move || runner.run())
        .map_err(|e| VoiceLiveError::Connect(format!("session thread: {e}")))?;

    Ok(VoiceLiveSession {
        control: control_tx,
        stop_flag,
        thread: Some(thread),
        log_path,
        work,
    })
}

struct QueuedCall {
    call: FunctionCall,
    cancel: Arc<AtomicBool>,
    authority: Option<AuthorizedAction>,
}

struct Runner {
    /// Swappable, because resuming replaces the socket underneath a session
    /// that is still running. The tool worker reads the current one per call.
    client: Arc<Mutex<Arc<LiveClient>>>,
    inbox: Receiver<ServerEvent>,
    /// Kept so a resumed session can be opened with the same instructions.
    setup: SessionSetup,
    audio: AudioIo,
    tools: Arc<ToolContext>,
    control_rx: Receiver<Control>,
    on_event: Arc<dyn Fn(VoiceLiveEvent) + Send + Sync>,
    log: Option<SessionLog>,
    mode: TalkMode,
    scheduling: String,
    stop_flag: Arc<AtomicBool>,
}

/// Chunks are 100 ms, so this is 300 ms. Below it a transcriber invents a word
/// out of a click or a breath.
const PTT_MIN_CHUNKS: usize = 3;

struct PttState {
    held: bool,
    started: bool,
    chunks: usize,
    /// Audio captured before the press was long enough to count. Held here
    /// rather than streamed, so a press that turns out to be too short really
    /// did send nothing, which is what the host tells the user.
    held_back: Vec<Vec<u8>>,
}

impl Runner {
    fn emit(&self, e: VoiceLiveEvent) {
        (self.on_event)(e);
    }

    /// The socket in use right now.
    fn client(&self) -> Arc<LiveClient> {
        Arc::clone(&self.client.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Open a fresh socket that continues this conversation.
    ///
    /// Returns the new inbox on success. The old client is dropped, which ends
    /// its reader thread; the tool worker picks the replacement up on its next
    /// call because it reads through the same cell.
    fn resume(&self, handle: &str) -> Option<Receiver<ServerEvent>> {
        let mut setup = self.setup.clone();
        setup.resume_handle = Some(handle.to_string());
        match LiveClient::connect(&setup) {
            Ok(fresh) => {
                let inbox = fresh.inbox.clone();
                let mut slot = self.client.lock().unwrap_or_else(|p| p.into_inner());
                let previous = std::mem::replace(&mut *slot, Arc::new(fresh));
                drop(slot);
                if let Ok(previous) = Arc::try_unwrap(previous) {
                    previous.close();
                }
                Some(inbox)
            }
            Err(e) => {
                self.emit(VoiceLiveEvent::Status {
                    text: format!("could not resume the session: {e}"),
                });
                None
            }
        }
    }

    fn run(mut self) {
        let mut inbox = self.inbox.clone();
        let mut resume_handle: Option<String> = None;
        let mut resumes = 0u32;
        // Tool worker: one at a time, results go straight back to the socket.
        let (tool_tx, tool_rx) = bounded::<QueuedCall>(64);
        let (agent_tx, agent_rx) = bounded::<QueuedCall>(2);
        let (audio_out, audio_in) = bounded::<Vec<u8>>(2);
        let make_worker = |tool_rx: Receiver<QueuedCall>| {
            let tools = Arc::clone(&self.tools);
            let client_cell = Arc::clone(&self.client);
            let on_event = Arc::clone(&self.on_event);
            let scheduling = self.scheduling.clone();
            let settle =
                Duration::from_millis(self.tools.config.voice_live.screen_settle_ms.min(3_000));
            let log_tx = self.log.as_ref().map(|l| l.sender());
            let audio_out = audio_out.clone();
            let stop_flag = Arc::clone(&self.stop_flag);
            std::thread::Builder::new()
                .name("voice-live-tools".into())
                .spawn(move || {
                    for queued in tool_rx.iter() {
                        let call = queued.call;
                        // Shutdown drops the sender, but whatever was already
                        // queued still arrives here. A send waiting behind a
                        // slow tool must not fire after the session is over.
                        if stop_flag.load(Ordering::SeqCst) || queued.cancel.load(Ordering::SeqCst)
                        {
                            tools.work.finish(&call.id);
                            if !stop_flag.load(Ordering::SeqCst) {
                                let client = Arc::clone(
                                    &client_cell.lock().unwrap_or_else(|p| p.into_inner()),
                                );
                                let _ = client.send_tool_response(
                                    &call,
                                    "Cancelled before execution; nothing dispatched",
                                    "WHEN_IDLE",
                                );
                            }
                            continue;
                        }
                        // A relayed agent can take half a minute. Without this
                        // the host shows the call going out and then nothing,
                        // which is indistinguishable from a wedged session.
                        let running = Arc::new(AtomicBool::new(true));
                        {
                            let running = Arc::clone(&running);
                            let on_event = Arc::clone(&on_event);
                            let name = call.name.clone();
                            std::thread::spawn(move || {
                                let start = Instant::now();
                                let mut next = TOOL_PROGRESS_EVERY;
                                while running.load(Ordering::Relaxed) {
                                    std::thread::sleep(Duration::from_millis(250));
                                    if !running.load(Ordering::Relaxed) {
                                        break;
                                    }
                                    if start.elapsed() >= next {
                                        on_event(VoiceLiveEvent::Status {
                                            text: format!(
                                                "{name} still running, {}s",
                                                start.elapsed().as_secs()
                                            ),
                                        });
                                        next += TOOL_PROGRESS_EVERY;
                                    }
                                }
                            });
                        }
                        let outcome = tools.execute_call(&call, &queued.cancel, queued.authority);
                        if let Some(review) = outcome.review.clone() {
                            running.store(false, Ordering::Relaxed);
                            on_event(VoiceLiveEvent::ApprovalRequired { review });
                            continue;
                        }
                        tools.work.finish(&call.id);
                        running.store(false, Ordering::Relaxed);
                        on_event(VoiceLiveEvent::ToolResult {
                            name: call.name.clone(),
                            ms: outcome.elapsed.as_millis(),
                            chars: outcome.text.chars().count(),
                            error: outcome.is_error,
                        });
                        if let Some(tx) = &log_tx {
                            let _ = tx.try_send(format!(
                                "  -> {} {} chars in {} ms",
                                if outcome.is_error { "error" } else { "ok" },
                                outcome.text.chars().count(),
                                outcome.elapsed.as_millis()
                            ));
                        }
                        // Read the socket only now, never before the tool ran.
                        // A tool can take a minute, and the session may have
                        // resumed onto a new socket while it did. Answering the
                        // old one loses the result and breaks this loop, which
                        // silently kills every later tool call in the session.
                        if stop_flag.load(Ordering::SeqCst) {
                            continue;
                        }
                        let client =
                            Arc::clone(&client_cell.lock().unwrap_or_else(|p| p.into_inner()));
                        // A frame answers as a turn, not as a tool result.
                        // Close the call silently so it produces no speech of
                        // its own, then send the frame as the turn the model
                        // actually answers.
                        if let Some(pcm) = &outcome.audio {
                            audio_out.try_send(pcm.clone()).ok();
                        }
                        let media = outcome.image.is_some();
                        let this_scheduling = if media { "SILENT" } else { &scheduling };
                        if client
                            .send_tool_response(&call, &outcome.text, this_scheduling)
                            .is_err()
                        {
                            // The socket went while this ran. Losing one result
                            // is bad; ending the worker loses every tool call
                            // for the rest of the session, including after a
                            // successful resume.
                            on_event(VoiceLiveEvent::Status {
                                text: format!("{} finished after the socket closed", call.name),
                            });
                            continue;
                        }
                        if let Some(image) = &outcome.image {
                            if settle > Duration::ZERO {
                                std::thread::sleep(settle);
                            }
                            if client
                                .send_image(image, "image/png", SCREEN_CAPTION)
                                .is_err()
                            {
                                // Same reasoning as the tool response above:
                                // losing one frame is bad, losing every later
                                // tool call for the session is worse.
                                on_event(VoiceLiveEvent::Status {
                                    text: "the screen frame arrived after the socket closed".into(),
                                });
                                continue;
                            }
                        }
                    }
                })
                .ok()
        };

        let worker = make_worker(tool_rx);
        let agent_worker = make_worker(agent_rx);
        if worker.is_none() || agent_worker.is_none() {
            self.emit(VoiceLiveEvent::Closed {
                reason: "Unable to start tool supervisors".into(),
            });
            self.stop_flag.store(true, Ordering::SeqCst);
        }
        let mut activity =
            LiveActivity::new(LiveModel::from_id(&self.setup.model).unwrap_or(LiveModel::Standard));
        let mut state = VoiceLiveState::Connecting;
        let mut ptt = PttState {
            held: false,
            started: false,
            chunks: 0,
            held_back: Vec::new(),
        };
        let mut you = String::new();
        let mut me = String::new();
        let mut last_audio = Instant::now();
        let set_state = |runner: &Runner, current: &mut VoiceLiveState, next: VoiceLiveState| {
            if *current != next {
                *current = next;
                runner.emit(VoiceLiveEvent::State { state: next });
            }
        };

        loop {
            if self.stop_flag.load(Ordering::SeqCst) {
                break;
            }
            if super::refuse_if_recording().is_err() {
                self.emit(VoiceLiveEvent::Closed {
                    reason: "Another capture started; voice stopped without changing it".into(),
                });
                break;
            }
            activity.audio_playing = !self.audio.is_idle();
            activity.outstanding_tasks = self.tools.work.pending_count();
            for call in self.tools.work.expire_reviews() {
                let _ = self.client().send_tool_response(
                    &call,
                    "Local approval expired; nothing executed.",
                    "WHEN_IDLE",
                );
            }
            // Playback draining is not proof that asynchronous reasoning ended.
            if state == VoiceLiveState::Speaking {
                let idle = self.audio.is_idle();
                if idle && last_audio.elapsed() > Duration::from_millis(300) {
                    set_state(
                        &self,
                        &mut state,
                        if activity.ready() {
                            VoiceLiveState::Ready
                        } else {
                            VoiceLiveState::Thinking
                        },
                    );
                }
            }
            if state == VoiceLiveState::Thinking && activity.ready() {
                set_state(&self, &mut state, VoiceLiveState::Ready);
            }
            select! {
                recv(inbox) -> msg => {
                    let Ok(ev) = msg else { self.emit(VoiceLiveEvent::Closed { reason: "io thread ended".into() }); break; };
                    match ev {
                        ServerEvent::SetupComplete => { activity.model_activity = ModelActivity::Idle; set_state(&self, &mut state, VoiceLiveState::Ready); },
                        ServerEvent::InteractionStatus(value) => activity.observe(&value),
                        ServerEvent::Audio(bytes) => {
                            activity.model_activity = ModelActivity::InProgress;
                            self.audio.push_pcm16(&bytes);
                            last_audio = Instant::now();
                            set_state(&self, &mut state, VoiceLiveState::Speaking);
                        }
                        ServerEvent::InputTranscript(t) => {
                            // The user's voice, as it arrives. The confirmation
                            // gate needs to know a person spoke and when, and a
                            // flush at a turn boundary is too late and can
                            // replay the very request that asked the question.
                            //
                            // Only count it as a person when it cannot be the
                            // assistant hearing itself. With cancellation the
                            // microphone never carries the speaker, so speaking
                            // over it is genuinely the user. Without it, a
                            // transcript arriving while audio is still playing
                            // is most likely the echo of the very sentence that
                            // asked for confirmation.
                            if self.audio.cancels_echo() || self.audio.is_idle() {
                                self.tools.desktop.heard_user(&t);
                            }
                            if you.len().saturating_add(t.len()) > 64 * 1024 { self.emit(VoiceLiveEvent::Closed { reason: "Transcript turn exceeded budget".into() }); break; }
                            you.push_str(&t);
                            self.emit(VoiceLiveEvent::UserTranscript { text: t, partial: true });
                        }
                        ServerEvent::OutputTranscript(t) | ServerEvent::Text(t) => {
                            if me.len().saturating_add(t.len()) > 64 * 1024 { self.emit(VoiceLiveEvent::Closed { reason: "Transcript turn exceeded budget".into() }); break; }
                            me.push_str(&t);
                            self.emit(VoiceLiveEvent::AssistantTranscript { text: t, partial: true });
                        }
                        ServerEvent::Interrupted => {
                            activity.user_turn_started();
                            self.audio.flush();
                            self.flush_transcripts(&mut you, &mut me);
                            set_state(&self, &mut state, VoiceLiveState::Thinking);
                        }
                        ServerEvent::TurnComplete => {
                            // The end of the assistant's turn is the earliest
                            // point an answer to its question can exist.
                            self.tools.desktop.finished_speaking();
                            self.flush_transcripts(&mut you, &mut me);
                            activity.observe(&serde_json::json!({"serverContent":{"turnComplete":true}}));
                            activity.audio_playing = !self.audio.is_idle();
                            activity.outstanding_tasks = self.tools.work.pending_count();
                            if activity.ready() { set_state(&self, &mut state, VoiceLiveState::Ready); }
                        }
                        ServerEvent::ToolCall(calls) => {
                            set_state(&self, &mut state, VoiceLiveState::Thinking);
                            for call in calls {
                                self.emit(VoiceLiveEvent::ToolCall { name: call.name.clone(), args: call.args.clone() });
                                self.log_line(format!("`{}({})`", call.name, call.args));
                                activity.user_turn_started();
                                match self.tools.work.register(&call.id) {
                                    Ok(cancel) => {
                                        let queued = QueuedCall { call: call.clone(), cancel, authority: None };
                                        if tool_tx.try_send(queued).is_err() {
                                            self.tools.work.finish(&call.id);
                                            let _ = self.client().send_tool_response(&call, "Tool queue full; nothing executed", "WHEN_IDLE");
                                        }
                                    }
                                    Err(error) => { let _ = self.client().send_tool_response(&call, &error, "WHEN_IDLE"); }
                                }
                            }
                        }
                        ServerEvent::ToolCallCancellation(ids) => {
                            self.tools.work.cancel(&ids);
                            self.emit(VoiceLiveEvent::Status { text: format!("cancellation requested: {}. An action already in progress may still complete.", ids.join(",")) });
                        }
                        ServerEvent::GoAway(t) => self.emit(VoiceLiveEvent::Status { text: format!("server ending session in {t}") }),
                        ServerEvent::ResumptionHandle(h) => resume_handle = Some(h),
                        ServerEvent::Error(e) => self.emit(VoiceLiveEvent::Status { text: format!("provider: {e}") }),
                        ServerEvent::Other(keys) => self.emit(VoiceLiveEvent::Status { text: format!("unhandled server message: {}", keys.join(",")) }),
                        ServerEvent::Closed(reason) => {
                            activity.disconnected();
                            self.flush_transcripts(&mut you, &mut me);
                            // A Live session is capped at around fifteen
                            // minutes. The provider hands out a handle before
                            // it goes, so carry the conversation to a new
                            // socket rather than losing it mid-sentence.
                            let wanted = self.tools.config.voice_live.resume_sessions
                                && !self.stop_flag.load(Ordering::SeqCst)
                                && resumes < MAX_RESUMES;
                            if let (true, Some(handle)) = (wanted, resume_handle.clone()) {
                                if let Some(fresh) = self.resume(&handle) {
                                    inbox = fresh;
                                    resumes += 1;
                                    set_state(&self, &mut state, VoiceLiveState::Connecting);
                                    self.emit(VoiceLiveEvent::Status {
                                        text: format!("session resumed after {reason}"),
                                    });
                                    continue;
                                }
                            }
                            self.emit(VoiceLiveEvent::Closed { reason });
                            break;
                        }
                    }
                }
                recv(self.audio.receiver()) -> chunk => {
                    let Ok(chunk) = chunk else { self.emit(VoiceLiveEvent::Closed { reason: "microphone stream ended".into() }); break; };
                    self.emit(VoiceLiveEvent::Level { rms: chunk.rms });
                    let pcm: Vec<u8> = chunk.samples.iter().flat_map(|s| ((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes()).collect();
                    let send = match self.mode {
                        TalkMode::OpenMic => true,
                        TalkMode::PushToTalk => {
                            if !ptt.held {
                                false
                            } else {
                                ptt.chunks += 1;
                                if ptt.chunks < PTT_MIN_CHUNKS {
                                    // Not yet long enough to be speech. Keep it
                                    // rather than sending it, so a short press
                                    // can be discarded rather than retracted.
                                    ptt.held_back.push(pcm.clone());
                                    false
                                } else {
                                    if !ptt.started {
                                        ptt.started = true;
                                        let _ = self.client().activity_start();
                                        let mut failed = false;
                                        for earlier in ptt.held_back.drain(..) {
                                            if self.client().send_audio(&earlier).is_err() {
                                                failed = true;
                                                break;
                                            }
                                        }
                                        if failed {
                                            break;
                                        }
                                    }
                                    true
                                }
                            }
                        }
                    };
                    if send {
                        if self.client().send_audio(&pcm).is_err() { break; }
                        if self.mode == TalkMode::OpenMic && chunk.rms > 0.02 && state == VoiceLiveState::Ready {
                            set_state(&self, &mut state, VoiceLiveState::Listening);
                        }
                    }
                }
                // Audio a tool produced, queued for the speaker on this thread
                // because the playback handle lives here.
                recv(audio_in) -> pcm => {
                    if let Ok(pcm) = pcm {
                        // Checked again here, not only where it was generated:
                        // a piece can wait in this queue while a recording
                        // starts, and music in the transcript is the one thing
                        // this must never do.
                        if crate::pid::status().recording {
                            self.emit(VoiceLiveEvent::Status {
                                text: "a recording started, so the music is saved but not played".into(),
                            });
                        } else {
                            self.audio.push_pcm16(&pcm);
                        }
                    }
                }
                recv(self.control_rx) -> ctl => {
                    match ctl {
                        Ok(Control::PttStart) if self.mode == TalkMode::PushToTalk => {
                            activity.user_turn_started();
                            ptt = PttState { held: true, started: false, chunks: 0, held_back: Vec::new() };
                            self.audio.flush();
                            set_state(&self, &mut state, VoiceLiveState::Listening);
                        }
                        Ok(Control::PttEnd) if self.mode == TalkMode::PushToTalk => {
                            let PttState { started, chunks, .. } = ptt;
                            ptt = PttState { held: false, started: false, chunks: 0, held_back: Vec::new() };
                            // Chunks are 100 ms; require 300 ms of audio or the transcriber invents a word.
                            if started && chunks >= PTT_MIN_CHUNKS {
                                let _ = self.client().activity_end();
                                set_state(&self, &mut state, VoiceLiveState::Thinking);
                            } else {
                                // The audio only leaves once the press is long
                                // enough, so there is nothing to retract here.
                                // Closing the turn anyway would hand the model
                                // exactly the fragment this guard exists to
                                // withhold, while the host said nothing was sent.
                                self.emit(VoiceLiveEvent::Status { text: "press was too short, nothing sent".into() });
                                set_state(&self, &mut state, VoiceLiveState::Ready);
                            }
                        }
                        Ok(Control::Approve(review)) => {
                            match self.tools.work.approve_from_host(&review) {
                                Ok((call, authority)) => {
                                    if let Some(cancel) = self.tools.work.cancel_flag(&call.id) {
                                        if (if call.name == "ask_agent" { &agent_tx } else { &tool_tx }).try_send(QueuedCall { call: call.clone(), cancel, authority: Some(authority) }).is_err() {
                                            self.tools.work.finish(&call.id);
                                            let _ = self.client().send_tool_response(&call, "Queue full; approved action not executed", "WHEN_IDLE");
                                        }
                                    }
                                }
                                Err(error) => self.emit(VoiceLiveEvent::Status { text: error }),
                            }
                        }
                        Ok(Control::Reject(id)) => {
                            if let Ok(call) = self.tools.work.reject_from_host(id) {
                                self.tools.work.finish(&call.id);
                                let _ = self.client().send_tool_response(&call, "User rejected this action. Nothing executed. Do not ask again unless requested.", "WHEN_IDLE");
                            }
                        }
                        Ok(Control::CancelTools) => { self.tools.work.cancel_all(); self.audio.flush(); }
                        Ok(Control::Text(text)) => {
                            activity.user_turn_started();
                            // A typed line is the user as surely as a spoken
                            // one, and rather less ambiguously: nothing the
                            // assistant does can produce a keystroke. The
                            // confirmation gate counts it.
                            self.tools.desktop.heard_user(&text);
                            self.log_line(format!("**You (typed):** {text}"));
                            self.tools.work.observe_transcript("User (typed)", &text);
                            self.emit(VoiceLiveEvent::UserTranscript { text: text.clone(), partial: false });
                            if self.client().send_text_turn(&text).is_err() { break; }
                            set_state(&self, &mut state, VoiceLiveState::Thinking);
                        }
                        Ok(Control::Stop) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                default(Duration::from_millis(50)) => {}
            }
        }

        // However the loop ended, the session is over. Anything still queued
        // must not act afterwards, and only this flag tells the worker.
        self.stop_flag.store(true, Ordering::SeqCst);
        self.tools.work.cancel_all();
        self.flush_transcripts(&mut you, &mut me);
        set_state(&self, &mut state, VoiceLiveState::Closed);
        self.audio.stop();
        self.tools.mcp.shutdown();
        drop(tool_tx);
        drop(agent_tx);
        if let Some(w) = agent_worker {
            let _ = w.join();
        }
        if let Some(w) = worker {
            let _ = w.join();
        }
        let final_client = {
            let slot = self.client.lock().unwrap_or_else(|p| p.into_inner());
            Arc::clone(&slot)
        };
        if let Ok(client) = Arc::try_unwrap(final_client) {
            client.close();
        }
    }

    fn flush_transcripts(&self, you: &mut String, me: &mut String) {
        if !you.trim().is_empty() {
            self.tools
                .work
                .observe_transcript("User (speech recognition, unverified)", you.trim());
            self.emit(VoiceLiveEvent::UserTranscript {
                text: you.trim().to_string(),
                partial: false,
            });
            self.log_line(format!("**You:** {}", you.trim()));
        }
        if !me.trim().is_empty() {
            self.tools.work.observe_transcript("Assistant", me.trim());
            self.emit(VoiceLiveEvent::AssistantTranscript {
                text: me.trim().to_string(),
                partial: false,
            });
            self.log_line(format!("**Minutes:** {}", me.trim()));
        }
        you.clear();
        me.clear();
    }

    fn log_line(&self, line: String) {
        if let Some(l) = &self.log {
            l.write(&line);
        }
    }
}

/// Append-only markdown transcript of one session, `0600`.
struct SessionLog {
    path: PathBuf,
    tx: Sender<String>,
    _writer: JoinHandle<()>,
}

impl SessionLog {
    fn open(model: &str, mode: TalkMode) -> std::io::Result<Self> {
        let dir = super::sessions_dir();
        let root = crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&dir)?;
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).map_err(std::io::Error::other)?;
        let suffix: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let name = format!("{}-{suffix}.md", Local::now().format("%Y-%m-%d-%H-%M-%S"));
        let path = dir.join(&name);
        let mut file = root.create_new_exact_file(std::ffi::OsStr::new(&name))?;
        writeln!(
            file,
            "# Voice session {} ({model}, {mode:?})\n",
            Local::now().to_rfc3339()
        )?;
        let (tx, rx) = bounded::<String>(256);
        let writer = std::thread::Builder::new()
            .name("voice-live-log".into())
            .spawn(move || {
                for line in rx.iter() {
                    let _ = writeln!(file, "{line}\n");
                }
            })?;
        Ok(Self {
            path,
            tx,
            _writer: writer,
        })
    }

    fn sender(&self) -> Sender<String> {
        self.tx.clone()
    }

    fn write(&self, line: &str) {
        let _ = self.tx.try_send(line.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_a_type_tag() {
        let v = serde_json::to_value(VoiceLiveEvent::State {
            state: VoiceLiveState::Listening,
        })
        .unwrap();
        assert_eq!(v["type"], "state");
        assert_eq!(v["state"], "listening");
        let v = serde_json::to_value(VoiceLiveEvent::ToolResult {
            name: "x".into(),
            ms: 3,
            chars: 9,
            error: false,
        })
        .unwrap();
        assert_eq!(v["type"], "tool_result");
    }

    #[test]
    fn default_options_are_push_to_talk_with_playback() {
        let o = SessionOptions::default();
        assert_eq!(o.mode, TalkMode::PushToTalk);
        assert!(!o.mute_playback);
    }
}
