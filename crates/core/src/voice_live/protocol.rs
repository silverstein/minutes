//! Live API wire protocol: a blocking WebSocket client and the JSON shapes it speaks.
//!
//! One IO thread owns the socket. It reads with a short timeout, decodes server
//! frames into [`ServerEvent`]s on an inbound channel, and drains an outbound
//! channel of client messages between reads. Nothing else touches the socket, so
//! there is no lock a slow tool could hold.

use std::io::ErrorKind;
use std::net::TcpStream;
use std::thread::JoinHandle;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use crossbeam_channel::{unbounded, Receiver, Sender, TrySendError};
use serde_json::{json, Value};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

use super::VoiceLiveError;
use crate::interaction::live::{self, LiveProfile, ProviderActivity};

/// Base endpoint for the bidirectional Live API.
pub const LIVE_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";
const THINKING_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1alpha.GenerativeService.BidiGenerateContent";

/// Model audio is fixed by the provider.
pub const OUTPUT_SAMPLE_RATE: u32 = 24_000;
/// Mic audio we send. Matches `AudioStream`.
pub const INPUT_SAMPLE_RATE: u32 = 16_000;

/// A tool invocation requested by the model.
#[derive(Debug, Clone)]
pub struct FunctionCall {
    pub id: String,
    pub name: String,
    pub args: Value,
}

/// Decoded server messages. Anything unrecognized becomes `Other` with the raw
/// keys so a protocol change is visible in the log rather than swallowed.
#[derive(Debug, Clone)]
pub enum ServerEvent {
    SetupComplete,
    /// Raw little-endian PCM16 at [`OUTPUT_SAMPLE_RATE`].
    Audio(Vec<u8>),
    InputTranscript(String),
    OutputTranscript(String),
    /// Model text part (only when the provider falls back to text).
    Text(String),
    Interrupted,
    TurnComplete,
    InteractionStatus(ProviderActivity),
    ToolCall(Vec<FunctionCall>),
    ToolCallCancellation(Vec<String>),
    GoAway(String),
    ResumptionHandle(String),
    Closed(String),
    Error(String),
    Other(Vec<String>),
}

/// Everything needed to open a session.
#[derive(Debug, Clone)]
pub struct SessionSetup {
    pub model: String,
    pub thinking_level: String,
    pub api_key: String,
    pub system_instruction: String,
    pub function_declarations: Vec<Value>,
    pub language: String,
    pub voice_name: String,
    /// `true` for push-to-talk: we send activityStart/activityEnd ourselves.
    pub manual_activity: bool,
    /// Let the model stay silent when speech was not addressed to it.
    pub proactive_audio: bool,
    /// Open-mic speech-start sensitivity: "low", "high", or empty for the provider default.
    pub start_sensitivity: String,
    /// Open-mic speech-end sensitivity: "low", "high", or empty for the provider default.
    pub end_sensitivity: String,
    pub resume_handle: Option<String>,
}

impl SessionSetup {
    pub fn profile(&self) -> Result<LiveProfile, VoiceLiveError> {
        let profile = LiveProfile::gemini(&self.model).map_err(VoiceLiveError::Connect)?;
        if profile == LiveProfile::AsyncReasoning
            && !matches!(
                self.thinking_level.to_ascii_lowercase().as_str(),
                "low" | "medium" | "high"
            )
        {
            return Err(VoiceLiveError::Connect(
                "thinking_level must be low, medium or high".into(),
            ));
        }
        Ok(profile)
    }

    fn to_json(&self) -> Value {
        // connect() validates the profile before network access. Fixtures may
        // still inspect an unknown model's generic JSON shape.
        let profile = self.profile().unwrap_or(LiveProfile::Standard);
        let declarations: Vec<Value> = self
            .function_declarations
            .iter()
            .cloned()
            .map(|d| {
                profile
                    .function_declaration(d)
                    .expect("validated function object")
            })
            .collect();
        let mut setup = json!({
            "model": format!("models/{}", self.model.strip_prefix("models/").unwrap_or(&self.model)),
            "generationConfig": {
                "responseModalities": ["AUDIO"],
                "speechConfig": { "languageCode": self.language },
            },
            "systemInstruction": { "parts": [{ "text": self.system_instruction }] },
            "inputAudioTranscription": {},
            "outputAudioTranscription": {},
            "contextWindowCompression": { "slidingWindow": {} },
        });
        if !self.voice_name.trim().is_empty() {
            setup["generationConfig"]["speechConfig"]["voiceConfig"] = json!({
                "prebuiltVoiceConfig": { "voiceName": self.voice_name.trim() }
            });
        }
        if !self.function_declarations.is_empty() {
            setup["tools"] = json!([{ "functionDeclarations": declarations }]);
        }
        if profile == LiveProfile::AsyncReasoning {
            setup["generationConfig"]["thinkingConfig"] =
                json!({"thinkingLevel": self.thinking_level.to_ascii_uppercase()});
        }
        if self.manual_activity {
            setup["realtimeInputConfig"] =
                json!({ "automaticActivityDetection": { "disabled": true } });
        } else {
            let mut detection = serde_json::Map::new();
            if let Some(v) = sensitivity_enum("START_SENSITIVITY", &self.start_sensitivity) {
                detection.insert("startOfSpeechSensitivity".into(), Value::String(v));
            }
            if let Some(v) = sensitivity_enum("END_SENSITIVITY", &self.end_sensitivity) {
                detection.insert("endOfSpeechSensitivity".into(), Value::String(v));
            }
            if !detection.is_empty() {
                setup["realtimeInputConfig"] =
                    json!({ "automaticActivityDetection": Value::Object(detection) });
            }
        }
        // Only meaningful with the provider's own detection running: in
        // push-to-talk the user has already said every turn is for it.
        if profile == LiveProfile::AsyncReasoning || (self.proactive_audio && !self.manual_activity)
        {
            setup["proactivity"] = json!({ "proactiveAudio": true });
        }
        setup["sessionResumption"] = match &self.resume_handle {
            Some(h) => json!({ "handle": h }),
            None => json!({}),
        };
        json!({ "setup": setup })
    }
}

/// Handle to a live session. Cloning the sender side is cheap; the IO thread
/// exits when the socket closes or [`LiveClient::close`] is called.
pub struct LiveClient {
    profile: LiveProfile,
    outbox: Sender<Message>,
    pub inbox: Receiver<ServerEvent>,
    io_thread: Option<JoinHandle<()>>,
}

