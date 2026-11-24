use minutes_core::config::Config;
use serde::Serialize;
use std::sync::Mutex;
#[cfg(feature = "voice-live")]
use tauri::Emitter;
use tauri::Manager;

pub(crate) const LABEL: &str = "voice-hud";
pub(crate) const SIZE: (f64, f64) = (320.0, 52.0);
const PREVIEW_HEIGHT: f64 = 94.0;
const EXPANDED_HEIGHT: f64 = 244.0;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Snapshot {
    revision: u64,
    state: &'static str,
    capture_style: &'static str,
    user: String,
    reply: String,
    detail: String,
    tool_started_ms: Option<i64>,
    output_device: Option<String>,
    virtual_output: bool,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            revision: 0,
            state: "closed",
            capture_style: "open_mic",
            user: String::new(),
            reply: String::new(),
            detail: String::new(),
            tool_started_ms: None,
            output_device: None,
            virtual_output: false,
        }
    }
}

#[derive(Default)]
struct Session {
    #[cfg(feature = "voice-live")]
    generation: u64,
    snapshot: Snapshot,
    #[cfg(feature = "voice-live")]
    user_partial: bool,
    #[cfg(feature = "voice-live")]
    reply_partial: bool,
}

#[derive(Default)]
pub(crate) struct VoiceHud(Mutex<Session>);

fn position(window: &tauri::WebviewWindow, height: f64) -> Result<(), String> {
    let monitor = window
        .current_monitor()
        .map_err(|e| e.to_string())?
        .or_else(|| window.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        let anchor = Config::load().ui.voice_hud_anchor;
        let (x, y) = crate::commands::dictation_hud_anchor_position(
            &anchor,
            area.position.x as f64 / scale,
            area.position.y as f64 / scale,
            area.size.width as f64 / scale,
            area.size.height as f64 / scale,
            SIZE.0,
            height,
        );
        window
            .set_position(tauri::LogicalPosition::new(x, y))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(feature = "voice-live")]
pub(crate) fn open(app: &tauri::AppHandle) -> Result<u64, String> {
    if let Some(window) = app.get_webview_window(LABEL) {
        window.destroy().map_err(|e| e.to_string())?;
    }
    let hud = app.state::<VoiceHud>();
    let generation = {
        let mut session = hud.0.lock().unwrap_or_else(|e| e.into_inner());
        session.generation += 1;
        session.snapshot = Snapshot {
            state: "connecting",
            revision: session.snapshot.revision + 1,
            ..Snapshot::default()
        };
        session.user_partial = false;
        session.reply_partial = false;
        session.generation
    };
    let window = tauri::WebviewWindowBuilder::new(
        app,
        LABEL,
        tauri::WebviewUrl::App("voice-hud.html".into()),
    )
    .title("Voice")
    .inner_size(SIZE.0, SIZE.1)
    .visible(false)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .content_protected(true)
    .focused(false)
    .focusable(false)
    .skip_taskbar(true)
    .build()
    .map_err(|e| e.to_string())?;
    position(&window, SIZE.1)?;
    window.show().map_err(|e| e.to_string())?;
    Ok(generation)
}

#[cfg(feature = "voice-live")]
pub(crate) fn event(
    app: &tauri::AppHandle,
    generation: u64,
    event: &minutes_core::voice_live::VoiceLiveEvent,
) -> bool {
    use minutes_core::voice_live::{VoiceLiveEvent as Event, VoiceLiveState as State};

    let hud = app.state::<VoiceHud>();
    let mut session = hud.0.lock().unwrap_or_else(|e| e.into_inner());
    if session.generation != generation {
        return false;
    }
    if session.snapshot.state == "closed" {
        return true;
    }
    match event {
        Event::Level { rms } | Event::PlaybackLevel { rms } => {
            let displaying_stream = matches!(
                (event, session.snapshot.state),
                (Event::Level { .. }, "ready" | "listening")
                    | (Event::PlaybackLevel { .. }, "speaking")
            );
            if displaying_stream {
                if let Some(window) = app.get_webview_window(LABEL) {
                    let _ = window.emit("voice:hud-level", rms.clamp(0.0, 1.0));
                }
            }
            return true;
        }
        Event::State {
            state: State::Closed,
        }
        | Event::Closed { .. } => {
            let detail = match event {
                Event::Closed { reason } => reason.chars().take(160).collect(),
                _ => "Voice ended".into(),
            };
            finish(app, &mut session, detail);
            return true;
        }
        Event::OutputDevice { name } => {
            session.snapshot.virtual_output = is_virtual_output(name);
            session.snapshot.output_device = Some(name.clone());
        }
        Event::State { state } => {
            if *state == State::Listening && session.snapshot.state == "ready" {
                session.snapshot.user.clear();
                session.snapshot.reply.clear();
            }
            session.snapshot.state = match state {
                State::Connecting => "connecting",
                State::Ready => "ready",
                State::Listening => "listening",
                State::Thinking => "thinking",
                State::Speaking => "speaking",
                State::Closed => unreachable!("handled above"),
            };
        }
        Event::UserTranscript { text, partial } => {
            let was_partial = session.user_partial;
            if !was_partial {
                session.snapshot.reply.clear();
            }
            transcript(&mut session.snapshot.user, text, *partial, was_partial);
            session.user_partial = *partial;
        }
        Event::AssistantTranscript { text, partial } => {
            let was_partial = session.reply_partial;
            transcript(&mut session.snapshot.reply, text, *partial, was_partial);
            session.reply_partial = *partial;
        }
        Event::ToolCall { name, .. } => {
            session.snapshot.detail = match name.as_str() {
                "upcoming_meetings" => "Checking calendar",
                "search_meetings" | "get_meeting" | "list_meetings" => "Looking up meetings",
                "search_brain" | "read_brain" => "Looking up your notes",
                "ask_agent" => "Preparing agent review",
                _ => "Looking up an answer",
            }
            .into();
            session.snapshot.tool_started_ms = Some(chrono::Utc::now().timestamp_millis());
        }
        Event::ToolResult { .. } | Event::Review { .. } => {
            session.snapshot.detail.clear();
            session.snapshot.tool_started_ms = None;
        }
        _ => return true,
    }
    publish(app, &mut session.snapshot);
    true
}

#[cfg(feature = "voice-live")]
fn is_virtual_output(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    [
        "jump desktop audio",
        "blackhole",
        "soundflower",
        "loopback",
        "vb-audio",
        "virtual",
    ]
    .iter()
    .any(|label| name.contains(label))
}

#[cfg(feature = "voice-live")]
fn transcript(target: &mut String, text: &str, partial: bool, was_partial: bool) {
    if !partial || !was_partial {
        target.clear();
    }
    target.push_str(text);
    if target.chars().count() > 1200 {
        if let Some((start, _)) = target.char_indices().rev().nth(1199) {
            target.drain(..start);
        }
    }
}

#[cfg(feature = "voice-live")]
fn publish(app: &tauri::AppHandle, snapshot: &mut Snapshot) {
    snapshot.revision += 1;
    if let Some(window) = app.get_webview_window(LABEL) {
        let _ = window.emit("voice:hud", &*snapshot);
    }
}

#[cfg(feature = "voice-live")]
fn finish(app: &tauri::AppHandle, session: &mut Session, detail: String) {
    session.snapshot = Snapshot {
        detail,
        revision: session.snapshot.revision,
        ..Snapshot::default()
    };
    publish(app, &mut session.snapshot);

    let generation = session.generation;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(650));
        let handle = app.clone();
        let _ = app.run_on_main_thread(move || {
            let hud = handle.state::<VoiceHud>();
            let session = hud.0.lock().unwrap_or_else(|e| e.into_inner());
            if session.generation == generation && session.snapshot.state == "closed" {
                if let Some(window) = handle.get_webview_window(LABEL) {
                    let _ = window.destroy();
                }
            }
        });
    });
}

