//! User-picked local pointer delivery. Neither capture nor Stop calls this worker.
use super::{local_address, Hc};
use minutes_core::{config::Config, live_transcript::SessionStatus};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use tauri::{Emitter, Manager};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Terminal {
    address: String,
    name: String,
    labels: BTreeMap<String, String>,
    agent: String,
    location: String,
}

// Process names such as python3 do not establish that a terminal reads prompts.
// Keep this allowlist explicit; shell comment syntax is not a delivery boundary.
fn prompt_agent(row: &serde_json::Value) -> Option<&str> {
    row["agent"]
        .as_str()
        .filter(|agent| matches!(*agent, "claude-code" | "codex"))
}
const ENDED_TARGET: &str = "That terminal has ended. Choose another.";
const UNSUPPORTED_TARGET: &str =
    "That terminal is no longer running a supported agent. Choose another.";
fn selected_row<'a>(
    rows: &'a [serde_json::Value],
    target: &str,
) -> Result<&'a serde_json::Value, String> {
    let row = rows
        .iter()
        .find(|row| row["address"] == target)
        .ok_or(ENDED_TARGET)?;
    if row["state"] != "live" {
        return Err(ENDED_TARGET.into());
    }
    prompt_agent(row).ok_or(UNSUPPORTED_TARGET)?;
    Ok(row)
}
impl Hc {
    fn live_rows(&self) -> Result<Vec<serde_json::Value>, String> {
        let (ok, json) = self.call(&["ls".into(), "--json".into()])?;
        if !ok {
            return Err("Could not list Herdr terminals".into());
        }
        Ok(json["terminals"]
            .as_array()
            .ok_or("Invalid Herdr terminal list")?
            .iter()
            .take(128)
            .cloned()
            .collect())
    }
    fn resolve_terminal(
        &self,
        row: &serde_json::Value,
        timeout: Duration,
    ) -> Result<Option<Terminal>, String> {
        let Some(address) = row["address"].as_str().filter(|v| local_address(v)) else {
            return Ok(None);
        };
        let Some(agent) = prompt_agent(row).filter(|_| row["state"] == "live") else {
            return Ok(None);
        };
        if timeout.is_zero() {
            return Err("Herdr terminal listing timed out".into());
        }
        let bounded = Hc {
            executable: self.executable.clone(),
            timeout,
        };
        let (ok, detail) = bounded.call(&["get".into(), address.into(), "--json".into()])?;
        if !ok || detail["state"] != "live" || detail["address"] != address {
            return Ok(None);
        }
        // Retain only the useful Recall marker. Never return herdr.* IDs,
        // arbitrary labels, terminal screen contents, cwd or argv to the UI.
        let labels = detail["labels"]["minutes.tab"]
            .as_str()
            .map(|value| {
                BTreeMap::from([("minutes.tab".into(), value.chars().take(256).collect())])
            })
            .unwrap_or_default();
        let location = row["where"]
            .as_str()
            .or_else(|| detail["where"].as_str())
            .or_else(|| detail["labels"]["where"].as_str())
            .unwrap_or("")
            .chars()
            .take(256)
            .collect();
        Ok(Some(Terminal {
            address: address.into(),
            name: detail["name"]
                .as_str()
                .unwrap_or(address)
                .chars()
                .take(256)
                .collect(),
            labels,
            agent: agent.into(),
            location,
        }))
    }
    fn terminals(&self) -> Result<Vec<Terminal>, String> {
        let started = Instant::now();
        let rows = self.live_rows()?;
        let mut terminals = Vec::new();
        // ls 0.1.21 supplies the current agent, but omits labels. Resolve only
        // local supported agents through get, under one shared deadline.
        for row in rows {
            if let Some(terminal) =
                self.resolve_terminal(&row, self.timeout.saturating_sub(started.elapsed()))?
            {
                terminals.push(terminal);
            }
        }
        Ok(terminals)
    }
    fn selected_terminal(&self, target: &str) -> Result<Terminal, String> {
        if !local_address(target) {
            return Err("Only local Herdr terminals are supported".into());
        }
        let started = Instant::now();
        let rows = self.live_rows()?;
        let row = selected_row(&rows, target)?;
        self.resolve_terminal(row, self.timeout.saturating_sub(started.elapsed()))?
            .ok_or_else(|| ENDED_TARGET.into())
    }
    fn send_pointer(&self, target: &str, text: &str, cancelled: &AtomicBool) -> Result<(), String> {
        if !local_address(target) {
            return Err("Only local Herdr terminals are supported".into());
        }
        // Fresh ls immediately before every pointer/end-note send: get's live
        // state alone cannot distinguish an agent that returned to a shell.
        let rows = self.live_rows()?;
        selected_row(&rows, target)?;
        let (ok, json) = self.call_cancellable(
            &["send".into(), target.into(), text.into(), "--json".into()],
            cancelled,
        )?;
        if ok && json["turn"].is_u64() {
            Ok(())
        } else {
            Err("Herdr did not accept the message; delivery was not retried".into())
        }
    }
}