impl LiveClient {
    /// Open the socket, send the setup message, and start the IO thread.
    pub fn connect(setup: &SessionSetup) -> Result<Self, VoiceLiveError> {
        let profile = setup.profile()?;
        for declaration in &setup.function_declarations {
            profile
                .function_declaration(declaration.clone())
                .map_err(VoiceLiveError::Connect)?;
        }
        let endpoint = if profile == LiveProfile::AsyncReasoning {
            THINKING_WS_BASE
        } else {
            LIVE_WS_BASE
        };
        let url = format!("{endpoint}?key={}", setup.api_key);
        let (mut socket, _response) = tungstenite::connect(url.as_str())
            .map_err(|e| VoiceLiveError::Connect(redact_key(&e.to_string(), &setup.api_key)))?;
        set_read_timeout(&mut socket, Duration::from_millis(20));
        socket
            .send(Message::Text(setup.to_json().to_string().into()))
            .map_err(|e| VoiceLiveError::Connect(format!("setup send failed: {e}")))?;

        let (outbox, out_rx) = crossbeam_channel::bounded::<Message>(128);
        let (in_tx, inbox) = unbounded::<ServerEvent>();
        let io_thread = std::thread::Builder::new()
            .name("voice-live-io".into())
            .spawn(move || io_loop(socket, out_rx, in_tx))
            .map_err(|e| VoiceLiveError::Connect(format!("io thread: {e}")))?;
        Ok(Self {
            profile,
            outbox,
            inbox,
            io_thread: Some(io_thread),
        })
    }

    fn send_json(&self, value: Value) -> Result<(), VoiceLiveError> {
        match self
            .outbox
            .try_send(Message::Text(value.to_string().into()))
        {
            Ok(()) => Ok(()),
            Err(TrySendError::Disconnected(_)) => Err(VoiceLiveError::Closed),
            Err(TrySendError::Full(_)) => Err(VoiceLiveError::Closed),
        }
    }

    /// Stream one chunk of little-endian PCM16 mic audio at 16 kHz.
    pub fn send_audio(&self, pcm16: &[u8]) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "realtimeInput": { "audio": {
            "data": B64.encode(pcm16),
            "mimeType": format!("audio/pcm;rate={INPUT_SAMPLE_RATE}"),
        }}}))
    }

    /// Deliver one image frame as an ordered part of the conversation.
    ///
    /// Deliberately not `realtimeInput`. That is the media timeline, and it is
    /// not ordered against `toolResponse`: a frame pushed there can still be
    /// ingesting when the model answers the tool call, so it answers from the
    /// previous frame and appears to be one turn behind. `clientContent` lands
    /// in send order, and `turnComplete: false` leaves the turn open for the
    /// tool response that explains the frame.
    pub fn send_image(
        &self,
        bytes: &[u8],
        mime: &str,
        caption: &str,
    ) -> Result<(), VoiceLiveError> {
        self.send_json(client_image_json(bytes, mime, caption))
    }

    /// Manual voice activity markers (push-to-talk mode only).
    pub fn activity_start(&self) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "realtimeInput": { "activityStart": {} } }))
    }

    /// See [`Self::activity_start`].
    pub fn activity_end(&self) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "realtimeInput": { "activityEnd": {} } }))
    }

    /// A typed user turn (useful for tests and the CLI).
    pub fn send_text_turn(&self, text: &str) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "clientContent": {
            "turns": [{ "role": "user", "parts": [{ "text": text }] }],
            "turnComplete": true,
        }}))
    }

    /// Add host state without starting a new user turn or interrupting speech.
    pub fn send_context_update(&self, text: &str) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "clientContent": {
            "turns": [{ "role": "user", "parts": [{ "text": text }] }],
            "turnComplete": false,
        }}))
    }

    /// Answer a tool call. `scheduling` is WHEN_IDLE, INTERRUPT, or SILENT.
    pub fn send_tool_response(
        &self,
        call: &FunctionCall,
        result: &str,
        scheduling: &str,
    ) -> Result<(), VoiceLiveError> {
        let response = self
            .profile
            .function_response(&call.id, &call.name, json!(result), scheduling)
            .map_err(VoiceLiveError::Connect)?;
        self.send_json(json!({ "toolResponse": { "functionResponses": [response] }}))
    }

    /// Ask the IO thread to close the socket and wait for it.
    pub fn close(mut self) {
        let _ = self.outbox.send(Message::Close(None));
        if let Some(t) = self.io_thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for LiveClient {
    fn drop(&mut self) {
        let _ = self.outbox.send(Message::Close(None));
        if let Some(t) = self.io_thread.take() {
            let _ = t.join();
        }
    }
}

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

fn set_write_timeout(socket: &mut Socket, timeout: Duration) {
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_write_timeout(Some(timeout));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_write_timeout(Some(timeout));
        }
        _ => {}
    }
}

/// Cap on a single socket write. Microphone audio arrives continuously, so a
/// peer that stops reading would otherwise park the writer while the outbox
/// grows, and closing the session would then join a thread that never returns.
const WRITE_TIMEOUT: Duration = Duration::from_secs(10);

fn set_read_timeout(socket: &mut Socket, timeout: Duration) {
    set_write_timeout(socket, WRITE_TIMEOUT);
    match socket.get_mut() {
        MaybeTlsStream::Plain(s) => {
            let _ = s.set_read_timeout(Some(timeout));
        }
        MaybeTlsStream::Rustls(s) => {
            let _ = s.sock.set_read_timeout(Some(timeout));
        }
        _ => {}
    }
}