#[cfg(feature = "voice-live")]
pub(crate) fn close(app: &tauri::AppHandle, detail: &str) {
    let hud = app.state::<VoiceHud>();
    let mut session = hud.0.lock().unwrap_or_else(|e| e.into_inner());
    if session.snapshot.state != "closed" {
        finish(app, &mut session, detail.into());
    }
}

#[tauri::command]
pub(crate) fn cmd_voice_hud_ready(hud: tauri::State<VoiceHud>) -> Snapshot {
    hud.0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .snapshot
        .clone()
}

#[tauri::command]
pub(crate) fn cmd_voice_hud_resize(app: tauri::AppHandle, layout: &str) -> Result<(), String> {
    let window = app.get_webview_window(LABEL).ok_or("Voice HUD is closed")?;
    let height = match layout {
        "compact" => SIZE.1,
        "preview" => PREVIEW_HEIGHT,
        "expanded" => EXPANDED_HEIGHT,
        _ => return Err("Unknown Voice HUD layout".into()),
    };
    window
        .set_size(tauri::LogicalSize::new(SIZE.0, height))
        .map_err(|e| e.to_string())?;
    position(&window, height)
}

#[tauri::command]
pub(crate) fn cmd_remember_voice_hud_position(app: tauri::AppHandle) -> Result<(), String> {
    let window = app.get_webview_window(LABEL).ok_or("Voice HUD is closed")?;
    let anchor = crate::commands::current_hud_anchor(&window)?;
    let mut config = Config::load();
    if config.ui.voice_hud_anchor != anchor {
        config.ui.voice_hud_anchor = anchor.into();
        config.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(all(test, feature = "voice-live"))]
mod tests {
    use super::*;

    #[test]
    fn recognizes_known_virtual_output_names() {
        assert!(is_virtual_output("Jump Desktop Audio"));
        assert!(is_virtual_output("BlackHole 2ch"));
        assert!(!is_virtual_output("MacBook Pro Speakers"));
        assert!(!is_virtual_output("AirPods Pro"));
    }

    #[test]
    fn transcript_chunks_reconcile_with_final_and_roll_over() {
        let mut text = String::new();
        transcript(&mut text, "Good ", true, false);
        transcript(&mut text, "morning", true, true);
        assert_eq!(text, "Good morning");
        transcript(&mut text, "Good morning.", false, true);
        assert_eq!(text, "Good morning.");
        transcript(&mut text, &"字".repeat(1201), true, false);
        assert_eq!(text.chars().count(), 1200);
    }
}