#[derive(Clone)]
struct Session {
    key: String,
    pointer: String,
    output_dir: PathBuf,
}
impl Session {
    fn from_status(status: &SessionStatus, config: &Config) -> Option<Self> {
        let pointer = crate::context::live_transcript_pointer(status)?;
        let key =
            serde_json::to_string(&(status.pid, &status.session_id, &status.jsonl_path)).ok()?;
        Some(Self {
            key,
            pointer,
            output_dir: config.output_dir.clone(),
        })
    }
    fn ended(&self) -> String {
        let directory =
            serde_json::to_string(&self.output_dir.to_string_lossy()).unwrap_or_default();
        format!("# Recording ended. Minutes is processing it now; the finished meeting will be saved in {directory}. Check progress with `minutes jobs`, then find it with `minutes list --limit 1`.")
    }
}
fn current_session() -> Option<Session> {
    let config = Config::load_strict().ok()?;
    let (key, started) = capture_identity()?;
    // A fixed JSONL/status path can still refer to the prior recording during
    // startup. Admit only status published for this capture, not its predecessor.
    let published = std::fs::metadata(minutes_core::pid::live_transcript_status_path())
        .ok()?
        .modified()
        .ok()?;
    if published < started {
        return None;
    }
    let mut session =
        Session::from_status(&minutes_core::live_transcript::session_status(), &config)?;
    session.key = key;
    Some(session)
}
fn current_key() -> Option<String> {
    capture_identity().map(|(key, _)| key)
}
fn capture_identity() -> Option<(String, SystemTime)> {
    capture_identity_at(
        &minutes_core::pid::pid_path(),
        &minutes_core::pid::live_transcript_pid_path(),
    )
}
fn capture_identity_at(recording: &Path, live: &Path) -> Option<(String, SystemTime)> {
    for (kind, path) in [("recording", recording), ("live", live)] {
        if minutes_core::pid::inspect_pid_file(path).is_active() {
            // This file is written once per capture and removed at Stop. Its
            // version stays stable when a sidecar heartbeat/relay fails, and
            // changes even when a new recording reuses the app PID/JSONL path.
            let modified = std::fs::metadata(path).ok()?.modified().ok()?;
            let stamp = modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()?
                .as_nanos();
            return Some((format!("{kind}:{stamp}"), modified));
        }
    }
    None
}

#[derive(Clone)]
struct Pick {
    generation: u64,
    session: Session,
    terminal: Terminal,
    phase: String,
    cancelled: Arc<AtomicBool>,
}
#[derive(Default)]
struct Inner {
    generation: u64,
    pick: Option<Pick>,
    notice: Option<String>,
}
#[derive(Clone, Default)]
pub(crate) struct Sharing(Arc<Mutex<Inner>>);
#[derive(Serialize)]
pub(crate) struct View {
    active: bool,
    session: Option<String>,
    selected: Option<Terminal>,
    phase: Option<String>,
    notice: Option<String>,
    generation: u64,
}
struct Delivery {
    generation: u64,
    target: String,
    text: String,
    cancelled: Arc<AtomicBool>,
    ended: bool,
}