fn io_loop(mut socket: Socket, out_rx: Receiver<Message>, in_tx: Sender<ServerEvent>) {
    let mut closing = false;
    loop {
        // Bound each batch so continuous mic input cannot starve provider events.
        for _ in 0..16 {
            match out_rx.try_recv() {
                Ok(Message::Close(frame)) => {
                    let _ = socket.close(frame);
                    closing = true;
                    break;
                }
                Ok(msg) => {
                    if let Err(e) = socket.send(msg) {
                        let _ = in_tx.send(ServerEvent::Error(format!("send failed: {e}")));
                        let _ = in_tx.send(ServerEvent::Closed("send failure".into()));
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        match socket.read() {
            Ok(Message::Text(text)) => decode(&text, &in_tx),
            Ok(Message::Binary(bytes)) => match std::str::from_utf8(&bytes) {
                Ok(text) => decode(text, &in_tx),
                Err(_) => {
                    let _ = in_tx.send(ServerEvent::Other(vec!["<non-utf8 binary frame>".into()]));
                }
            },
            Ok(Message::Close(frame)) => {
                let reason = frame
                    .map(|f| format!("{} {}", f.code, f.reason))
                    .unwrap_or_default();
                let _ = in_tx.send(ServerEvent::Closed(reason));
                return;
            }
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
            {
                if closing {
                    let _ = in_tx.send(ServerEvent::Closed("closed by client".into()));
                    return;
                }
            }
            Err(tungstenite::Error::ConnectionClosed) | Err(tungstenite::Error::AlreadyClosed) => {
                let _ = in_tx.send(ServerEvent::Closed("connection closed".into()));
                return;
            }
            Err(e) => {
                let _ = in_tx.send(ServerEvent::Error(e.to_string()));
                let _ = in_tx.send(ServerEvent::Closed("read failure".into()));
                return;
            }
        }
    }
}

/// Decode one server JSON message into zero or more events.
pub fn decode(text: &str, out: &Sender<ServerEvent>) {
    for ev in decode_events(text) {
        let _ = out.send(ev);
    }
}

/// Pure decoder, separated so it can be unit-tested without a socket.
pub fn decode_events(text: &str) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    let v: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return vec![ServerEvent::Error(format!("bad json from server: {e}"))],
    };
    if v.get("setupComplete").is_some() {
        events.push(ServerEvent::SetupComplete);
    }
    if let Some(sc) = v.get("serverContent") {
        if sc.get("interrupted").and_then(Value::as_bool) == Some(true) {
            events.push(ServerEvent::Interrupted);
        }
        if let Some(t) = sc
            .pointer("/inputTranscription/text")
            .and_then(Value::as_str)
        {
            if !t.is_empty() {
                events.push(ServerEvent::InputTranscript(t.to_string()));
            }
        }
        if let Some(t) = sc
            .pointer("/outputTranscription/text")
            .and_then(Value::as_str)
        {
            if !t.is_empty() {
                events.push(ServerEvent::OutputTranscript(t.to_string()));
            }
        }
        if let Some(parts) = sc.pointer("/modelTurn/parts").and_then(Value::as_array) {
            for part in parts {
                if let Some(data) = part.pointer("/inlineData/data").and_then(Value::as_str) {
                    match B64.decode(data) {
                        Ok(bytes) => events.push(ServerEvent::Audio(bytes)),
                        Err(e) => events.push(ServerEvent::Error(format!("audio base64: {e}"))),
                    }
                } else if let Some(t) = part.get("text").and_then(Value::as_str) {
                    events.push(ServerEvent::Text(t.to_string()));
                }
            }
        }
        if sc.get("turnComplete").and_then(Value::as_bool) == Some(true) {
            events.push(ServerEvent::TurnComplete);
        }
    }
    if let Some(calls) = v
        .pointer("/toolCall/functionCalls")
        .and_then(Value::as_array)
    {
        let calls: Vec<FunctionCall> = calls
            .iter()
            .filter_map(|c| {
                Some(FunctionCall {
                    id: c
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    name: c.get("name").and_then(Value::as_str)?.to_string(),
                    args: c.get("args").cloned().unwrap_or_else(|| json!({})),
                })
            })
            .collect();
        if !calls.is_empty() {
            events.push(ServerEvent::ToolCall(calls));
        }
    }
    if let Some(ids) = v
        .pointer("/toolCallCancellation/ids")
        .and_then(Value::as_array)
    {
        events.push(ServerEvent::ToolCallCancellation(
            ids.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
        ));
    }
    if let Some(g) = v.get("goAway") {
        events.push(ServerEvent::GoAway(
            g.get("timeLeft").map(|t| t.to_string()).unwrap_or_default(),
        ));
    }
    if let Some(u) = v.get("sessionResumptionUpdate") {
        if u.get("resumable").and_then(Value::as_bool) == Some(true) {
            if let Some(h) = u.get("newHandle").and_then(Value::as_str) {
                events.push(ServerEvent::ResumptionHandle(h.to_string()));
            }
        }
    }
    if let Some(err) = v.get("error") {
        events.push(ServerEvent::Error(err.to_string()));
    }
    if let Some(status) = live::interaction_status(&v) {
        events.push(ServerEvent::InteractionStatus(status));
    }
    // Only genuinely new top-level keys are worth surfacing. Known envelopes that
    // carried nothing actionable this time (a `serverContent` with only
    // `generationComplete`, a bare `usageMetadata`, a keepalive `{}`) stay quiet.
    if events.is_empty() {
        let unknown: Vec<String> = v
            .as_object()
            .map(|o| {
                o.keys()
                    .filter(|k| !KNOWN_TOP_LEVEL_KEYS.contains(&k.as_str()))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !unknown.is_empty() {
            events.push(ServerEvent::Other(unknown));
        }
    }
    events
}

/// Top-level keys the decoder understands, including ones it deliberately ignores.
const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "setupComplete",
    "serverContent",
    "toolCall",
    "toolCallCancellation",
    "goAway",
    "sessionResumptionUpdate",
    "usageMetadata",
    "interactionStatus",
    "interaction_status",
    "error",
];

/// The wire message for one image frame. Split out so the shape is testable
/// without a socket.
///
/// `turnComplete` is true and the frame carries a caption, so this is the turn
/// the model answers. An earlier version sent the frame as an open-ended
/// message just before the tool response, and the model reliably answered the
/// tool call without it and only saw the frame on the following turn. Waiting
/// longer did not help, because the frame was not part of the turn being
/// generated at all.
fn client_image_json(bytes: &[u8], mime: &str, caption: &str) -> Value {
    json!({ "clientContent": {
        "turns": [{ "role": "user", "parts": [
            { "inlineData": { "mimeType": mime, "data": B64.encode(bytes) } },
            { "text": caption },
        ]}],
        "turnComplete": true,
    }})
}

/// Map a config level to the provider's enum; anything else keeps the default.
fn sensitivity_enum(prefix: &str, level: &str) -> Option<String> {
    match level.trim().to_ascii_lowercase().as_str() {
        "low" => Some(format!("{prefix}_LOW")),
        "high" => Some(format!("{prefix}_HIGH")),
        _ => None,
    }
}

fn redact_key(message: &str, key: &str) -> String {
    if key.is_empty() {
        message.to_string()
    } else {
        message.replace(key, "<redacted>")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_frame_is_an_ordered_turn_not_realtime_media() {
        let msg = client_image_json(&[1u8, 2, 3], "image/png", "look at this");
        // Not the media side channel: that arrived a turn late every time.
        assert!(msg.get("realtimeInput").is_none());
        let parts = &msg["clientContent"]["turns"][0]["parts"];
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/png");
        assert!(parts[0]["inlineData"]["data"]
            .as_str()
            .is_some_and(|d| !d.is_empty()));
        assert_eq!(parts[1]["text"], "look at this");
        // The frame completes the turn, so it is what the model answers.
        assert_eq!(msg["clientContent"]["turnComplete"], true);
    }

    #[test]
    fn setup_json_has_required_shape() {
        let setup = SessionSetup {
            model: "gemini-3.8-live".into(),
            api_key: "k".into(),
            thinking_level: "medium".into(),
            system_instruction: "hi".into(),
            function_declarations: vec![
                json!({"name": "t", "parameters": {"type": "object", "properties": {}}}),
            ],
            language: "en-US".into(),
            voice_name: String::new(),
            manual_activity: true,
            proactive_audio: false,
            start_sensitivity: "low".into(),
            end_sensitivity: "low".into(),
            resume_handle: None,
        };
        let v = setup.to_json();
        assert_eq!(v["setup"]["model"], "models/gemini-3.8-live");
        assert!(v["setup"]["generationConfig"]
            .get("thinkingConfig")
            .is_none());
        assert_eq!(
            v["setup"]["generationConfig"]["responseModalities"][0],
            "AUDIO"
        );
        assert_eq!(
            v["setup"]["generationConfig"]["speechConfig"]["languageCode"],
            "en-US"
        );
        assert_eq!(
            v["setup"]["realtimeInputConfig"]["automaticActivityDetection"]["disabled"],
            true
        );
        assert_eq!(
            v["setup"]["tools"][0]["functionDeclarations"][0]["name"],
            "t"
        );
        assert!(v["setup"]["inputAudioTranscription"].is_object());
        assert!(v["setup"]["outputAudioTranscription"].is_object());
        assert!(v["setup"]["generationConfig"]["speechConfig"]
            .get("voiceConfig")
            .is_none());
        for model in ["gemini-3.8-live", "gemini-3.8-live-extended-thinking"] {
            let named = SessionSetup {
                model: model.into(),
                voice_name: "Charon".into(),
                ..setup.clone()
            };
            assert_eq!(
                named.to_json()["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]
                    ["prebuiltVoiceConfig"]["voiceName"],
                "Charon"
            );
        }
    }

    #[test]
    fn a_resumed_setup_keeps_everything_the_session_needs() {
        let setup = SessionSetup {
            model: "m".into(),
            api_key: "k".into(),
            thinking_level: "medium".into(),
            system_instruction: "the rules".into(),
            function_declarations: vec![
                json!({"name": "t", "parameters": {"type": "object", "properties": {}}}),
            ],
            language: "en-US".into(),
            voice_name: "Charon".into(),
            manual_activity: false,
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: Some("handle-1".into()),
        };
        let v = setup.to_json();
        assert_eq!(v["setup"]["sessionResumption"]["handle"], "handle-1");
        assert_eq!(
            v["setup"]["generationConfig"]["speechConfig"]["voiceConfig"]["prebuiltVoiceConfig"]
                ["voiceName"],
            "Charon"
        );
        // A resumed socket that lost the instructions or the tools would look
        // like the same conversation and behave like a different assistant.
        assert_eq!(
            v["setup"]["systemInstruction"]["parts"][0]["text"],
            "the rules"
        );
        assert_eq!(
            v["setup"]["tools"][0]["functionDeclarations"][0]["name"],
            "t"
        );
    }

    #[test]
    fn proactivity_is_asked_for_only_with_provider_detection() {
        let base = SessionSetup {
            model: "m".into(),
            api_key: "k".into(),
            thinking_level: "medium".into(),
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
            voice_name: String::new(),
            proactive_audio: true,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        assert_eq!(
            base.to_json()["setup"]["proactivity"]["proactiveAudio"],
            true
        );
        // Push-to-talk already says every turn is meant for it.
        let manual = SessionSetup {
            manual_activity: true,
            ..base.clone()
        };
        assert!(manual.to_json()["setup"].get("proactivity").is_none());
        // Off unless asked for, because an unsupported field fails the setup.
        let off = SessionSetup {
            proactive_audio: false,
            ..base
        };
        assert!(off.to_json()["setup"].get("proactivity").is_none());
    }

    #[test]
    fn open_mic_setup_passes_speech_sensitivity() {
        let setup = SessionSetup {
            model: "m".into(),
            api_key: "k".into(),
            thinking_level: "medium".into(),
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
            voice_name: String::new(),
            proactive_audio: false,
            start_sensitivity: "low".into(),
            end_sensitivity: "HIGH".into(),
            resume_handle: None,
        };
        let v = setup.to_json();
        let detection = &v["setup"]["realtimeInputConfig"]["automaticActivityDetection"];
        assert_eq!(
            detection["startOfSpeechSensitivity"],
            "START_SENSITIVITY_LOW"
        );
        assert_eq!(detection["endOfSpeechSensitivity"], "END_SENSITIVITY_HIGH");
        assert!(detection.get("disabled").is_none());
    }

    #[test]
    fn open_mic_setup_omits_manual_activity() {
        let setup = SessionSetup {
            model: "m".into(),
            api_key: "k".into(),
            thinking_level: "medium".into(),
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
            voice_name: String::new(),
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: Some("h".into()),
        };
        let v = setup.to_json();
        assert!(v["setup"].get("realtimeInputConfig").is_none());
        assert!(v["setup"].get("tools").is_none());
        assert_eq!(v["setup"]["sessionResumption"]["handle"], "h");
    }

    #[test]
    fn decodes_audio_transcripts_and_turn_complete() {
        let msg = json!({"serverContent": {
            "modelTurn": {"parts": [{"inlineData": {"mimeType": "audio/pcm;rate=24000", "data": B64.encode([1u8, 0, 2, 0])}}]},
            "outputTranscription": {"text": "hello"},
            "turnComplete": true
        }});
        let events = decode_events(&msg.to_string());
        assert!(matches!(events[0], ServerEvent::OutputTranscript(ref t) if t == "hello"));
        assert!(matches!(events[1], ServerEvent::Audio(ref b) if b == &[1u8, 0, 2, 0]));
        assert!(matches!(events[2], ServerEvent::TurnComplete));
    }

    #[test]
    fn decodes_tool_calls_and_interrupt() {
        let msg = json!({"toolCall": {"functionCalls": [{"id": "c1", "name": "list_meetings", "args": {"limit": 3}}]}});
        let events = decode_events(&msg.to_string());
        match &events[0] {
            ServerEvent::ToolCall(calls) => {
                assert_eq!(calls[0].id, "c1");
                assert_eq!(calls[0].name, "list_meetings");
                assert_eq!(calls[0].args["limit"], 3);
            }
            other => panic!("unexpected {other:?}"),
        }
        let events = decode_events(&json!({"serverContent": {"interrupted": true}}).to_string());
        assert!(matches!(events[0], ServerEvent::Interrupted));
    }

    #[test]
    fn known_envelopes_with_nothing_actionable_stay_quiet() {
        for msg in [
            json!({"serverContent": {"generationComplete": true}}),
            json!({"serverContent": {"modelTurn": {"parts": [{"thought": true}]}}}),
            json!({"usageMetadata": {"totalTokenCount": 12}}),
            json!({}),
        ] {
            assert!(
                decode_events(&msg.to_string()).is_empty(),
                "expected no events for {msg}"
            );
        }
    }

    #[test]
    fn unknown_messages_surface_their_keys() {
        let events = decode_events(&json!({"somethingNew": {}}).to_string());
        assert!(
            matches!(events[0], ServerEvent::Other(ref k) if k == &["somethingNew".to_string()])
        );
    }

    #[test]
    fn connect_errors_never_leak_the_key() {
        assert_eq!(
            redact_key("bad url ?key=SECRET x", "SECRET"),
            "bad url ?key=<redacted> x"
        );
    }

    #[test]
    fn real_extended_setup_and_resume_use_one_capability_profile() {
        let mut setup = SessionSetup {
            model: "models/gemini-3.8-live-extended-thinking".into(),
            api_key: "fixture-never-used".into(),
            thinking_level: "medium".into(),
            system_instruction: String::new(),
            function_declarations: vec![
                json!({"name":"lookup","behavior":"BLOCKING","parameters":{"type":"object"}}),
            ],
            language: "en-US".into(),
            manual_activity: true,
            voice_name: String::new(),
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        for handle in [None, Some("resume-fixture".into())] {
            setup.resume_handle = handle;
            let v = setup.to_json();
            assert_eq!(
                v["setup"]["model"],
                "models/gemini-3.8-live-extended-thinking"
            );
            assert_eq!(
                v["setup"]["tools"][0]["functionDeclarations"][0]["behavior"],
                "NON_BLOCKING"
            );
            assert_eq!(v["setup"]["proactivity"]["proactiveAudio"], true);
            assert_eq!(
                v["setup"]["generationConfig"]["thinkingConfig"]["thinkingLevel"],
                "MEDIUM"
            );
        }
        setup.thinking_level = "minimal".into();
        assert!(setup.profile().is_err());
    }

    #[test]
    fn actual_client_response_omits_unsupported_extended_scheduling() {
        let (outbox, receiver) = crossbeam_channel::bounded(4);
        let (_, inbox) = unbounded();
        let client = LiveClient {
            profile: LiveProfile::AsyncReasoning,
            outbox,
            inbox,
            io_thread: None,
        };
        let call = FunctionCall {
            id: "call-1".into(),
            name: "lookup".into(),
            args: json!({}),
        };
        client
            .send_tool_response(&call, "fixture", "INTERRUPT")
            .unwrap();
        let Message::Text(text) = receiver.recv().unwrap() else {
            panic!("expected JSON");
        };
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(value["toolResponse"]["functionResponses"][0]["response"]
            .get("scheduling")
            .is_none());
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY and sends synthetic voice-routing requests"]
    fn live_capability_routing_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.ask_agent = true;
        config.voice_live.delegate_agent = "codex".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        for (request, expected) in [
            ("Ask Kodak CLI whether PR 2058 in x1wealth/x1wealth should be landed. I want an assessment only, do not merge it.", "review_pull_request"),
            ("Use extended thinking to analyze this tradeoff: our team can ship now with one known intermittent crash, or delay two days to fix it. We have no hard deadline. Think it through.", "think_deeply"),
            ("I'm a startup founder. You just told me my next meeting is a fireside chat with Alex Komoroske. What does he do that would apply to my job?", "research_public"),
        ] {
            let setup = SessionSetup {
                model: config.voice_live.model.clone(),
                thinking_level: config.voice_live.thinking_level.clone(),
                api_key: crate::voice_live::api_key(&config).unwrap(),
                system_instruction: crate::voice_live::system_prompt(&config, &names, false),
                function_declarations: tools.declarations(),
                language: "en-US".into(),
                voice_name: config.voice_live.voice_name.clone(),
                manual_activity: true,
                proactive_audio: false,
                start_sensitivity: String::new(),
                end_sensitivity: String::new(),
                resume_handle: None,
            };
            let client = LiveClient::connect(&setup).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(90);
            let mut matched = false;
            let mut spoken_answer = String::new();
            while std::time::Instant::now() < deadline {
                match client.inbox.recv_timeout(Duration::from_millis(250)) {
                    Ok(ServerEvent::SetupComplete) => client.send_text_turn(request).unwrap(),
                    Ok(ServerEvent::ToolCall(calls)) => {
                        for call in calls {
                            if expected == "research_public" {
                                assert_ne!(call.name, "ask_agent", "public research must not launch a broad agent");
                            }
                            if call.name == expected {
                                if expected == "review_pull_request" {
                                    assert_eq!(call.args["agent"], "codex");
                                    assert_eq!(call.args["repository"], "x1wealth/x1wealth");
                                    assert_eq!(call.args["number"], 2058);
                                }
                                matched = true;
                            }
                            if call.name == "research_public" {
                                let result = tools.execute(&call.name, &call.args);
                                assert!(!result.is_error, "{}", result.text);
                                assert!(!result.text.contains("requires_local_approval"));
                                println!("public research completed: {} chars", result.text.len());
                                client.send_tool_response(&call, &result.text, "INTERRUPT").unwrap();
                                continue;
                            }
                            client.send_tool_response(&call, "Routing test only. No action or review was executed.", "WHEN_IDLE").unwrap();
                        }
                        if matched && expected != "research_public" { break; }
                    }
                    Ok(ServerEvent::OutputTranscript(text)) if matched => spoken_answer.push_str(&text),
                    Ok(ServerEvent::TurnComplete) if matched && !spoken_answer.is_empty() => break,
                    Ok(ServerEvent::Error(error) | ServerEvent::Closed(error)) => panic!("{error}"),
                    _ => {}
                }
            }
            client.close();
            assert!(matched, "did not route to {expected}");
            if expected == "research_public" {
                assert!(!spoken_answer.is_empty(), "research never produced a spoken answer");
                let lower = spoken_answer.to_lowercase();
                assert!(!lower.contains("approv") && !lower.contains("terminal"), "unexpected approval prompt: {spoken_answer}");
                println!("public research spoken answer: {spoken_answer}");
            }
            println!("routing passed: {expected}");
        }
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; MINUTES_VOICE_SMOKE_VOICE selects the voice"]
    fn live_morris_voice_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.persona = "morris".into();
        config.voice_live.voice_name =
            std::env::var("MINUTES_VOICE_SMOKE_VOICE").unwrap_or_else(|_| "Charon".into());
        let names = crate::voice_live::NameIndex::default();
        let setup = SessionSetup {
            model: config.voice_live.model.clone(),
            thinking_level: config.voice_live.thinking_level.clone(),
            api_key: crate::voice_live::api_key(&config).unwrap(),
            system_instruction: crate::voice_live::system_prompt(&config, &names, false),
            function_declarations: vec![],
            language: config.voice_live.language.clone(),
            voice_name: config.voice_live.voice_name.clone(),
            manual_activity: true,
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        let client = LiveClient::connect(&setup).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(30);
        let mut audio_bytes = 0;
        let mut transcript = String::new();
        while std::time::Instant::now() < deadline {
            match client.inbox.recv_timeout(Duration::from_millis(250)) {
                Ok(ServerEvent::SetupComplete) => client.send_text_turn("Hello. What should I call you? Also, should we schedule a meeting to plan the meeting about reducing meetings?").unwrap(),
                Ok(ServerEvent::Audio(bytes)) => audio_bytes += bytes.len(),
                Ok(ServerEvent::OutputTranscript(text)) => transcript.push_str(&text),
                Ok(ServerEvent::TurnComplete) if audio_bytes > 0 && !transcript.is_empty() => break,
                Ok(ServerEvent::Error(error)) => panic!("voice smoke: {error}"),
                Ok(ServerEvent::Closed(reason)) => panic!("voice smoke closed: {reason}"),
                _ => {}
            }
        }
        client.close();
        println!("MORRIS_VOICE audio_bytes={audio_bytes} transcript={transcript}");
        assert!(audio_bytes > 6_400, "no meaningful audio returned");
        assert!(
            transcript.to_ascii_lowercase().contains("morris"),
            "{transcript}"
        );
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; checks spoken acknowledgment with tool results withheld"]
    fn live_pending_generation_acknowledgment_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.html_prototypes = true;
        config.voice_live.music = true;
        config.voice_live.persona = "morris".into();
        config.voice_live.model = "gemini-3.8-live-extended-thinking".into();
        config.voice_live.voice_name =
            std::env::var("MINUTES_TEST_VOICE").unwrap_or_else(|_| "Kore".into());
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        let setup = SessionSetup {
            model: config.voice_live.model.clone(),
            thinking_level: config.voice_live.thinking_level.clone(),
            api_key: crate::voice_live::api_key(&config).unwrap(),
            system_instruction: crate::voice_live::system_prompt(&config, &names, false),
            function_declarations: tools.declarations(),
            language: config.voice_live.language.clone(),
            voice_name: config.voice_live.voice_name.clone(),
            manual_activity: true,
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        let client = LiveClient::connect(&setup).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        let mut turn = 0;
        let mut called = [false; 2];
        let mut audio = [0usize; 2];
        let mut transcript = [String::new(), String::new()];
        let mut music_description = String::new();
        let mut finished = false;
        // Neither tool is executed or answered: the board stays pending while
        // the second request must get its own audible acknowledgment.
        while std::time::Instant::now() < deadline {
            match client.inbox.recv_timeout(Duration::from_millis(250)) {
                Ok(ServerEvent::SetupComplete) => client.send_text_turn(
                    "Build a custom interactive calculator showing how team cost changes with hours saved each week. Include editable team size, hourly cost and hours saved, and a live chart. This is the complete brief; build it now."
                ).unwrap(),
                Ok(ServerEvent::Audio(bytes)) => audio[turn] += bytes.len(),
                Ok(ServerEvent::OutputTranscript(text)) => transcript[turn].push_str(&text),
                Ok(ServerEvent::ToolCall(calls)) => {
                    for call in calls {
                        assert_eq!(call.name, ["build_prototype", "make_music"][turn]);
                        if call.name == "make_music" {
                            music_description = call.args["description"].as_str().unwrap_or("").to_owned();
                        }
                        called[turn] = true;
                    }
                }
                Ok(ServerEvent::TurnComplete) if called[turn] => {
                    if turn == 0 {
                        turn = 1;
                        client.send_text_turn("While I'm waiting for that, can you create hold music for me?").unwrap();
                    } else {
                        finished = true;
                        break;
                    }
                }
                Ok(ServerEvent::Error(error)) => panic!("acknowledgment smoke: {error}"),
                Ok(ServerEvent::Closed(reason)) => panic!("acknowledgment smoke closed: {reason}"),
                _ => {}
            }
        }
        client.close();
        println!(
            "GENERATION_ACK voice={} called={called:?} audio={audio:?} transcripts={transcript:?} music_description={music_description}",
            config.voice_live.voice_name
        );
        assert!(finished && called.iter().all(|called| *called));
        assert!(
            audio.iter().all(|bytes| *bytes > 6_400),
            "missing spoken acknowledgment"
        );
        assert!(transcript.iter().all(|text| !text.trim().is_empty()));
        let description = music_description.to_ascii_lowercase();
        assert!(
            ["snark", "sarcas", "witt", "dry", "deadpan"]
                .iter()
                .any(|word| description.contains(word)),
            "hold-music default missing: {music_description}"
        );
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; synthetic routing only, no prototypes generated"]
    fn live_prototype_routing_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.html_prototypes = true;
        config.voice_live.ask_agent = true;
        config.voice_live.delegate_agent = "claude".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        let previous = "a".repeat(64);
        for (request, revision, should_build) in [
            ("Build me a small interactive HTML focus timer with start, pause and reset. Use Codex.".to_owned(), false, true),
            (format!("The prototype_id of the timer you just built is {previous}. Revise that prototype to add 15, 25 and 45 minute buttons. Keep the working timer."), true, true),
            ("Maybe a timer could be interesting. Do not build anything yet; let's just discuss what would make it useful.".to_owned(), false, false),
        ] {
            let setup = SessionSetup {
                model: config.voice_live.model.clone(), thinking_level: config.voice_live.thinking_level.clone(),
                api_key: crate::voice_live::api_key(&config).unwrap(),
                system_instruction: crate::voice_live::system_prompt(&config, &names, false),
                function_declarations: tools.declarations(), language: "en-US".into(),
                voice_name: config.voice_live.voice_name.clone(),
                manual_activity: true, proactive_audio: false, start_sensitivity: String::new(),
                end_sensitivity: String::new(), resume_handle: None,
            };
            let client = LiveClient::connect(&setup).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(40);
            let mut matched = false;
            let mut answered = false;
            while std::time::Instant::now() < deadline {
                match client.inbox.recv_timeout(Duration::from_millis(250)) {
                    Ok(ServerEvent::SetupComplete) => client.send_text_turn(&request).unwrap(),
                    Ok(ServerEvent::ToolCall(calls)) => {
                        for call in calls {
                            assert_ne!(call.name, "ask_agent", "prototype escaped to broad delegation");
                            if call.name == "build_prototype" {
                                assert!(should_build, "discussion became unauthorized work");
                                assert!(!call.args["brief"].as_str().unwrap_or("").is_empty());
                                if revision { assert_eq!(call.args["previous_id"], previous); }
                                else { assert_eq!(call.args["agent"], "codex"); }
                                matched = true;
                            }
                            client.send_tool_response(&call, "Routing probe only. Nothing generated, saved or opened.", "WHEN_IDLE").unwrap();
                        }
                        if matched { break; }
                    }
                    Ok(ServerEvent::TurnComplete) if !should_build => { answered = true; break; }
                    Ok(ServerEvent::Error(e) | ServerEvent::Closed(e)) => panic!("{e}"),
                    _ => {}
                }
            }
            client.close();
            assert!(if should_build { matched } else { answered });
            println!("prototype routing passed: revision={revision}, should_build={should_build}");
        }
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; synthetic text routing only, no clipboard or app access"]
    fn live_text_transfer_routing_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.clipboard = true;
        config.voice_live.text_input = true;
        config.voice_live.desktop_control = true;
        config.voice_live.voice_name = "Orus".into();
        config.voice_live.persona = "morris".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        for (request, expected) in [
            ("Summarize the text I just copied. Please read my clipboard for this.", "read_clipboard_text"),
            ("Notes is frontmost and my caret is in the draft. Put exactly 'A better way to work.' there. Do not send it or replace any selection.", "paste_text"),
            ("Read the paragraph I selected in Notes, make it shorter, and replace only that selection.", "paste_text"),
        ] {
            let setup = SessionSetup {
                model: config.voice_live.model.clone(), thinking_level: config.voice_live.thinking_level.clone(),
                api_key: crate::voice_live::api_key(&config).unwrap(),
                system_instruction: crate::voice_live::system_prompt(&config, &names, false),
                function_declarations: tools.declarations(), language: "en-US".into(),
                voice_name: config.voice_live.voice_name.clone(),
                manual_activity: true, proactive_audio: false, start_sensitivity: String::new(),
                end_sensitivity: String::new(), resume_handle: None,
            };
            let client = LiveClient::connect(&setup).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(45);
            let mut matched = false;
            let mut read_selection = false;
            let mut speech = String::new();
            while std::time::Instant::now() < deadline {
                match client.inbox.recv_timeout(Duration::from_millis(250)) {
                    Ok(ServerEvent::SetupComplete) => client.send_text_turn(request).unwrap(),
                    Ok(ServerEvent::OutputTranscript(text)) => speech.push_str(&text),
                    Ok(ServerEvent::ToolCall(calls)) => {
                        for call in calls {
                            if call.name == "read_selected_text" {
                                read_selection = true;
                                client.send_tool_response(&call, r#"{"source":"host_selected_text","bundle_id":"com.apple.Notes","selection_id":"fixture-selection-1","selected_text":"We should schedule fewer meetings to leave more time for focused work."}"#, "INTERRUPT").unwrap();
                                continue;
                            }
                            if call.name == "open_app" {
                                client.send_tool_response(&call, r#"{"opened":true,"app":"Notes"}"#, "INTERRUPT").unwrap();
                                continue;
                            }
                            assert_eq!(call.name, expected, "unexpected text routing");
                            if expected == "paste_text" {
                                assert!(matches!(call.args["target_app"].as_str(), Some("Notes" | "com.apple.Notes")));
                                if request.contains("selected") {
                                    assert!(read_selection);
                                    assert_eq!(call.args["mode"], "replace_selection");
                                    assert_eq!(call.args["expected_selection"], "We should schedule fewer meetings to leave more time for focused work.");
                                    assert_eq!(call.args["selection_id"], "fixture-selection-1");
                                } else {
                                    assert_eq!(call.args["text"], "A better way to work.");
                                }
                            }
                            matched = true;
                        }
                        if matched { break; }
                    }
                    Ok(ServerEvent::Error(e) | ServerEvent::Closed(e)) => panic!("{e}"),
                    _ => {}
                }
            }
            client.close();
            assert!(matched, "missing {expected}; speech={speech}");
            assert!(!speech.to_lowercase().contains("/approve"), "{speech}");
            println!("text routing passed: {expected}; selection={read_selection}");
        }
    }
    #[test]
    #[ignore = "requires GEMINI_API_KEY and MINUTES_SCREEN_FIXTURE_PNG (synthetic image only)"]
    fn live_visible_thread_interpretation_smoke() {
        let image = std::fs::read(std::env::var("MINUTES_SCREEN_FIXTURE_PNG").expect(
            "generate the synthetic image with scripts/fixtures/voice-thread-fixture.swift",
        ))
        .unwrap();
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.screen_on_request = true;
        config.voice_live.persona = "morris".into();
        config.voice_live.voice_name = "Kore".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        let setup = SessionSetup {
            model: config.voice_live.model.clone(),
            thinking_level: config.voice_live.thinking_level.clone(),
            api_key: crate::voice_live::api_key(&config).unwrap(),
            system_instruction: crate::voice_live::system_prompt(&config, &names, false),
            function_declarations: tools.declarations(),
            language: "en-US".into(),
            voice_name: "Kore".into(),
            manual_activity: true,
            proactive_audio: false,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        let client = LiveClient::connect(&setup).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut captures = 0;
        let mut image_due = None;
        let mut image_sent = false;
        let mut replies = [String::new(), String::new()];
        let mut phase = 0;
        let mut completed = false;
        while std::time::Instant::now() < deadline {
            if image_due.is_some_and(|due| std::time::Instant::now() >= due) {
                client
                    .send_image(&image, "image/png", super::super::session::SCREEN_CAPTION)
                    .unwrap();
                image_due = None;
                image_sent = true;
            }
            match client.inbox.recv_timeout(Duration::from_millis(50)) {
                Ok(ServerEvent::SetupComplete) => client
                    .send_text_turn("Can you tell what's going on in this message thread?")
                    .unwrap(),
                Ok(ServerEvent::ToolCall(calls)) => {
                    for call in calls {
                        assert_eq!(
                            call.name, "look_at_screen",
                            "no real tool execution; unexpected call: {}",
                            call.name
                        );
                        captures += 1;
                        assert_eq!(captures, 1, "duplicate capture instead of waiting or interpreting existing evidence");
                        client
                            .send_tool_response(
                                &call,
                                &json!({"note": super::super::tools::SCREEN_DELIVERY_NOTE})
                                    .to_string(),
                                "SILENT",
                            )
                            .unwrap();
                        // Match the host's separate receipt/image ordering, including its delay.
                        image_due = Some(std::time::Instant::now() + Duration::from_millis(800));
                    }
                }
                Ok(ServerEvent::OutputTranscript(text)) => replies[phase].push_str(&text),
                // A tool-call turn can finish after the image was sent. Do not
                // mistake its pre-image acknowledgment for the image answer.
                Ok(ServerEvent::TurnComplete)
                    if image_sent
                        && [
                            "robin", "casey", "place", "location", "venue", "book", "where",
                        ]
                        .iter()
                        .any(|word| replies[phase].to_lowercase().contains(word)) =>
                {
                    if phase == 0 {
                        phase = 1;
                        client
                            .send_text_turn("Yeah, but you can't reason about that.")
                            .unwrap();
                    } else {
                        completed = true;
                        break;
                    }
                }
                Ok(ServerEvent::Error(e) | ServerEvent::Closed(e)) => panic!("{e}"),
                _ => {}
            }
        }
        client.close();
        println!(
            "VISIBLE_THREAD captures={captures} initial={} repair={}",
            replies[0], replies[1]
        );
        assert!(completed, "two spoken answers were not completed");
        assert_eq!(captures, 1);
        for reply in &replies {
            let lower = reply.to_lowercase();
            assert!(
                !lower.contains("don't have access") && !lower.contains("/approve"),
                "{reply}"
            );
            assert!(
                !lower.contains("actually, i can")
                    && !lower.contains("extended thinking capabilities"),
                "{reply}"
            );
            assert!(
                lower.contains("place")
                    || lower.contains("location")
                    || lower.contains("venue")
                    || lower.contains("book")
                    || lower.contains("where"),
                "missing actionable interpretation: {reply}"
            );
        }
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; checks first-tool routing only, never executes tools"]
    fn live_research_before_artifact_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.html_prototypes = true;
        config.voice_live.voice_name = "Kore".into();
        config.voice_live.persona = "morris".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        for request in [
            "How would you define popular mainstream taste in erotica in 2026?",
            "Can you create a reading-list artifact of reputable scholarly resources on erotic romance audiences, representation and consent?",
        ] {
            let setup = SessionSetup {
                model: config.voice_live.model.clone(), thinking_level: config.voice_live.thinking_level.clone(),
                api_key: crate::voice_live::api_key(&config).unwrap(),
                system_instruction: crate::voice_live::system_prompt(&config, &names, false),
                function_declarations: tools.declarations(), language: "en-US".into(),
                voice_name: "Kore".into(), manual_activity: true, proactive_audio: false,
                start_sensitivity: String::new(), end_sensitivity: String::new(), resume_handle: None,
            };
            let client = LiveClient::connect(&setup).unwrap();
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            let mut researched = false;
            let mut speech = String::new();
            while std::time::Instant::now() < deadline {
                match client.inbox.recv_timeout(Duration::from_millis(100)) {
                    Ok(ServerEvent::SetupComplete) => client.send_text_turn(request).unwrap(),
                    Ok(ServerEvent::OutputTranscript(text)) => speech.push_str(&text),
                    Ok(ServerEvent::ToolCall(calls)) => {
                        assert!(!calls.is_empty());
                        for call in calls {
                            assert_eq!(call.name, "research_public", "must research before analysis or artifact creation");
                            researched = true;
                        }
                        break;
                    }
                    Ok(ServerEvent::Error(e) | ServerEvent::Closed(e)) => panic!("{e}"),
                    _ => {}
                }
            }
            client.close();
            assert!(researched, "no research call; speech={speech}");
            println!("RESEARCH_FIRST request={request}; speech={speech}");
        }
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; synthetic source routing only, no browser or research actions"]
    fn live_research_source_recovery_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.desktop_control = true;
        config.voice_live.model = "gemini-3.8-live-extended-thinking".into();
        config.voice_live.thinking_level = "low".into();
        config.voice_live.voice_name = "Kore".into();
        config.voice_live.persona = "morris".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        let setup = SessionSetup {
            model: config.voice_live.model.clone(),
            thinking_level: "low".into(),
            api_key: crate::voice_live::api_key(&config).unwrap(),
            system_instruction: crate::voice_live::system_prompt(&config, &names, false),
            function_declarations: tools.declarations(),
            language: "en-US".into(),
            voice_name: "Kore".into(),
            manual_activity: true,
            proactive_audio: true,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        let client = LiveClient::connect(&setup).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(45);
        let mut phase = 0;
        let mut speech = String::new();
        while std::time::Instant::now() < deadline && phase < 3 {
            match client.inbox.recv_timeout(Duration::from_millis(100)) {
                Ok(ServerEvent::SetupComplete) => client.send_text_turn("Find the specific Circana report about romance print sales and open that article in my browser.").unwrap(),
                Ok(ServerEvent::OutputTranscript(text)) => speech.push_str(&text),
                Ok(ServerEvent::ToolCall(calls)) => {
                    for call in calls {
                        match phase {
                            0 => {
                                assert_eq!(call.name, "research_public", "{speech}");
                                client.send_tool_response(&call, &json!({"answer":"A publisher report describes US print romance sales, not per-capita consumption.","sources":[{"source_id":"source-17","title":"Romance sales report","url":"https://source.test/exact-returned-link"}],"navigation_note":"Use open_research_source with source-17, not a reconstructed URL."}).to_string(), "INTERRUPT").unwrap();
                                phase = 1;
                            }
                            1 => {
                                assert_eq!(call.name, "open_research_source", "{speech}");
                                assert_eq!(call.args["source_id"], "source-17");
                                client.send_tool_response(&call, r#"{"error":"The source could not be opened. It is no longer available. Find a replacement source for the user's requested article; do not retry this source or only promise to search."}"#, "INTERRUPT").unwrap();
                                phase = 2;
                            }
                            2 => {
                                assert_eq!(call.name, "research_public", "recovery must perform research, not only promise it: {speech}");
                                phase = 3;
                            }
                            _ => {}
                        }
                    }
                }
                Ok(ServerEvent::Error(e) | ServerEvent::Closed(e)) => panic!("{e}"),
                _ => {}
            }
        }
        client.close();
        assert_eq!(phase, 3, "incomplete research/open/recovery flow: {speech}");
        println!("SOURCE_RECOVERY: {speech}");
    }

    #[test]
    #[ignore = "requires GEMINI_API_KEY; synthetic board routing, no browser or private data"]
    fn live_decision_board_revision_smoke() {
        let mut config = crate::config::Config::default();
        config.voice_live.enabled = true;
        config.voice_live.allow_cloud = true;
        config.voice_live.html_prototypes = true;
        config.voice_live.model = "gemini-3.8-live-extended-thinking".into();
        config.voice_live.thinking_level = "low".into();
        config.voice_live.voice_name = "Kore".into();
        let names = std::sync::Arc::new(crate::voice_live::NameIndex::default());
        let tools = crate::voice_live::ToolContext::new(config.clone(), names.clone());
        let setup = SessionSetup {
            model: config.voice_live.model.clone(),
            thinking_level: "low".into(),
            api_key: crate::voice_live::api_key(&config).unwrap(),
            system_instruction: crate::voice_live::system_prompt(&config, &names, false),
            function_declarations: tools.declarations(),
            language: "en-US".into(),
            voice_name: "Kore".into(),
            manual_activity: true,
            proactive_audio: true,
            start_sensitivity: String::new(),
            end_sensitivity: String::new(),
            resume_handle: None,
        };
        let client = LiveClient::connect(&setup).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        let mut phase = 0;
        let mut asked = false;
        let mut speech = String::new();
        let board = |revision| json!({"board_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","revision":revision,"title":"Future work","columns":[{"id":"column-1","title":"Now","cards":[{"id":"card-4","title":"Fewer meetings","body":"Manual edit: protect focus"}]},{"id":"column-2","title":"Next","cards":[]},{"id":"column-3","title":"Later","cards":[]}],"selected_card_id":"card-4"});
        while std::time::Instant::now() < deadline && phase < 3 {
            match client.inbox.recv_timeout(Duration::from_millis(250)) {
                Ok(ServerEvent::SetupComplete)=>client.send_text_turn("Create a Now Next Later decision board with one idea: Fewer meetings. Do it now.").unwrap(),
                Ok(ServerEvent::OutputTranscript(text))=>speech.push_str(&text),
                Ok(ServerEvent::ToolCall(calls))=>for call in calls {
                    match phase {
                        0=>{assert_eq!(call.name,"create_decision_board","{speech}");client.send_tool_response(&call,&board(1).to_string(),"SILENT").unwrap();phase=1;},
                        1=>{assert_eq!(call.name,"read_decision_board","{speech}");client.send_tool_response(&call,&board(7).to_string(),"SILENT").unwrap();phase=2;},
                        2=>{assert_eq!(call.name,"edit_decision_board","{speech}");assert_eq!(call.args["revision"],7);assert_eq!(call.args["board_id"],"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");assert_eq!(call.args["change"]["operation"],"move_card");assert_eq!(call.args["change"]["card_id"],"card-4");assert_eq!(call.args["change"]["column_id"],"column-2");phase=3;},
                        _=>{}
                    }
                },
                Ok(ServerEvent::TurnComplete) if phase==1&&!asked=>{asked=true;client.send_text_turn("I selected this card and made a manual edit. Move this one to Next; keep its current writing.").unwrap();},
                Ok(ServerEvent::Error(e)|ServerEvent::Closed(e))=>panic!("{e}"),
                _=>{}
            }
        }
        client.close();
        assert_eq!(phase, 3, "Board flow incomplete: {speech}");
        println!("BOARD_ROUTING: create, read current revision, move selected exact card passed");
    }

    #[test]
    fn compound_status_is_decoded_after_utterance_completion() {
        let events = decode_events(
            &json!({"serverContent":{"turnComplete":true},"interactionStatus":"IN_PROGRESS"})
                .to_string(),
        );
        assert!(matches!(events[0], ServerEvent::TurnComplete));
        assert!(matches!(
            events[1],
            ServerEvent::InteractionStatus(ProviderActivity::InProgress)
        ));
        assert!(matches!(
            decode_events(r#"{"interaction_status":"IDLE"}"#)[0],
            ServerEvent::InteractionStatus(ProviderActivity::Idle)
        ));
    }
}
