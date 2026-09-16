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

/// Base endpoint for the bidirectional Live API.
pub const LIVE_WS_BASE: &str = "wss://generativelanguage.googleapis.com/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent";

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
    pub api_key: String,
    pub system_instruction: String,
    pub function_declarations: Vec<Value>,
    pub language: String,
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
    fn to_json(&self) -> Value {
        let mut setup = json!({
            "model": format!("models/{}", self.model),
            "generationConfig": {
                "responseModalities": ["AUDIO"],
                "speechConfig": { "languageCode": self.language },
            },
            "systemInstruction": { "parts": [{ "text": self.system_instruction }] },
            "inputAudioTranscription": {},
            "outputAudioTranscription": {},
            "contextWindowCompression": { "slidingWindow": {} },
        });
        if !self.function_declarations.is_empty() {
            setup["tools"] = json!([{ "functionDeclarations": self.function_declarations }]);
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
        if self.proactive_audio && !self.manual_activity {
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
    outbox: Sender<Message>,
    pub inbox: Receiver<ServerEvent>,
    io_thread: Option<JoinHandle<()>>,
}

impl LiveClient {
    /// Open the socket, send the setup message, and start the IO thread.
    pub fn connect(setup: &SessionSetup) -> Result<Self, VoiceLiveError> {
        let url = format!("{LIVE_WS_BASE}?key={}", setup.api_key);
        let (mut socket, _response) = tungstenite::connect(url.as_str())
            .map_err(|e| VoiceLiveError::Connect(redact_key(&e.to_string(), &setup.api_key)))?;
        set_read_timeout(&mut socket, Duration::from_millis(20));
        socket
            .send(Message::Text(setup.to_json().to_string().into()))
            .map_err(|e| VoiceLiveError::Connect(format!("setup send failed: {e}")))?;

        let (outbox, out_rx) = unbounded::<Message>();
        let (in_tx, inbox) = unbounded::<ServerEvent>();
        let io_thread = std::thread::Builder::new()
            .name("voice-live-io".into())
            .spawn(move || io_loop(socket, out_rx, in_tx))
            .map_err(|e| VoiceLiveError::Connect(format!("io thread: {e}")))?;
        Ok(Self {
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

    /// Answer a tool call. `scheduling` is WHEN_IDLE, INTERRUPT, or SILENT.
    pub fn send_tool_response(
        &self,
        call: &FunctionCall,
        result: &str,
        scheduling: &str,
    ) -> Result<(), VoiceLiveError> {
        self.send_json(json!({ "toolResponse": { "functionResponses": [{
            "id": call.id,
            "name": call.name,
            "response": { "result": result, "scheduling": scheduling },
        }]}}))
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

fn set_read_timeout(socket: &mut Socket, timeout: Duration) {
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
        // Drain outbound first so a tool response or audio chunk never waits on a read timeout.
        loop {
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
            system_instruction: "hi".into(),
            function_declarations: vec![
                json!({"name": "t", "parameters": {"type": "object", "properties": {}}}),
            ],
            language: "en-US".into(),
            manual_activity: true,
            proactive_audio: false,
            start_sensitivity: "low".into(),
            end_sensitivity: "low".into(),
            resume_handle: None,
        };
        let v = setup.to_json();
        assert_eq!(v["setup"]["model"], "models/gemini-3.8-live");
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
    }

    #[test]
    fn proactivity_is_asked_for_only_with_provider_detection() {
        let base = SessionSetup {
            model: "m".into(),
            api_key: "k".into(),
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
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
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
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
            system_instruction: String::new(),
            function_declarations: vec![],
            language: "en-US".into(),
            manual_activity: false,
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
}