impl Sharing {
    fn view(&self, session_key: Option<&str>, ready: bool) -> Result<View, String> {
        let state = self.0.lock().map_err(|_| "Herdr sharing is unavailable")?;
        let pick = state
            .pick
            .as_ref()
            .filter(|pick| session_key == Some(pick.session.key.as_str()));
        Ok(View {
            active: ready || pick.is_some(),
            session: session_key.map(str::to_string),
            selected: pick.map(|p| p.terminal.clone()),
            phase: pick.map(|p| p.phase.clone()),
            notice: pick.and_then(|_| state.notice.clone()),
            generation: state.generation,
        })
    }
    fn generation(&self) -> Result<u64, String> {
        Ok(self
            .0
            .lock()
            .map_err(|_| "Herdr sharing is unavailable")?
            .generation)
    }
    fn pick(&self, session: Session, terminal: Terminal, generation: u64) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "Herdr sharing is unavailable")?;
        if state.generation != generation {
            return Err("The pick was cancelled or changed; nothing was sent".into());
        }
        if state.pick.is_some() {
            return Err("Cancel the current pick before choosing another terminal".into());
        }
        state.generation += 1;
        state.notice = None;
        state.pick = Some(Pick {
            generation: state.generation,
            session,
            terminal,
            phase: "queued".into(),
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        Ok(())
    }
    fn cancel(&self) -> Result<(), String> {
        let mut state = self.0.lock().map_err(|_| "Herdr sharing is unavailable")?;
        if let Some(pick) = state.pick.take() {
            pick.cancelled.store(true, Ordering::Release);
        }
        state.generation += 1;
        state.notice = None;
        Ok(())
    }
    fn next(&self, session_key: Option<&str>) -> Option<Delivery> {
        let mut state = self.0.lock().ok()?;
        let pick = state.pick.as_mut()?;
        if session_key != Some(pick.session.key.as_str()) {
            let pick = state.pick.take()?;
            pick.cancelled.store(true, Ordering::Release);
            return Some(Delivery {
                generation: pick.generation,
                target: pick.terminal.address,
                text: pick.session.ended(),
                cancelled: Arc::new(AtomicBool::new(false)),
                ended: true,
            });
        }
        if pick.phase != "queued" {
            return None;
        }
        pick.phase = "sending".into();
        Some(Delivery {
            generation: pick.generation,
            target: pick.terminal.address.clone(),
            text: pick.session.pointer.clone(),
            cancelled: pick.cancelled.clone(),
            ended: false,
        })
    }
    fn finish(&self, delivery: &Delivery, result: Result<(), String>) {
        if let Ok(mut state) = self.0.lock() {
            if let Some(pick) = state
                .pick
                .as_mut()
                .filter(|p| p.generation == delivery.generation)
            {
                pick.phase = if result.is_ok() { "sent" } else { "failed" }.into();
            }
            if state.generation == delivery.generation {
                if let Err(error) = result {
                    if !delivery.cancelled.load(Ordering::Acquire) {
                        state.notice = Some(error);
                    }
                }
            }
        }
    }
}

fn deliver(hc: &Hc, delivery: &Delivery) -> Result<(), String> {
    if delivery.cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    match hc.get(&delivery.target)? {
        super::Lookup::Live(address) if address == delivery.target => {
            hc.send_pointer(&address, &delivery.text, &delivery.cancelled)
        }
        _ => Err(ENDED_TARGET.into()),
    }
}

pub(crate) fn start_observer(app: tauri::AppHandle) {
    let sharing = app.state::<Sharing>().inner().clone();
    let (sender, receiver) = mpsc::sync_channel::<Delivery>(4);
    let network_state = sharing.clone();
    let network_app = app.clone();
    // Dedicated network worker: no capture-owned locks, callbacks, joins or
    // channels. Full queues are rejected, never backpressure capture or Stop.
    std::thread::spawn(move || {
        while let Ok(delivery) = receiver.recv() {
            let result = Hc::discover().and_then(|hc| deliver(&hc, &delivery));
            network_state.finish(&delivery, result);
            let _ = network_app.emit_to("main", "herdr:live-changed", ());
        }
    });
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(250));
        // No filesystem or hc work without an explicit pick.
        if sharing.0.lock().is_ok_and(|s| s.pick.is_none()) {
            continue;
        }
        if let Some(delivery) = sharing.next(current_key().as_deref()) {
            let ended = delivery.ended;
            if let Err(
                mpsc::TrySendError::Full(delivery) | mpsc::TrySendError::Disconnected(delivery),
            ) = sender.try_send(delivery)
            {
                sharing.finish(
                    &delivery,
                    Err("Herdr delivery is busy; nothing was queued".into()),
                );
            }
            // Stop clears the pick before any network call, including when a
            // prior send hangs. Only this optional observer sees the transition.
            if ended {
                let _ = app.emit_to("main", "herdr:live-changed", ());
            }
        }
    });
}

