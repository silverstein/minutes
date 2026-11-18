//! Optional local Herdr edge. No capture/pipeline caller depends on this module.
use crate::recall_tabs::{Tab, TabStore};
use minutes_core::config::Config;
use serde::Deserialize;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

#[path = "herdr_live.rs"]
pub(crate) mod live;

static TRANSITION: Mutex<()> = Mutex::new(());
const DEADLINE: Duration = Duration::from_secs(5);
const HC_NOT_FOUND: &str = "hc was not found on the assistant PATH";
const NESTED_ENV: [&str; 4] = [
    "CLAUDECODE",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_PID",
];

fn transition_guard() -> Result<MutexGuard<'static, ()>, String> {
    TRANSITION
        .try_lock()
        .map_err(|_| "A Recall handoff is already in progress".into())
}

#[derive(Default, Deserialize)]
struct EdgeConfig {
    #[serde(default)]
    herdr: HerdrConfig,
}
#[derive(Default, Deserialize)]
struct HerdrConfig {
    #[serde(default)]
    enabled: Enabled,
}
#[derive(Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
enum Enabled {
    #[default]
    Auto,
    Off,
}
fn enabled_at(path: &Path) -> bool {
    match std::fs::read_to_string(path) {
        Ok(raw) => toml::from_str::<EdgeConfig>(&raw)
            .is_ok_and(|config| config.herdr.enabled == Enabled::Auto),
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

/// Injectable executable and deadline; tests use an isolated PATH and fake hc.
struct Hc {
    executable: PathBuf,
    timeout: Duration,
}
#[derive(Debug)]
struct LaunchError {
    message: String,
    uncertain: bool,
}
impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
#[derive(Debug)]
enum Lookup {
    Missing,
    Live(String),
    Finished,
}
impl Hc {
    fn discover() -> Result<Self, String> {
        if !enabled_at(&Config::config_path()) {
            return Err("Herdr integration is turned off or its configuration is invalid".into());
        }
        Self::locate()
    }
    fn locate() -> Result<Self, String> {
        Self::locate_in(
            &crate::pty::assistant_process_environment(),
            &std::env::current_dir().map_err(|e| e.to_string())?,
        )
    }
    fn locate_in(environment: &[(String, String)], cwd: &Path) -> Result<Self, String> {
        let path = environment
            .iter()
            .find(|(key, _)| key == "PATH")
            .ok_or("Assistant PATH is unavailable")?;
        let executable = which::which_in("hc", Some(&path.1), cwd).map_err(|_| HC_NOT_FOUND)?;
        Ok(Self {
            executable,
            timeout: DEADLINE,
        })
    }
    fn call(&self, args: &[String]) -> Result<(bool, serde_json::Value), String> {
        self.call_cancellable(args, &std::sync::atomic::AtomicBool::new(false))
    }
    fn call_cancellable(
        &self,
        args: &[String],
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<(bool, serde_json::Value), String> {
        if cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("Herdr delivery was cancelled".into());
        }
        // Anonymous files avoid pipe deadlocks and never expose environment values.
        let mut stdout = tempfile::tempfile().map_err(|e| e.to_string())?;
        let mut stderr = tempfile::tempfile().map_err(|e| e.to_string())?;
        let mut command = Command::new(&self.executable);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(stdout.try_clone().map_err(|e| e.to_string())?)
            .stderr(stderr.try_clone().map_err(|e| e.to_string())?);
        for name in NESTED_ENV {
            command.env_remove(name);
        }
        let mut child = command
            .spawn()
            .map_err(|e| format!("Could not start hc: {e}"))?;
        let start = Instant::now();
        let success = loop {
            if cancelled.load(std::sync::atomic::Ordering::Acquire) {
                let _ = child.kill();
                let _ = child.wait();
                return Err("Herdr delivery was cancelled".into());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status.success(),
                Ok(None) if start.elapsed() < self.timeout => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                result => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(match result {
                        Err(error) => format!("Could not inspect hc: {error}"),
                        _ => "Herdr did not answer before the timeout".into(),
                    });
                }
            }
        };
        let mut bytes = Vec::new();
        stdout.rewind().map_err(|e| e.to_string())?;
        stdout
            .take(65_537)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.len() > 65_536 {
            return Err("Herdr returned too much metadata".into());
        }
        if !success && bytes.is_empty() {
            stderr.rewind().map_err(|e| e.to_string())?;
            stderr
                .take(65_537)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            if bytes.len() > 65_536 {
                return Err("Herdr returned too much metadata".into());
            }
        }
        let json: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|_| "Herdr returned invalid metadata")?;
        if success {
            Ok((true, json))
        } else {
            // Errors in hc 0.1.21 use stderr. Consume only the bounded kind;
            // never retain or display arbitrary error messages, hints or context.
            let kind = json["error"]["kind"]
                .as_str()
                .ok_or("Herdr returned invalid error metadata")?;
            Ok((false, serde_json::json!({"error": {"kind": kind}})))
        }
    }
    fn available(&self) -> bool {
        self.call(&["server".into(), "status".into(), "--json".into()])
            .is_ok_and(|(ok, json)| {
                ok && json["core"]["id"].is_string() && json["version"].is_string()
            })
    }
    fn get(&self, target: &str) -> Result<Lookup, String> {
        if !local_address(target) {
            return Err("Only local Herdr terminals are supported".into());
        }
        let (ok, json) = self.call(&["get".into(), target.into(), "--json".into()])?;
        if !ok && json["error"]["kind"] == "not_found" {
            return Ok(Lookup::Missing);
        }
        if !ok {
            return Err("Could not inspect the Herdr terminal".into());
        }
        let address = json["address"]
            .as_str()
            .filter(|a| local_address(a))
            .ok_or("Herdr returned an invalid local address")?
            .to_string();
        match json["state"].as_str() {
            Some("live") => Ok(Lookup::Live(address)),
            Some("finished") => Ok(Lookup::Finished),
            _ => Err("Herdr returned an unknown terminal state".into()),
        }
    }
    fn run(
        &self,
        name: &str,
        tab: &Tab,
        workspace: &Path,
        agent: &Path,
        args: Vec<String>,
    ) -> Result<String, LaunchError> {
        let mut argv: Vec<String> = [
            "run",
            "--json",
            "--bg",
            "--no-parent",
            "--outlive-parent",
            "--cwd",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        argv.extend([
            workspace.to_string_lossy().into_owned(),
            "--name".into(),
            name.into(),
            "--label".into(),
            format!("minutes.tab={}", tab.id),
        ]);
        if let Some(path) = &tab.meeting_path {
            if let Some(slug) = Path::new(path).file_stem().and_then(|s| s.to_str()) {
                argv.extend(["--label".into(), format!("minutes.meeting={slug}")]);
            }
        }
        // `env -u` runs inside the server's terminal, where inherited variables matter.
        argv.extend(["--".into(), "env".into()]);
        for name in NESTED_ENV {
            argv.extend(["-u".into(), name.into()]);
        }
        // Carry the same local CLI discovery environment as a Minutes PTY.
        argv.extend(
            crate::pty::assistant_process_environment()
                .into_iter()
                .map(|(k, v)| format!("{k}={v}")),
        );
        argv.push(format!("{}={}", crate::recall_tabs::TAB_ENV, tab.id));
        argv.push(agent.to_string_lossy().into_owned());
        argv.extend(args);
        let (ok, json) = self.call(&argv).map_err(|message| LaunchError {
            message,
            uncertain: true,
        })?;
        if !ok {
            return Err(LaunchError {
                message: "Herdr could not start the conversation".into(),
                uncertain: false,
            });
        }
        json["address"]
            .as_str()
            .filter(|address| local_address(address))
            .map(str::to_string)
            .ok_or_else(|| LaunchError {
                message: "Herdr did not return a local terminal address".into(),
                uncertain: true,
            })
    }
    fn kill(&self, target: &str) -> Result<(), String> {
        if !local_address(target) {
            return Err("Only local Herdr terminals are supported".into());
        }
        let (ok, _) = self.call(&["kill".into(), target.into(), "--json".into()])?;
        if ok {
            Ok(())
        } else {
            Err("Herdr could not end the terminal".into())
        }
    }
}
fn local_address(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
fn terminal_name(id: &str) -> String {
    if id.len() <= 24 {
        return format!("minutes-{id}");
    }
    // Recall IDs fit today; a hash keeps future longer IDs inside hc's 32-byte limit.
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hash);
    format!("minutes-{:016x}", hash.finish())
}

#[tauri::command]
pub async fn cmd_recall_herdr_available() -> bool {
    tauri::async_runtime::spawn_blocking(|| Hc::discover().is_ok_and(|hc| hc.available()))
        .await
        .unwrap_or(false)
}

fn emit(app: &tauri::AppHandle, tabs: &TabStore) {
    let _ = app.emit_to("main", "recall:tabs-changed", tabs);
}
fn set_owner(app: &tauri::AppHandle, id: &str, owner: Option<String>) -> Result<TabStore, String> {
    let state = app.state::<crate::commands::AppState>();
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    set_tab_owner(
        &mut manager.tabs,
        &crate::context::workspace_dir(),
        id,
        owner,
    )?;
    let tabs = manager.tabs.clone();
    drop(manager);
    emit(app, &tabs);
    Ok(tabs)
}
fn set_tab_owner(
    tabs: &mut TabStore,
    root: &Path,
    id: &str,
    owner: Option<String>,
) -> Result<(), String> {
    let previous = tabs.clone();
    let tab = tabs
        .tabs
        .iter_mut()
        .find(|t| t.id == id)
        .ok_or("Recall tab not found")?;
    tab.herdr_terminal = owner;
    tab.handed_off_at = tab
        .herdr_terminal
        .as_ref()
        .map(|_| chrono::Utc::now().to_rfc3339());
    tab.process_exited = false;
    if let Err(error) = tabs.save(root) {
        *tabs = previous;
        return Err(error);
    }
    Ok(())
}
fn restore_local(app: &tauri::AppHandle, id: &str, theme: Option<bool>) -> Result<(), String> {
    let state = app.state::<crate::commands::AppState>();
    // A pre-run failure cannot have transferred ownership. Repair this tab's
    // persisted fence even if the failed operation poisoned the manager lock.
    let mut manager = state
        .pty_manager
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    set_tab_owner(
        &mut manager.tabs,
        &crate::context::workspace_dir(),
        id,
        None,
    )?;
    state.pty_manager.clear_poison();
    let tabs = manager.tabs.clone();
    drop(manager);
    emit(app, &tabs);
    restart(app, id, theme)
}
fn finish_preparation<T>(
    result: Result<T, String>,
    restore: impl FnOnce() -> Result<(), String>,
) -> Result<T, String> {
    result.map_err(|error| match restore() {
        Ok(()) => format!("{error}. The conversation has restarted in Minutes."),
        Err(recovery) => format!("{error}. Could not restore the local tab: {recovery}"),
    })
}
fn ensure_idle(tab: &Tab) -> Result<(), String> {
    if tab.status == crate::recall_tabs::Status::Working {
        Err("Wait for this reply to finish, then move it to Herdr".into())
    } else {
        Ok(())
    }
}
fn restart(app: &tauri::AppHandle, id: &str, theme: Option<bool>) -> Result<(), String> {
    let state = app.state::<crate::commands::AppState>();
    crate::commands::spawn_terminal_tab(
        app,
        &state.pty_manager,
        "deferred",
        None,
        None,
        theme,
        None,
        Some(id),
        false,
        true,
    )
    .map(|_| ())
}

#[tauri::command]
pub async fn cmd_recall_herdr_open(
    app: tauri::AppHandle,
    tab_id: String,
    confirmed: bool,
) -> Result<TabStore, String> {
    tauri::async_runtime::spawn_blocking(move || open(&app, &tab_id, confirmed))
        .await
        .map_err(|e| e.to_string())?
}
fn open(app: &tauri::AppHandle, id: &str, confirmed: bool) -> Result<TabStore, String> {
    let _transition = transition_guard()?;
    if !confirmed {
        return Err("Confirm moving this conversation to Herdr".into());
    }
    let hc = Hc::discover()?;
    if !hc.available() {
        return Err("Herdr is not available".into());
    }
    let config = Config::load_strict()?;
    let state = app.state::<crate::commands::AppState>();
    let root = crate::context::workspace_dir();
    let (tab, theme) = {
        let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
        manager.tabs.ensure(&root)?;
        manager.tabs.refresh(&root)?;
        let tab = manager
            .tabs
            .tabs
            .iter()
            .find(|t| t.id == id)
            .ok_or("Recall tab not found")?
            .clone();
        if tab.herdr_terminal.is_some() {
            return Err("This conversation is already in Herdr".into());
        }
        ensure_idle(&tab)?;
        (tab, manager.session_theme_dark(id))
    };
    let workspace = crate::recall_tabs::workspace(id)?;
    let (agent_name, _) = launch_spec(&config, &tab, &root, &workspace);
    // Resolve before stopping the local process, including Claude inside a shell.
    crate::commands::resolve_agent_binary(&agent_name).map_err(|e| e.user_message())?;
    let mut name = terminal_name(id);
    match hc.get(&name)? {
        Lookup::Live(_) => return Err("A Herdr terminal already owns this conversation".into()),
        Lookup::Finished => {
            // Never reuse a finished record: a timed-out launch must not be
            // mistaken for the previous handoff when Take back checks state.
            name = format!(
                "minutes-{}",
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
            );
            if !matches!(hc.get(&name)?, Lookup::Missing) {
                return Err("Could not reserve a unique Herdr name".into());
            }
        }
        Lookup::Missing => {}
    }
    // Write-ahead ownership fences every spawn route, including relaunch.
    {
        let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
        manager.tabs.refresh(&root)?;
        ensure_idle(
            manager
                .tabs
                .tabs
                .iter()
                .find(|t| t.id == id)
                .ok_or("Recall tab not found")?,
        )?;
        set_tab_owner(&mut manager.tabs, &root, id, Some(name.clone()))?;
        emit(app, &manager.tabs);
    }
    // Everything fallible before run is inside this recovery envelope.
    let prepared = (|| {
        let session = state
            .pty_manager
            .lock()
            .map_err(|_| "Recall unavailable")?
            .take_session(id);
        if let Some(session) = session {
            if let Err((session, error)) = stop_and_reap(session) {
                // Keep the original handle so every local spawn still sees its owner.
                state
                    .pty_manager
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .restore_session(id.to_string(), *session);
                return Err(error);
            }
        }
        // Re-read transcript metadata after reaping: the last local turn may just have flushed.
        let mut latest = tab;
        {
            let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
            manager.tabs.refresh(&root)?;
            if let Some(saved) = manager.tabs.tabs.iter().find(|t| t.id == id) {
                latest = saved.clone();
            }
        }
        let (agent_name, args) = launch_spec(&config, &latest, &root, &workspace);
        let agent =
            crate::commands::resolve_agent_binary(&agent_name).map_err(|e| e.user_message())?;
        let cwd = crate::recall_tabs::launch_directory(&root, &workspace, &agent_name);
        crate::recall_tabs::settings(&root)?;
        Ok((latest, agent, args, cwd))
    })();
    let (latest, agent, args, cwd) =
        finish_preparation(prepared, || restore_local(app, id, theme))?;
    match hc.run(&name, &latest, &cwd, &agent, args) {
        Ok(address) => set_owner(app, id, Some(address)),
        Err(error) => {
            match reconcile_launch(&hc, &name, &error) {
                Recovery::Owned(address) => { set_owner(app, id, Some(address))?; Err(format!("{error}. The conversation is in Herdr; its address was recovered.")) }
                Recovery::Restart => {
                    set_owner(app, id, None)?;
                    restart(app, id, theme).map_err(|e| format!("{error}. Could not restart the tab: {e}"))?;
                    Err(format!("{error}. The conversation has restarted in Minutes."))
                }
                Recovery::Uncertain => Err(format!("{error}. Ownership could not be checked. Use Take back after Herdr answers; Minutes will not start a second copy.")),
            }
        }
    }
}

fn launch_spec(config: &Config, tab: &Tab, root: &Path, workspace: &Path) -> (String, Vec<String>) {
    let resume = tab.claude_session_id.as_deref().filter(|id| {
        crate::recall_tabs::transcript_available(
            root,
            &tab.id,
            id,
            tab.claude_transcript_path.as_deref(),
        )
    });
    let configured_claude = crate::commands::command_is_claude(&config.assistant.agent);
    let (agent, args) = if resume.is_some() && !configured_claude {
        // A shell's flags are not Claude flags. Resume the conversation launched
        // inside it using the same Claude resolver as Recall's session tooling.
        ("claude", Vec::new())
    } else {
        (
            config.assistant.agent.as_str(),
            crate::commands::filtered_agent_args(
                &config.assistant.agent,
                &config.assistant.agent_args,
            ),
        )
    };
    let mut args = crate::context::assistant_skills::terminal_args(agent, args, workspace);
    if crate::commands::command_is_claude(agent) {
        args = crate::commands::recall_tab_claude_args(args, workspace, resume);
    }
    (agent.to_string(), args)
}

#[derive(Debug, PartialEq)]
enum Recovery {
    Owned(String),
    Restart,
    Uncertain,
}
fn reconcile_launch(hc: &Hc, name: &str, error: &LaunchError) -> Recovery {
    match hc.get(name) {
        Ok(Lookup::Live(address)) => Recovery::Owned(address),
        Ok(Lookup::Missing | Lookup::Finished) if !error.uncertain => Recovery::Restart,
        _ => Recovery::Uncertain,
    }
}
fn release_terminal(hc: &Hc, target: &str, end: bool, confirm_missing: bool) -> Result<(), String> {
    match hc.get(target)? {
        Lookup::Missing if confirm_missing => Ok(()),
        Lookup::Missing => Err("RECALL_HERDR_MISSING_CONFIRM".into()),
        Lookup::Finished => Ok(()),
        Lookup::Live(_) if !end => Err("RECALL_HERDR_END_CONFIRM".into()),
        Lookup::Live(_) => {
            hc.kill(target)?;
            if matches!(hc.get(target)?, Lookup::Finished) {
                Ok(())
            } else {
                Err("The Herdr terminal has not confirmed its exit. Minutes did not start another copy.".into())
            }
        }
    }
}

fn take_back_with<T>(
    hc: Result<&Hc, String>,
    target: &str,
    end: bool,
    confirm_missing: bool,
    reclaim: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    match hc {
        Ok(hc) => release_terminal(hc, target, end, confirm_missing)?,
        Err(error) if error == HC_NOT_FOUND => {
            if !confirm_missing {
                return Err("RECALL_HERDR_MISSING_CONFIRM".into());
            }
        }
        Err(error) => return Err(error),
    }
    reclaim()
}

/// Unlike normal shutdown, handoff must establish child exit before hc run.
fn stop_and_reap(
    mut session: crate::pty::PtySession,
) -> Result<(), (Box<crate::pty::PtySession>, String)> {
    #[cfg(unix)]
    {
        // The foreground group can differ from the original shell's group.
        // Both are confined to this PTY; kill them before reaping its leader.
        let mut groups = vec![session.master.process_group_leader()];
        groups.push(
            session
                .child
                .process_id()
                .and_then(|pid| i32::try_from(pid).ok()),
        );
        for group in groups.into_iter().flatten().filter(|pid| *pid > 1) {
            if group == unsafe { libc::getpgrp() } {
                return Err((
                    Box::new(session),
                    "Refusing to stop a shared process group".into(),
                ));
            }
            let result = unsafe { libc::killpg(group, libc::SIGKILL) };
            if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
                return Err((
                    Box::new(session),
                    "Could not stop this tab's process group".into(),
                ));
            }
        }
    }
    #[cfg(windows)]
    if let Some(pid) = session.child.process_id() {
        let mut killer = match Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                return Err((
                    Box::new(session),
                    "Could not stop the conversation process tree".into(),
                ))
            }
        };
        let start = Instant::now();
        while killer.try_wait().ok().flatten().is_none() {
            if start.elapsed() >= DEADLINE {
                let _ = killer.kill();
                let _ = killer.wait();
                return Err((
                    Box::new(session),
                    "Stopping the conversation process tree timed out".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if let Err(error) = session.child.kill() {
        // An already-exited child is safe once reaped; an unknown child stays owned.
        if session.child.try_wait().ok().flatten().is_some() {
            return Ok(());
        }
        return Err((
            Box::new(session),
            format!("Could not stop the conversation: {error}"),
        ));
    }
    let start = Instant::now();
    loop {
        match session.child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if start.elapsed() < DEADLINE => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                return Err((Box::new(session), "The conversation has not exited. Herdr was not started; keep this tab until its process is stopped.".into()));
            }
        }
    }
}

#[tauri::command]
pub async fn cmd_recall_herdr_take_back(
    app: tauri::AppHandle,
    tab_id: String,
    end_terminal: bool,
    confirm_missing: Option<bool>,
) -> Result<TabStore, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _transition = transition_guard()?;
        let hc = Hc::locate();
        let state = app.state::<crate::commands::AppState>();
        let target = state
            .pty_manager
            .lock()
            .map_err(|_| "Recall unavailable")?
            .tabs
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .and_then(|t| t.herdr_terminal.clone())
            .ok_or("This tab is not in Herdr")?;
        take_back_with(
            hc.as_ref().map_err(Clone::clone),
            &target,
            end_terminal,
            confirm_missing.unwrap_or(false),
            || {
                set_owner(&app, &tab_id, None)?;
                restart(&app, &tab_id, None)?;
                let tabs = state
                    .pty_manager
                    .lock()
                    .map_err(|_| "Recall unavailable")?
                    .tabs
                    .clone();
                Ok(tabs)
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Palette selection opens the same confirmation in Recall; it never bypasses it.
#[tauri::command]
pub async fn cmd_recall_herdr_palette(app: tauri::AppHandle) -> Result<(), String> {
    if !cmd_recall_herdr_available().await {
        return Err("Herdr is not available".into());
    }
    let state = app.state::<crate::commands::AppState>();
    let manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    let id = manager
        .tabs
        .active
        .clone()
        .ok_or("Open a Recall tab first")?;
    if manager
        .tabs
        .tabs
        .iter()
        .find(|t| t.id == id)
        .is_none_or(|t| t.herdr_terminal.is_some())
    {
        return Err("Choose a conversation that is in Minutes".into());
    }
    drop(manager);
    crate::show_main_window(&app);
    app.emit_to("main", "recall:herdr-confirm", id)
        .map_err(|e| e.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn fake(body: &str) -> (tempfile::TempDir, Hc) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hc");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        // An isolated PATH lookup exercises presence without changing global env.
        let executable = which::which_in("hc", Some(dir.path()), dir.path()).unwrap();
        (
            dir,
            Hc {
                executable,
                timeout: Duration::from_millis(150),
            },
        )
    }
    fn tab() -> Tab {
        serde_json::from_value(serde_json::json!({
            "id":"tab-123", "title":"Synthetic conversation", "meeting_path":"/synthetic/board.md", "claude_session_id": null
        })).unwrap()
    }
    #[test]
    fn gui_lookup_uses_the_augmented_assistant_path() {
        let (dir, _) = fake("echo '{}'");
        let gui = "/usr/bin:/bin:/usr/sbin:/sbin";
        assert!(which::which_in("hc", Some(gui), dir.path()).is_err());
        let path =
            std::env::join_paths([dir.path(), Path::new("/usr/bin"), Path::new("/bin")]).unwrap();
        let hc = Hc::locate_in(
            &[("PATH".into(), path.to_string_lossy().into_owned())],
            dir.path(),
        )
        .unwrap();
        assert_eq!(hc.executable, dir.path().join("hc"));
        assert!(Hc::locate_in(&[("PATH".into(), gui.into())], dir.path()).is_err());
    }
    #[test]
    fn missing_owner_requires_its_own_confirmation_then_clears_the_saved_fence() {
        let (dir, hc) = fake("echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3");
        let mut tabs = TabStore::default();
        let id = tabs.open(dir.path(), None, true).unwrap();
        set_tab_owner(&mut tabs, dir.path(), &id, Some("t42".into())).unwrap();
        let blocked = take_back_with(Ok(&hc), "t42", true, false, || {
            set_tab_owner(&mut tabs, dir.path(), &id, None)
        });
        assert_eq!(blocked.unwrap_err(), "RECALL_HERDR_MISSING_CONFIRM");
        assert!(
            tabs.tabs[0].herdr_terminal.is_some(),
            "End confirmation is not missing-owner confirmation"
        );
        let mut restarts = 0;
        take_back_with(Ok(&hc), "t42", false, true, || {
            set_tab_owner(&mut tabs, dir.path(), &id, None)?;
            restarts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(restarts, 1);
        let mut restored = TabStore::default();
        restored.ensure(dir.path()).unwrap();
        assert!(restored.tabs[0].herdr_terminal.is_none());
        assert!(restored.tabs[0].handed_off_at.is_none());
    }
    #[test]
    fn uninstalled_hc_requires_missing_confirmation_before_reclaiming() {
        let dir = tempfile::tempdir().unwrap();
        let locate = || {
            Hc::locate_in(
                &[("PATH".into(), dir.path().to_string_lossy().into_owned())],
                dir.path(),
            )
        };
        let mut tabs = TabStore::default();
        let id = tabs.open(dir.path(), None, true).unwrap();
        set_tab_owner(&mut tabs, dir.path(), &id, Some("t42".into())).unwrap();
        let mut restarts = 0;
        for (discovery, end, confirm, expected) in [
            (locate(), false, false, "RECALL_HERDR_MISSING_CONFIRM"),
            (locate(), true, false, "RECALL_HERDR_MISSING_CONFIRM"),
            (
                Err("Other discovery failure".into()),
                false,
                true,
                "Other discovery failure",
            ),
        ] {
            let result = take_back_with(
                discovery.as_ref().map_err(Clone::clone),
                "t42",
                end,
                confirm,
                || {
                    restarts += 1;
                    set_tab_owner(&mut tabs, dir.path(), &id, None)
                },
            );
            assert_eq!(result.unwrap_err(), expected);
            assert_eq!(restarts, 0);
            assert!(tabs.tabs[0].herdr_terminal.is_some());
        }
        take_back_with(
            locate().as_ref().map_err(Clone::clone),
            "t42",
            false,
            true,
            || {
                restarts += 1;
                set_tab_owner(&mut tabs, dir.path(), &id, None)
            },
        )
        .unwrap();
        assert_eq!(restarts, 1);
        let mut restored = TabStore::default();
        restored.ensure(dir.path()).unwrap();
        assert!(restored.tabs[0].herdr_terminal.is_none());
        assert!(restored.tabs[0].handed_off_at.is_none());
        // hc may have been reinstalled while the prompt was open. Missing-owner
        // confirmation never grants permission to kill a now-live terminal.
        let (_dir, hc) = fake("echo '{\"address\":\"t42\",\"state\":\"live\"}'");
        assert_eq!(
            take_back_with(Ok(&hc), "t42", false, true, || Ok(())).unwrap_err(),
            "RECALL_HERDR_END_CONFIRM"
        );
    }
    #[test]
    fn shell_tab_with_a_matching_transcript_moves_the_claude_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().join("tabs/tab-123");
        let transcript = dir.path().join("saved-123.jsonl");
        std::fs::write(&transcript, "synthetic nonempty transcript\n").unwrap();
        let mut tab = tab();
        tab.shell = true;
        tab.claude_active = true;
        tab.claude_session_id = Some("saved-123".into());
        tab.claude_transcript_path = Some(transcript.clone());
        let mut config = Config::default();
        config.assistant.agent = "zsh".into();
        config.assistant.agent_args = vec!["-l".into()];
        assert_eq!(
            launch_spec(&config, &tab, dir.path(), &workspace),
            ("claude".into(), vec!["--resume".into(), "saved-123".into()])
        );
        // An explicit Claude configuration keeps its executable and settings,
        // but replaces stale inherited session selection with this tab's ID.
        config.assistant.agent = "/custom/bin/claude".into();
        config.assistant.agent_args = vec![
            "--settings".into(),
            "custom.json".into(),
            "--continue".into(),
            "--resume".into(),
            "old-session".into(),
        ];
        assert_eq!(
            launch_spec(&config, &tab, dir.path(), &workspace),
            (
                "/custom/bin/claude".into(),
                vec![
                    "--settings".into(),
                    "custom.json".into(),
                    "--resume".into(),
                    "saved-123".into()
                ]
            )
        );
        config.assistant.agent = "zsh".into();
        config.assistant.agent_args = vec!["-l".into()];
        for invalid in [
            PathBuf::from("missing.jsonl"),
            dir.path().join("wrong-id.jsonl"),
        ] {
            std::fs::write(dir.path().join("wrong-id.jsonl"), "synthetic\n").unwrap();
            tab.claude_transcript_path = Some(invalid);
            assert_eq!(
                launch_spec(&config, &tab, dir.path(), &workspace),
                ("zsh".into(), vec!["-l".into()])
            );
        }
        tab.claude_transcript_path = Some(transcript.clone());
        std::fs::write(&transcript, "").unwrap();
        assert_eq!(
            launch_spec(&config, &tab, dir.path(), &workspace),
            ("zsh".into(), vec!["-l".into()])
        );
    }
    #[test]
    fn errors_use_bounded_stderr_and_expose_only_the_error_kind() {
        for redirect in [">&2", ""] {
            let (_dir, hc) = fake(&format!(
                r#"echo '{{"error":{{"kind":"not_found","message":"PRIVATE","hint":"PRIVATE"}},"address":"PRIVATE"}}' {redirect}; exit 3"#
            ));
            let (ok, json) = hc.call(&["get".into()]).unwrap();
            assert!(!ok);
            assert_eq!(json, serde_json::json!({"error":{"kind":"not_found"}}));
            assert!(matches!(hc.get("t42").unwrap(), Lookup::Missing));
            assert_eq!(
                hc.kill("t42").unwrap_err(),
                "Herdr could not end the terminal"
            );
        }
        for body in [
            "echo PRIVATE >&2; exit 1",
            "echo '{\"error\":{\"message\":\"PRIVATE\"}}' >&2; exit 1",
            "echo invalid-stdout; echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3",
            "echo '{\"address\":\"t42\",\"state\":\"live\"}' >&2",
        ] {
            let (_dir, hc) = fake(body);
            let error = hc.call(&[]).unwrap_err();
            assert!(!error.contains("PRIVATE"));
        }
        let (_dir, hc) = fake("head -c 65537 /dev/zero >&2; exit 1");
        assert_eq!(
            hc.call(&[]).unwrap_err(),
            "Herdr returned too much metadata"
        );
        let (_dir, hc) = fake("head -c 65537 /dev/zero; exit 1");
        assert_eq!(
            hc.call(&[]).unwrap_err(),
            "Herdr returned too much metadata"
        );
    }
    #[test]
    fn pre_run_failure_restores_local_ownership_and_restarts_once() {
        let dir = tempfile::tempdir().unwrap();
        let mut tabs = TabStore::default();
        let id = tabs.open(dir.path(), None, true).unwrap();
        set_tab_owner(&mut tabs, dir.path(), &id, Some("minutes-pending".into())).unwrap();
        let mut restarts = 0;
        let result: Result<(), String> =
            finish_preparation(Err("Transcript refresh failed".into()), || {
                set_tab_owner(&mut tabs, dir.path(), &id, None)?;
                restarts += 1;
                Ok(())
            });
        assert!(result.unwrap_err().contains("restarted in Minutes"));
        assert_eq!(restarts, 1);
        let mut restored = TabStore::default();
        restored.ensure(dir.path()).unwrap();
        assert!(restored.tabs[0].herdr_terminal.is_none());
        finish_preparation(Ok(()), || {
            restarts += 1;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            restarts, 1,
            "Successful preparation must not restart before hc run"
        );
    }
    #[test]
    fn a_working_tab_cannot_be_handed_off() {
        let mut tab = tab();
        tab.status = crate::recall_tabs::Status::Working;
        assert_eq!(
            ensure_idle(&tab).unwrap_err(),
            "Wait for this reply to finish, then move it to Herdr"
        );
        for status in [
            crate::recall_tabs::Status::Done,
            crate::recall_tabs::Status::Waiting,
            crate::recall_tabs::Status::Error,
        ] {
            tab.status = status;
            assert!(ensure_idle(&tab).is_ok());
        }
    }

    #[test]
    fn absent_down_invalid_slow_and_successful_server() {
        let empty = tempfile::tempdir().unwrap();
        assert!(which::which_in("hc", Some(empty.path()), empty.path()).is_err());
        for body in ["exit 1", "echo '{}'", "exec /bin/sleep 30"] {
            let (_dir, hc) = fake(body);
            let start = Instant::now();
            assert!(!hc.available());
            assert!(start.elapsed() < Duration::from_secs(2));
        }
        let (_dir, hc) = fake("echo '{\"core\":{\"id\":\"test\"},\"version\":\"0.1.21\"}'");
        assert!(hc.available());
    }
    #[test]
    fn config_defaults_auto_and_off_or_invalid_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        assert!(enabled_at(&path));
        for (raw, expected) in [
            ("", true),
            ("[herdr]\nenabled = 'auto'", true),
            ("[herdr]\nenabled = 'off'", false),
            ("[herdr]\nenabled = 'yes'", false),
            ("bad TOML", false),
        ] {
            std::fs::write(&path, raw).unwrap();
            assert_eq!(enabled_at(&path), expected);
        }
    }
    #[test]
    fn terminal_lifetime_uses_state_not_done_status_and_unknown_is_refused() {
        for (body, expected) in [
            (
                "echo '{\"address\":\"t42\",\"state\":\"live\",\"status\":{\"state\":\"done\"}}'",
                "live",
            ),
            (
                "echo '{\"address\":\"t42\",\"state\":\"finished\",\"exit_code\":0}'",
                "finished",
            ),
            (
                "echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3",
                "missing",
            ),
        ] {
            let (_dir, hc) = fake(body);
            let state = hc.get("t42").unwrap();
            assert_eq!(
                match state {
                    Lookup::Live(_) => "live",
                    Lookup::Finished => "finished",
                    Lookup::Missing => "missing",
                },
                expected
            );
            assert!(hc.get("remote/t42").is_err());
        }
        let (_dir, hc) = fake("echo '{\"address\":\"t42\",\"state\":\"unknown\"}'");
        assert!(hc.get("t42").is_err());
    }
    #[test]
    fn failed_run_is_distinct_from_an_uncertain_timeout() {
        for (body, uncertain) in [
            (
                "echo '{\"error\":{\"kind\":\"spawn_failed\"}}' >&2; exit 1",
                false,
            ),
            ("exec /bin/sleep 30", true),
            ("echo 'invalid-json'", true),
        ] {
            let (dir, hc) = fake(body);
            let error = hc
                .run(
                    "minutes-test",
                    &tab(),
                    dir.path(),
                    Path::new("/bin/true"),
                    vec![],
                )
                .unwrap_err();
            assert_eq!(error.uncertain, uncertain);
        }
    }
    #[test]
    fn launch_preserves_argv_and_unsets_nested_environment_inside_server() {
        let (dir, hc) = fake(
            r#"
if [ "$1" = run ]; then
  printf '%s\n' "$@" > "$0.args"
  while [ "$1" != -- ]; do shift; done
  shift
  export CLAUDECODE=synthetic CLAUDE_CODE_CHILD_SESSION=synthetic CLAUDE_CODE_SESSION_ID=synthetic CLAUDE_PID=synthetic
  "$@" || exit 1
  echo '{"address":"t42","state":"live"}'
fi
"#,
        );
        let observed = dir.path().join("observed");
        let agent = dir.path().join("agent");
        std::fs::write(
            &agent,
            format!(
                r#"#!/bin/sh
for variable in CLAUDECODE CLAUDE_CODE_CHILD_SESSION CLAUDE_CODE_SESSION_ID CLAUDE_PID; do
  eval 'present=${{'"$variable"'+set}}'
  [ -z "$present" ] || exit 1
done
printf '%s\n' "$@" > '{}'
"#,
                observed.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&agent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let args = vec![
            "--custom".into(),
            "a value with spaces; $(literal)".into(),
            "--resume".into(),
            "synthetic-id".into(),
        ];
        assert_eq!(
            hc.run("minutes-tab-123", &tab(), dir.path(), &agent, args.clone())
                .unwrap(),
            "t42"
        );
        assert_eq!(
            std::fs::read_to_string(observed)
                .unwrap()
                .lines()
                .collect::<Vec<_>>(),
            args
        );
        let argv = std::fs::read_to_string(dir.path().join("hc.args")).unwrap();
        for flag in [
            "--bg",
            "--no-parent",
            "--outlive-parent",
            "minutes.tab=tab-123",
            "minutes.meeting=board",
            "MINUTES_RECALL_TAB=tab-123",
            "env\n-u\nCLAUDECODE",
        ] {
            assert!(argv.contains(flag), "{flag}");
        }
        assert!(!argv.contains("--on"));
        assert!(terminal_name(&"a".repeat(80)).len() <= 32);
        assert_eq!(terminal_name("tab-123"), "minutes-tab-123");
    }
    #[test]
    fn claude_handoff_uses_shared_root_and_explicit_tab_identity() {
        let (dir, hc) = fake(
            r#"
if [ "$1" = run ]; then
  printf '%s\n' "$@" > "$0.args"
  echo '{"address":"t42","state":"live"}'
fi
"#,
        );
        let root = dir.path().join("assistant");
        let workspace = root.join("tabs/tab-123");
        let mut config = Config::default();
        config.assistant.agent = "claude".into();
        let (agent, args) = launch_spec(&config, &tab(), &root, &workspace);
        let cwd = crate::recall_tabs::launch_directory(&root, &workspace, &agent);
        assert_eq!(cwd, root);
        hc.run("minutes-tab-123", &tab(), &cwd, Path::new(&agent), args)
            .unwrap();
        let argv = std::fs::read_to_string(dir.path().join("hc.args")).unwrap();
        assert!(argv.contains(&format!("--cwd\n{}\n", root.display())));
        assert!(argv.contains("MINUTES_RECALL_TAB=tab-123\n"));
        assert!(!argv.contains("--dangerously-skip-permissions"));
        config.assistant.agent = "codex".into();
        let (agent, _) = launch_spec(&config, &tab(), &root, &workspace);
        assert_eq!(
            crate::recall_tabs::launch_directory(&root, &workspace, &agent),
            workspace
        );
    }

    #[test]
    fn recovery_restarts_only_a_rejected_launch_with_confirmed_no_owner() {
        let rejected = LaunchError {
            message: "Rejected".into(),
            uncertain: false,
        };
        let uncertain = LaunchError {
            message: "Timeout".into(),
            uncertain: true,
        };
        let (_dir, missing) = fake("echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3");
        assert_eq!(
            reconcile_launch(&missing, "minutes-test", &rejected),
            Recovery::Restart
        );
        assert_eq!(
            reconcile_launch(&missing, "minutes-test", &uncertain),
            Recovery::Uncertain
        );
        let (_dir, live) = fake("echo '{\"address\":\"t42\",\"state\":\"live\"}'");
        assert_eq!(
            reconcile_launch(&live, "minutes-test", &uncertain),
            Recovery::Owned("t42".into())
        );
        let (_dir, down) = fake("exit 1");
        assert_eq!(
            reconcile_launch(&down, "minutes-test", &rejected),
            Recovery::Uncertain
        );
    }
    #[test]
    fn take_back_requires_explicit_end_and_confirmed_exit() {
        let (dir, hc) = fake(
            r#"
case "$1" in
  get) if [ -f "$0.ended" ]; then echo '{"address":"t42","state":"finished"}'; else echo '{"address":"t42","state":"live","status":{"state":"done"}}'; fi ;;
  kill) touch "$0.ended"; echo '{"ok":true}' ;;
esac
"#,
        );
        assert_eq!(
            release_terminal(&hc, "t42", false, false).unwrap_err(),
            "RECALL_HERDR_END_CONFIRM"
        );
        assert!(!dir.path().join("hc.ended").exists());
        release_terminal(&hc, "t42", true, false).unwrap();
        assert!(dir.path().join("hc.ended").exists());
        release_terminal(&hc, "t42", false, false).unwrap();
        let (_dir, hc) = fake("echo '{\"error\":{\"kind\":\"not_found\"}}' >&2; exit 3");
        assert!(release_terminal(&hc, "t42", true, false).is_err());
        let (_dir, hc) = fake("case \"$1\" in get) echo '{\"address\":\"t42\",\"state\":\"live\"}';; kill) echo '{}';; esac");
        assert!(
            release_terminal(&hc, "t42", true, false).is_err(),
            "kill acknowledgement alone cannot release ownership"
        );
    }

    #[test]
    fn child_is_reaped_before_handoff_can_launch() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        let dir = tempfile::tempdir().unwrap();
        let pty = native_pty_system().openpty(PtySize::default()).unwrap();
        let mut command = CommandBuilder::new("/bin/sleep");
        command.arg("30");
        let child = pty.slave.spawn_command(command).unwrap();
        let pid = child.process_id().unwrap();
        drop(pty.slave);
        let session = crate::pty::PtySession {
            writer: pty.master.take_writer().unwrap(),
            master: pty.master,
            reader_handle: None,
            child,
            context_dir: dir.path().into(),
            title: "Synthetic".into(),
            command: "/bin/sleep".into(),
            theme_dark: None,
            focus_hook: false,
        };
        assert!(stop_and_reap(session).is_ok());
        // waitpid sees no child after the production handoff stop returned.
        assert_eq!(
            unsafe { libc::waitpid(pid as i32, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