#[tauri::command]
pub(crate) async fn cmd_herdr_live_view(app: tauri::AppHandle) -> Result<View, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let key = current_key();
        let ready = current_session().is_some_and(|s| Some(s.key.as_str()) == key.as_deref());
        app.state::<Sharing>().view(key.as_deref(), ready)
    })
    .await
    .map_err(|_| "Herdr sharing is unavailable")?
}
#[derive(Serialize)]
pub(crate) struct Options {
    session: String,
    generation: u64,
    terminals: Vec<Terminal>,
}
#[tauri::command]
pub(crate) async fn cmd_herdr_live_options(app: tauri::AppHandle) -> Result<Options, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let session =
            current_session().ok_or("Start a live recording before choosing a terminal")?;
        let generation = app.state::<Sharing>().generation()?;
        let hc = Hc::discover()?;
        if !hc.available() {
            return Err("Herdr is not available".into());
        }
        let terminals = hc.terminals()?;
        if current_session().is_none_or(|s| s.key != session.key) {
            return Err("The live recording changed; choose again".into());
        }
        Ok(Options {
            session: session.key,
            generation,
            terminals,
        })
    })
    .await
    .map_err(|_| "Herdr terminal listing stopped")?
}
#[tauri::command]
pub(crate) async fn cmd_herdr_live_pick(
    app: tauri::AppHandle,
    session: String,
    target: String,
    generation: u64,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !local_address(&target) {
            return Err("Only local Herdr terminals are supported".into());
        }
        let current = current_session()
            .filter(|s| s.key == session)
            .ok_or("The live recording changed; choose again")?;
        let terminal = Hc::discover()?.selected_terminal(&target)?;
        if current_session().is_none_or(|s| s.key != current.key) {
            return Err("The recording ended; nothing was sent".into());
        }
        app.state::<Sharing>().pick(current, terminal, generation)
    })
    .await
    .map_err(|_| "Herdr terminal selection stopped")?
}
#[tauri::command]
pub(crate) fn cmd_herdr_live_cancel(app: tauri::AppHandle) -> Result<(), String> {
    app.state::<Sharing>().cancel()
}
#[tauri::command]
pub(crate) async fn cmd_herdr_live_palette(app: tauri::AppHandle) -> Result<(), String> {
    let ready = tauri::async_runtime::spawn_blocking(|| {
        current_session().is_some() && Hc::discover().is_ok_and(|hc| hc.available())
    })
    .await
    .map_err(|_| "Herdr is not available")?;
    if !ready {
        return Err("Start a live recording with Herdr available first".into());
    }
    crate::show_main_window(&app);
    app.emit_to("main", "herdr:live-picker", ())
        .map_err(|_| "Could not open the terminal picker".into())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fake(body: &str) -> (tempfile::TempDir, Hc) {
        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("hc");
        std::fs::write(&executable, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        (
            dir,
            Hc {
                executable,
                timeout: Duration::from_millis(250),
            },
        )
    }
    fn status() -> SessionStatus {
        serde_json::from_value(serde_json::json!({"active":true,"pid":123,"session_id":"synthetic-session","jsonl_path":"/synthetic/live.jsonl","line_count":0,"duration_secs":0})).unwrap()
    }
    fn session() -> Session {
        let config = Config {
            output_dir: "/synthetic/meetings".into(),
            ..Config::default()
        };
        Session::from_status(&status(), &config).unwrap()
    }
    fn terminal() -> Terminal {
        Terminal {
            address: "t42".into(),
            name: "Picked only".into(),
            labels: BTreeMap::new(),
            agent: "codex".into(),
            location: "Development / Tests".into(),
        }
    }
    #[test]
    fn pointer_is_shared_with_recall_and_contains_no_meeting_contents() {
        let session = session();
        let section = crate::context::live_transcript_section_for_status(&status());
        assert!(section.contains(&session.pointer));
        assert!(section.contains(crate::context::LIVE_TRANSCRIPT_GUIDANCE));
        assert!(session
            .pointer
            .contains("minutes transcript --since N --include-current --format json"));
        assert!(session.pointer.contains("/synthetic/live.jsonl"));
        assert!(!session.ended().contains("live.jsonl"));
        assert!(session.ended().contains("/synthetic/meetings"));
        assert!(session.ended().contains("minutes jobs"));
        assert!(session.ended().contains("minutes list --limit 1"));
        let mut ended = status();
        ended.active = false;
        assert!(Session::from_status(&ended, &Config::default()).is_none());
        let mut anonymous = status();
        anonymous.session_id = None;
        assert!(Session::from_status(&anonymous, &Config::default()).is_none());
        for path in ["relative.jsonl", "/synthetic/not-a-transcript.md"] {
            let mut invalid = status();
            invalid.jsonl_path = Some(path.into());
            assert!(Session::from_status(&invalid, &Config::default()).is_none());
        }
    }
    #[test]
    fn standalone_live_without_context_tracking_uses_verified_relay_identity() {
        assert!(!Config::default().desktop_context.enabled);
        let mut status = status();
        status.session_id = None;
        status.pid = Some(std::process::id());
        let pointer = crate::context::live_transcript_pointer_with_relay(
            &status,
            Some("synthetic-relay-epoch"),
        )
        .unwrap();
        assert!(pointer.contains("Expected capture_relay.session_id: \"synthetic-relay-epoch\""));
        assert!(pointer.contains("minutes transcript --since N --include-current --format json"));
        assert!(pointer.contains("discard unavailable or mismatched identities"));
        assert!(!pointer.contains("auth_token"));
        assert!(crate::context::live_transcript_pointer_with_relay(&status, None).is_none());
        // A real transcript identity remains authoritative when both exist.
        status.session_id = Some("synthetic-transcript".into());
        let pointer = crate::context::live_transcript_pointer_with_relay(
            &status,
            Some("synthetic-relay-epoch"),
        )
        .unwrap();
        assert!(pointer.contains("Expected transcript_status.session_id: \"synthetic-transcript\""));
        assert!(!pointer.contains("synthetic-relay-epoch"));
    }
    #[test]
    fn local_live_listing_resolves_labels_without_returning_screen_or_cwd() {
        let (dir, hc) = fake(
            r#"
printf '%s\n' "$*" >> "$0.calls"
case "$1" in
ls) echo '{"hidden_finished":1,"terminals":[{"address":"t42","state":"live","agent":"codex","where":"Development / Tests"},{"address":"remote/t9","state":"live"},{"address":"t8","state":"finished"}]}' ;;
get) echo '{"address":"t42","name":"Recall handoff","state":"live","labels":{"minutes.tab":"synthetic","team":"QA","herdr.seen":"288","herdr.space":"PRIVATE","where":"fallback"},"screen":"PRIVATE","cwd":"PRIVATE"}' ;;
esac
"#,
        );
        let terminals = hc.terminals().unwrap();
        assert_eq!(terminals.len(), 1);
        assert_eq!(terminals[0].labels["minutes.tab"], "synthetic");
        assert_eq!(terminals[0].name, "Recall handoff");
        let json = serde_json::to_string(&terminals).unwrap();
        assert!(!json.contains("PRIVATE"));
        assert!(!json.contains("herdr.") && !json.contains("team"));
        assert_eq!(terminals[0].agent, "codex");
        assert_eq!(terminals[0].location, "Development / Tests");
        let calls = std::fs::read_to_string(dir.path().join("hc.calls")).unwrap();
        assert!(!calls.contains("remote/") && !calls.contains("t8"));
    }
    #[test]
    fn pointer_and_end_note_are_attempted_once_only_for_the_explicit_pick() {
        let (dir, hc) = fake(
            r#"
case "$1" in
ls) echo '{"terminals":[{"address":"t42","state":"live","agent":"codex"}]}' ;;
get) printf '{"address":"%s","state":"live"}\n' "$2" ;;
send) printf '%s\n%s\n' "$2" "$3" >> "$0.messages"; echo '{"started":null,"status":null,"turn":0}' ;;
esac
"#,
        );
        let sharing = Sharing::default();
        let session = session();
        assert!(sharing.next(Some(&session.key)).is_none());
        assert!(!dir.path().join("hc.messages").exists());
        sharing.pick(session.clone(), terminal(), 0).unwrap();
        let pointer = sharing.next(Some(&session.key)).unwrap();
        assert!(!pointer.ended);
        assert_eq!(pointer.text, session.pointer);
        let result = deliver(&hc, &pointer);
        sharing.finish(&pointer, result);
        assert_eq!(
            sharing
                .view(Some(&session.key), true)
                .unwrap()
                .phase
                .as_deref(),
            Some("sent")
        );
        assert!(sharing.next(Some(&session.key)).is_none());
        let end = sharing.next(None).unwrap();
        assert!(end.ended);
        assert!(
            sharing.0.lock().unwrap().pick.is_none(),
            "Stop drops the pick before network work"
        );
        assert!(sharing.next(None).is_none());
        deliver(&hc, &end).unwrap();
        let messages = std::fs::read_to_string(dir.path().join("hc.messages")).unwrap();
        assert_eq!(messages.matches("t42\n").count(), 2);
        assert_eq!(messages.matches("Live transcript JSONL:").count(), 1);
        assert_eq!(messages.matches("Recording ended.").count(), 1);
        assert!(!messages.contains("t43"));
    }
    #[test]
    fn cancel_fences_pending_picks_and_late_delivery_and_does_not_reattach() {
        let sharing = Sharing::default();
        let session = session();
        sharing.cancel().unwrap();
        assert!(sharing.pick(session.clone(), terminal(), 0).is_err());
        sharing.pick(session.clone(), terminal(), 1).unwrap();
        let pointer = sharing.next(Some(&session.key)).unwrap();
        sharing.cancel().unwrap();
        assert!(pointer.cancelled.load(Ordering::Acquire));
        let (dir, hc) = fake("touch \"$0.called\"; echo '{}'");
        deliver(&hc, &pointer).unwrap();
        assert!(!dir.path().join("hc.called").exists());
        assert!(sharing.next(Some("next-recording")).is_none());
        sharing
            .pick(session.clone(), terminal(), sharing.generation().unwrap())
            .unwrap();
        let ended = sharing.next(Some("next-recording")).unwrap();
        assert!(ended.ended);
        assert!(sharing
            .view(Some(&session.key), true)
            .unwrap()
            .selected
            .is_none());
    }
    #[test]
    fn full_delivery_queue_never_backpressures_stop_or_restores_the_pick() {
        let sharing = Sharing::default();
        let session = session();
        let (sender, _receiver) = mpsc::sync_channel(1);
        sharing.pick(session.clone(), terminal(), 0).unwrap();
        sender
            .try_send(sharing.next(Some(&session.key)).unwrap())
            .unwrap_or_else(|_| panic!("empty queue"));
        let start = Instant::now();
        let ended = sharing.next(None).unwrap();
        let Err(mpsc::TrySendError::Full(delivery)) = sender.try_send(ended) else {
            panic!("queue must stay full")
        };
        sharing.finish(&delivery, Err("Delivery queue is full".into()));
        assert!(start.elapsed() < Duration::from_millis(100));
        assert!(sharing.0.lock().unwrap().pick.is_none());
    }
    #[test]
    fn capture_lifetime_survives_sidecar_failure_and_distinguishes_reused_pids() {
        let dir = tempfile::tempdir().unwrap();
        let recording = dir.path().join("recording.pid");
        let live = dir.path().join("live.pid");
        std::fs::write(&recording, std::process::id().to_string()).unwrap();
        let (key, modified) = capture_identity_at(&recording, &live).unwrap();
        let mut session = session();
        session.key = key.clone();
        let sharing = Sharing::default();
        sharing.pick(session.clone(), terminal(), 0).unwrap();
        assert!(sharing.next(Some(&key)).is_some());
        // Failed/stale sidecar status is not Stop. The producer is still alive,
        // its version is unchanged, and the existing pick remains cancellable.
        let mut failed = status();
        failed.active = false;
        failed.pid = None;
        assert!(Session::from_status(&failed, &Config::default()).is_none());
        let (still_running, _) = capture_identity_at(&recording, &live).unwrap();
        assert_eq!(still_running, key);
        assert!(sharing.next(Some(&still_running)).is_none());
        assert!(sharing
            .view(Some(&still_running), false)
            .unwrap()
            .selected
            .is_some());
        assert!(sharing.view(Some(&still_running), false).unwrap().active);
        // A real Stop removes the PID file and drops the pick exactly once.
        std::fs::remove_file(&recording).unwrap();
        assert!(capture_identity_at(&recording, &live).is_none());
        assert!(sharing.next(None).unwrap().ended);
        // New capture, same app PID and fixed JSONL path: a different file
        // version cannot inherit the old pick, even with context tracking off.
        std::fs::write(&recording, std::process::id().to_string()).unwrap();
        std::fs::File::open(&recording)
            .unwrap()
            .set_modified(modified + Duration::from_secs(1))
            .unwrap();
        let (next, _) = capture_identity_at(&recording, &live).unwrap();
        assert_ne!(next, key);
        assert!(sharing.next(Some(&next)).is_none());
    }
    #[test]
    fn shells_missing_and_unknown_agents_are_not_listed_or_pickable() {
        for agent in ["zsh", "bash", "sh", "python3", "cat", "", "unknown"] {
            let (dir, hc) = fake(&format!(
                r#"
case "$1" in
ls) echo '{{"terminals":[{{"address":"t42","state":"live","agent":"{agent}"}}]}}';;
*) touch "$0.unexpected";;
esac
"#
            ));
            assert!(hc.terminals().unwrap().is_empty(), "{agent}");
            assert_eq!(hc.selected_terminal("t42").unwrap_err(), UNSUPPORTED_TARGET);
            assert!(hc
                .send_pointer("t42", "synthetic", &AtomicBool::new(false))
                .is_err());
            assert!(!dir.path().join("hc.unexpected").exists());
        }
        let (_dir, hc) = fake("echo '{\"terminals\":[{\"address\":\"t42\",\"state\":\"live\"}]}'");
        assert!(hc.terminals().unwrap().is_empty());
        assert_eq!(hc.selected_terminal("t42").unwrap_err(), UNSUPPORTED_TARGET);
        for body in [
            "echo '{\"terminals\":[]}'",
            "echo '{\"terminals\":[{\"address\":\"t42\",\"state\":\"finished\",\"agent\":\"codex\"}]}'",
        ] {
            let (_dir, hc) = fake(body);
            assert_eq!(hc.selected_terminal("t42").unwrap_err(), ENDED_TARGET);
        }
    }
    #[test]
    fn agent_returning_to_a_shell_after_pick_cannot_receive_pointer_or_end_note() {
        let (dir, hc) = fake(
            r#"
case "$1" in
ls) if test -f "$0.shell"; then agent=zsh; else agent=claude-code; fi
    printf '{"terminals":[{"address":"t42","state":"live","agent":"%s"}]}' "$agent";;
get) if test -f "$0.flip"; then touch "$0.shell"; fi
     echo '{"address":"t42","name":"Agent","state":"live"}';;
send) touch "$0.sent"; echo '{"turn":0}';;
esac
"#,
        );
        let picked = hc.selected_terminal("t42").unwrap();
        assert_eq!(picked.agent, "claude-code");
        let sharing = Sharing::default();
        let session = session();
        sharing.pick(session.clone(), picked, 0).unwrap();
        // It was eligible at pick time; fresh ls, after get, must refuse it now.
        std::fs::write(dir.path().join("hc.flip"), "").unwrap();
        let pointer = sharing.next(Some(&session.key)).unwrap();
        assert_eq!(deliver(&hc, &pointer).unwrap_err(), UNSUPPORTED_TARGET);
        let ended = sharing.next(None).unwrap();
        assert_eq!(deliver(&hc, &ended).unwrap_err(), UNSUPPORTED_TARGET);
        assert!(!dir.path().join("hc.sent").exists());
    }
    #[test]
    fn missing_dead_rejected_hung_and_invalid_servers_cannot_redirect_or_retry() {
        for body in [
            "echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3",
            "echo '{\"address\":\"t42\",\"state\":\"finished\"}'",
            "echo '{\"address\":\"t43\",\"state\":\"live\"}'",
            "exec /bin/sleep 30",
            "echo invalid",
            r#"case "$1" in ls) echo '{"terminals":[{"address":"t42","state":"live","agent":"codex"}]}';; get) echo '{"address":"t42","state":"live"}';; send) echo '{"error":{"kind":"blocked","message":"PRIVATE"}}' >&2; exit 1;; esac"#,
        ] {
            let (_dir, hc) = fake(body);
            let sharing = Sharing::default();
            let session = session();
            sharing.pick(session.clone(), terminal(), 0).unwrap();
            let delivery = sharing.next(Some(&session.key)).unwrap();
            let start = Instant::now();
            let result = deliver(&hc, &delivery);
            assert!(result.is_err());
            assert!(!result.as_ref().unwrap_err().contains("PRIVATE"));
            assert!(start.elapsed() < Duration::from_secs(1));
            sharing.finish(&delivery, result);
            assert!(
                sharing.next(Some(&session.key)).is_none(),
                "Ambiguous sends must not be retried"
            );
            assert_eq!(
                sharing
                    .view(Some(&session.key), true)
                    .unwrap()
                    .phase
                    .as_deref(),
                Some("failed")
            );
        }
        let (dir, hc) = fake("touch \"$0.called\"");
        assert!(hc
            .send_pointer("remote/t42", "synthetic", &AtomicBool::new(false))
            .is_err());
        assert!(!dir.path().join("hc.called").exists());
    }

    pub(super) fn assert_isolation(mut capture_controls: impl FnMut()) {
        for hangs in [true, false] {
            let body = if hangs {
                "exec /bin/sleep 30"
            } else {
                "echo '{\"error\":{\"kind\":\"blocked\"}}' >&2; exit 1"
            };
            let (dir, mut hc) = fake(&format!(
                r#"case "$1" in ls) echo '{{"terminals":[{{"address":"t42","state":"live","agent":"codex"}}]}}';; get) echo '{{"address":"t42","state":"live"}}';; send) touch "$0.started"; {body};; esac"#
            ));
            hc.timeout = Duration::from_secs(2);
            let sharing = Sharing::default();
            let session = session();
            sharing.pick(session.clone(), terminal(), 0).unwrap();
            let delivery = sharing.next(Some(&session.key)).unwrap();
            let worker_state = sharing.clone();
            let done = Arc::new(AtomicBool::new(false));
            let worker_done = done.clone();
            let worker = std::thread::spawn(move || {
                let result = deliver(&hc, &delivery);
                worker_state.finish(&delivery, result);
                worker_done.store(true, Ordering::Release);
            });
            let wait = Instant::now();
            while !dir.path().join("hc.started").exists() && wait.elapsed() < Duration::from_secs(1)
            {
                std::thread::sleep(Duration::from_millis(2));
            }
            assert!(dir.path().join("hc.started").exists());
            if hangs {
                assert!(!done.load(Ordering::Acquire));
            }
            let start = Instant::now();
            capture_controls();
            let end = sharing.next(None).unwrap();
            assert!(end.ended);
            assert!(sharing.0.lock().unwrap().pick.is_none());
            assert!(
                start.elapsed() < Duration::from_millis(500),
                "Capture controls/WAV/processing admission cannot wait for hc"
            );
            worker.join().unwrap();
        }
    }
}
#[cfg(all(test, unix))]
pub(crate) fn assert_capture_isolation(controls: impl FnMut()) {
    tests::assert_isolation(controls);
}
