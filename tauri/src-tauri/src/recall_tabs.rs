//! Recall tab metadata and Claude hook protocol. Optional consumers never block capture.
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use tauri::{Emitter, Manager};

pub const LIMIT: usize = 4;
const MANIFEST: &str = "recall-tabs.json";
const STATE: &str = ".recall-state.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Working,
    Waiting,
    #[default]
    Done,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tab {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub phase: Option<String>,
    pub meeting_path: Option<String>,
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub shell: bool,
    #[serde(default)]
    pub claude_active: bool,
    #[serde(default)]
    pub capture_session_id: Option<String>,
    #[serde(default)]
    pub capture_job_id: Option<String>,
    #[serde(default)]
    pub capture_complete: bool,
    #[serde(default)]
    pub status: Status,
    #[serde(default)]
    pub live: bool,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TabStore {
    pub tabs: Vec<Tab>,
    pub active: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_live_id: Option<String>,
    #[serde(skip)]
    loaded: bool,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 80 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

pub fn workspace_at(root: &Path, id: &str) -> Result<PathBuf, String> {
    if !valid_id(id) {
        return Err("Invalid Recall tab ID".into());
    }
    Ok(root.join("tabs").join(id))
}

pub fn workspace(id: &str) -> Result<PathBuf, String> {
    workspace_at(&crate::context::workspace_dir(), id)
}

pub(crate) fn private_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing parent")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    // Each writer owns its own file; hook process IDs keep staging names disjoint.
    let temp = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    let mut file = options.open(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(temp, path).map_err(|e| e.to_string())
}

/// Bound optional consumer state reads even if a managed file was overwritten.
fn read_small(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(65_537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65_536 {
        return Err("Recall state file is too large".into());
    }
    Ok(bytes)
}

/// Locate legacy conversation metadata by filename, without reading transcripts.
fn legacy_session(root: &Path, home: &Path) -> Option<String> {
    let key: String = root
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    fs::read_dir(home.join(".claude/projects").join(key))
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let id = path.file_stem()?.to_str()?.to_string();
            if path.extension()?.to_str()? != "jsonl" || !valid_id(&id) {
                return None;
            }
            Some((entry.metadata().ok()?.modified().ok()?, id))
        })
        .max_by_key(|(time, _)| *time)
        .map(|(_, id)| id)
}

impl TabStore {
    pub fn ensure(&mut self, root: &Path) -> Result<(), String> {
        if self.loaded {
            return Ok(());
        }
        if root.join(MANIFEST).exists() {
            *self = serde_json::from_slice(&read_small(&root.join(MANIFEST))?)
                .map_err(|e| format!("Recall tabs could not be restored: {e}"))?;
            let unique: std::collections::HashSet<_> = self.tabs.iter().map(|t| &t.id).collect();
            if self.tabs.len() > LIMIT
                || unique.len() != self.tabs.len()
                || self.tabs.iter().any(|t| !valid_id(&t.id))
            {
                return Err("Invalid saved Recall tabs".into());
            }
        } else {
            let legacy = dirs::home_dir().and_then(|home| legacy_session(root, &home));
            if root.join("CLAUDE.md").exists() || legacy.is_some() {
                let dir = workspace_at(root, "assistant")?;
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                for name in [
                    "CURRENT_MEETING.md",
                    "CURRENT_ARTIFACT.md",
                    "ASSISTANT_CONTEXT.md",
                    "CLAUDE.md",
                    "AGENTS.md",
                ] {
                    if root.join(name).is_file() && !dir.join(name).exists() {
                        fs::copy(root.join(name), dir.join(name)).map_err(|e| e.to_string())?;
                    }
                }
                self.tabs.push(Tab {
                    id: "assistant".into(),
                    title: "Recall".into(),
                    phase: None,
                    meeting_path: fs::read(
                        root.parent().unwrap_or(root).join("recall-workspace.json"),
                    )
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                    .and_then(|value| {
                        value["currentMeetingPath"]
                            .as_str()
                            .or_else(|| value["current_meeting_path"].as_str())
                            .map(str::to_string)
                    }),
                    claude_session_id: legacy,
                    shell: false,
                    claude_active: false,
                    capture_session_id: None,
                    capture_job_id: None,
                    capture_complete: false,
                    status: Status::Done,
                    live: false,
                    note: None,
                });
                if let Some(tab) = self.tabs.last_mut() {
                    if let Some(path) = &tab.meeting_path {
                        tab.title = Path::new(path)
                            .file_stem()
                            .and_then(|p| p.to_str())
                            .unwrap_or("Recall")
                            .replace('-', " ");
                    }
                }
                self.active = Some("assistant".into());
            }
            // Parent instructions must be neutral: nested tabs must not inherit a live or meeting focus.
            for name in ["CLAUDE.md", "AGENTS.md"] {
                private_write(&root.join(name), b"# Minutes Recall\n\nEach tab has its own instructions and context in its working directory. Only use that tab's CURRENT_MEETING.md, CURRENT_ARTIFACT.md and live guidance.\n")?;
            }
            for name in [
                "CURRENT_MEETING.md",
                "CURRENT_ARTIFACT.md",
                "ASSISTANT_CONTEXT.md",
            ] {
                let _ = fs::remove_file(root.join(name));
            }
            self.save(root)?;
        }
        crate::context::ensure_workspace_repository(root)?;
        self.loaded = true;
        self.refresh(root)?;
        for tab in &mut self.tabs {
            tab.live = false;
            tab.status = Status::Done;
            tab.claude_active = false;
            let dir = workspace_at(root, &tab.id)?;
            let _ = fs::remove_file(dir.join(STATE));
            let _ = fs::remove_file(dir.join(".live-tab"));
            for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
                if dir.join(name).exists() {
                    crate::commands::update_assistant_live_context_file(&dir.join(name), false);
                }
            }
            if tab
                .meeting_path
                .as_ref()
                .is_some_and(|p| !Path::new(p).is_file())
            {
                tab.meeting_path = None;
                tab.capture_session_id = None;
                tab.capture_job_id = None;
                tab.title = "Chat".into();
                tab.phase = None;
                tab.note = Some(
                    "The meeting for this tab was deleted. This conversation is now a plain chat."
                        .into(),
                );
                let dir = workspace_at(root, &tab.id)?;
                set_focus_target(&dir, None, false)?;
                crate::context::clear_active_meeting_context(&dir)?;
                crate::context::clear_general_assistant_context(&dir)?;
            }
        }
        if !self
            .tabs
            .iter()
            .any(|t| Some(&t.id) == self.active.as_ref())
        {
            self.active = self.tabs.first().map(|t| t.id.clone());
        }
        // Upgrade generic free-chat names once, retaining their identity on
        // subsequent restores and after other tabs are closed.
        for index in 0..self.tabs.len() {
            if self.tabs[index].meeting_path.is_none() && self.tabs[index].title == "Chat" {
                self.tabs[index].title = self.next_chat_title();
            }
        }
        self.save(root)
    }

    fn next_chat_title(&self) -> String {
        (1..=self.tabs.len() + 1)
            .map(|number| format!("Chat {number}"))
            .find(|title| self.tabs.iter().all(|tab| &tab.title != title))
            .expect("one more candidate than existing tabs")
    }

    fn set_presentation(&mut self, id: &str, title: Option<&str>, phase: Option<&str>) {
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) {
            if tab.meeting_path.is_some() {
                if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
                    tab.title = title.chars().take(256).collect();
                }
                if let Some(phase) = phase.filter(|phase| matches!(*phase, "prep" | "debrief")) {
                    tab.phase = Some(phase.into());
                }
            }
        }
    }

    fn remove(&mut self, id: &str) {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.tabs.remove(index);
        if self.active.as_deref() == Some(id) {
            self.active = self
                .tabs
                .get(index.saturating_sub(1))
                .map(|tab| tab.id.clone());
        }
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        private_write(
            &root.join(MANIFEST),
            &serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?,
        )
    }

    pub fn open(
        &mut self,
        root: &Path,
        meeting: Option<&str>,
        fresh: bool,
    ) -> Result<String, String> {
        self.ensure(root)?;
        if !fresh {
            let existing = meeting
                .and_then(|p| {
                    self.tabs
                        .iter()
                        .find(|t| t.meeting_path.as_deref() == Some(p))
                })
                .or_else(|| {
                    if meeting.is_none() {
                        self.active
                            .as_ref()
                            .and_then(|id| self.tabs.iter().find(|t| &t.id == id))
                    } else {
                        None
                    }
                });
            if let Some(tab) = existing {
                let id = tab.id.clone();
                self.active = Some(id.clone());
                self.save(root)?;
                return Ok(id);
            }
        }
        if self.tabs.len() >= LIMIT {
            return Err("RECALL_TAB_LIMIT".into());
        }
        let id = format!(
            "tab-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        );
        let title = meeting
            .and_then(|p| Path::new(p).file_stem())
            .and_then(|p| p.to_str())
            .map(|s| s.replace('-', " "))
            .unwrap_or_else(|| self.next_chat_title());
        self.tabs.push(Tab {
            id: id.clone(),
            title,
            phase: None,
            meeting_path: meeting.map(str::to_string),
            claude_session_id: None,
            shell: false,
            claude_active: false,
            capture_session_id: None,
            capture_job_id: None,
            capture_complete: false,
            status: Status::Done,
            live: false,
            note: None,
        });
        self.active = Some(id.clone());
        self.save(root)?;
        Ok(id)
    }

    pub fn refresh(&mut self, root: &Path) -> Result<(), String> {
        for tab in &mut self.tabs {
            if let Ok(raw) = read_small(&workspace_at(root, &tab.id)?.join(STATE)) {
                if let Ok(state) = serde_json::from_slice::<HookState>(&raw) {
                    tab.status = state.status;
                    tab.claude_active = state.claude_active;
                    if state.session_id.as_deref().is_some_and(valid_id) {
                        tab.claude_session_id = state.session_id;
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct HookState {
    #[serde(default)]
    claude_active: bool,
    status: Status,
    session_id: Option<String>,
}

fn status_for_event(event: &str, input: &serde_json::Value) -> Option<Status> {
    match event {
        "PreToolUse" if input["tool_name"].as_str() == Some("AskUserQuestion") => {
            Some(Status::Waiting)
        }
        "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => Some(Status::Working),
        "Notification"
            if matches!(
                input["notification_type"].as_str(),
                Some("permission_prompt" | "idle_prompt" | "elicitation_dialog")
            ) =>
        {
            Some(Status::Waiting)
        }
        "PermissionRequest" => Some(Status::Waiting),
        "Stop" | "SessionStart" | "SessionEnd" => Some(Status::Done),
        "StopFailure" => Some(Status::Error),
        _ => None,
    }
}

/// Run before GUI initialization, using the app's own executable on all platforms.
/// Claude's exec-form hook passes arguments directly, with no shell or Node dependency.
pub fn maybe_run_hook() -> Option<i32> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).map(String::as_str) != Some("--recall-hook") {
        return None;
    }
    let result = (|| -> Result<(), String> {
        let root = args.get(2).ok_or("Missing workspace")?;
        let event = args.get(3).ok_or("Missing event")?;
        let dir = Path::new(root);
        if !dir.join(".recall-tab").is_file() {
            return Err("Not a Recall workspace".into());
        }
        let mut input = String::new();
        std::io::stdin()
            .take(1_048_577)
            .read_to_string(&mut input)
            .map_err(|e| e.to_string())?;
        if input.len() > 1_048_576 {
            return Err("Hook input too large".into());
        }
        let input: serde_json::Value = serde_json::from_str(&input).map_err(|e| e.to_string())?;
        if let Some(status) = status_for_event(event, &input) {
            let state = HookState {
                claude_active: event != "SessionEnd",
                status,
                session_id: input["session_id"]
                    .as_str()
                    .filter(|id| valid_id(id))
                    .map(str::to_string),
            };
            private_write(
                &dir.join(STATE),
                &serde_json::to_vec(&state).map_err(|e| e.to_string())?,
            )?;
        }
        if event == "UserPromptSubmit" {
            prepare_prompt_context(dir, &input)?;
            // Preserve #1308: notices are context for the next prompt, never typed turns.
            print!(
                "{}",
                crate::context::assistant_skills::take_focus_notices(dir)?
            );
        }
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Recall hook: {error}");
    }
    // Optional status/focus hooks must never block the user's agent.
    Some(0)
}

pub fn settings(workspace: &Path) -> Result<PathBuf, String> {
    private_write(&workspace.join(".recall-tab"), b"Minutes Recall tab v1\n")?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut hooks = serde_json::Map::new();
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PostToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ] {
        hooks.insert(event.into(), serde_json::json!([{ "hooks": [{ "type": "command", "command": executable, "args": ["--recall-hook", workspace, event], "timeout": if event == "UserPromptSubmit" { 15 } else { 5 } }] }]));
    }
    let directory = workspace.join(".claude");
    // Shared project settings are discovered from Claude's starting directory.
    // settings.local.json now resolves at the repository root on modern Claude,
    // so using it here would collapse tab hooks into the shared parent.
    let path = directory.join("settings.json");
    for target in [&directory, &path] {
        if fs::symlink_metadata(target).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Recall will not write through a .claude settings symlink".into());
        }
    }
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let mut settings: serde_json::Value = if path.exists() {
        serde_json::from_slice(&read_small(&path)?).map_err(|e| e.to_string())?
    } else {
        serde_json::json!({})
    };
    if !settings.is_object() {
        return Err("Recall local settings must be an object".into());
    }
    if settings
        .get("hooks")
        .is_some_and(|value| !value.is_object())
    {
        return Err("Recall local hooks must be an object".into());
    }
    if settings.get("hooks").is_none() {
        settings["hooks"] = serde_json::json!({});
    }
    for (event, managed) in hooks {
        let entries = settings["hooks"]
            .as_object_mut()
            .unwrap()
            .entry(event)
            .or_insert_with(|| serde_json::json!([]));
        let entries = entries
            .as_array_mut()
            .ok_or("Recall hook entries must be an array")?;
        // Replace only our own commands, retaining custom hooks, matchers and settings.
        for entry in entries.iter_mut() {
            if let Some(commands) = entry["hooks"].as_array_mut() {
                commands.retain(|command| {
                    !command["args"].as_array().is_some_and(|args| {
                        args.first().and_then(|v| v.as_str()) == Some("--recall-hook")
                    })
                });
            }
        }
        entries.retain(|entry| !entry["hooks"].as_array().is_some_and(Vec::is_empty));
        entries.extend(managed.as_array().unwrap().iter().cloned());
    }
    private_write(
        &path,
        &serde_json::to_vec_pretty(&settings).map_err(|e| e.to_string())?,
    )?;
    Ok(path)
}

#[derive(Serialize, Deserialize)]
struct FocusTarget {
    meeting_path: Option<String>,
    context_ready: bool,
}

pub(crate) fn set_focus_target(
    workspace: &Path,
    meeting: Option<&str>,
    ready: bool,
) -> Result<(), String> {
    private_write(
        &workspace.join(".recall-focus.json"),
        &serde_json::to_vec(&FocusTarget {
            meeting_path: meeting.map(str::to_string),
            context_ready: ready,
        })
        .map_err(|e| e.to_string())?,
    )
}

pub(crate) fn mark_context_ready(workspace: &Path) -> Result<(), String> {
    let path = workspace.join(".recall-focus.json");
    if !path.exists() {
        return Ok(());
    }
    let mut target: FocusTarget =
        serde_json::from_slice(&read_small(&path)?).map_err(|e| e.to_string())?;
    target.context_ready = true;
    private_write(
        &path,
        &serde_json::to_vec(&target).map_err(|e| e.to_string())?,
    )
}

fn prepare_prompt_context(workspace: &Path, input: &serde_json::Value) -> Result<(), String> {
    prepare_prompt_context_with(workspace, input, |meeting| {
        // Running from Claude's prompt hook proves the shell launched Claude.
        // Verify subscription auth before reading stored meeting contents.
        let binary =
            crate::commands::resolve_agent_binary("claude").map_err(|e| e.user_message())?;
        if !crate::commands::claude_terminal_is_authenticated(
            &binary.to_string_lossy(),
            Some(workspace),
        ) {
            return Err("Sign in to Claude before sharing meeting context".into());
        }
        let config = minutes_core::Config::load_strict()?;
        crate::commands::materialize_recall_terminal_context(workspace, &config, meeting)
    })
}

fn prepare_prompt_context_with(
    workspace: &Path,
    input: &serde_json::Value,
    prepare: impl FnOnce(Option<&str>) -> Result<(), String>,
) -> Result<(), String> {
    let prompt = input["prompt"].as_str().unwrap_or("").trim();
    if prompt.is_empty() || prompt.starts_with('/') {
        return Ok(());
    }
    let path = workspace.join(".recall-focus.json");
    if !path.exists() {
        return Ok(());
    }
    let target: FocusTarget =
        serde_json::from_slice(&read_small(&path)?).map_err(|e| e.to_string())?;
    if target.context_ready {
        return Ok(());
    }
    prepare(target.meeting_path.as_deref())?;
    mark_context_ready(workspace)
}

#[derive(Clone)]
struct ActiveCapture {
    id: String,
    title: String,
}

// The recording PID file is written once per capture and removed on stop.
// Its owner/version stays stable even after the optional transcript relay
// shuts down. This also supports microphone capture with context tracking off.
pub(crate) fn recording_capture_id() -> Option<String> {
    recording_capture_id_at(&minutes_core::pid::pid_path())
}

fn recording_capture_id_at(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let modified = file
        .metadata()
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let mut owner = String::new();
    file.take(128).read_to_string(&mut owner).ok()?;
    let pid = owner.trim().parse::<u32>().ok()?;
    Some(format!("recording:{pid}:{modified}"))
}

// The relay ID is a producer identity, not a guessed future Markdown path.
// Deserialize only identity fields; never copy the relay capability token.
fn relay_capture_id() -> Option<String> {
    #[derive(Deserialize)]
    struct Identity {
        session_id: String,
        owner_pid: u32,
        heartbeat_at: chrono::DateTime<chrono::Utc>,
    }
    let raw = read_small(&minutes_core::copilot::capture_relay_discovery_path()).ok()?;
    let identity: Identity = serde_json::from_slice(&raw).ok()?;
    (identity.owner_pid == std::process::id()
        && chrono::Utc::now()
            .signed_duration_since(identity.heartbeat_at)
            .num_seconds()
            <= 10)
        .then(|| format!("relay:{}", identity.session_id))
}

fn active_capture(
    live: &minutes_core::live_transcript::SessionStatus,
    recording: &minutes_core::pid::RecordingStatus,
) -> Option<ActiveCapture> {
    if !recording.recording
        && (!live.active
            || live.source
                == Some(minutes_core::live_transcript::TranscriptSource::RecordingSidecar))
    {
        return None;
    }
    let id = if recording.recording {
        minutes_core::pid::read_recording_metadata()
            .and_then(|meta| meta.context_session_id)
            .or_else(recording_capture_id)
    } else {
        live.session_id.clone()
    }
    .or_else(relay_capture_id)?;
    let title = id
        .starts_with("ctx-")
        .then(|| minutes_core::context_store::get_session(&id).ok().flatten())
        .flatten()
        .and_then(|session| session.title)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or_else(|| "Live call".into());
    Some(ActiveCapture { id, title })
}

impl TabStore {
    fn record_capture_completion(
        &mut self,
        root: &Path,
        capture_id: &str,
        job_id: Option<&str>,
        meeting: Option<&str>,
    ) -> Result<(), String> {
        for tab in &mut self.tabs {
            if tab.capture_session_id.as_deref() != Some(capture_id) {
                continue;
            }
            tab.capture_job_id = job_id.map(str::to_string);
            if let Some(path) = meeting {
                finish_tab_capture(root, tab, path, None)?;
            }
        }
        Ok(())
    }

    fn admit_capture(&mut self, root: &Path, capture: &ActiveCapture) -> Result<String, String> {
        if let Some(tab) = self
            .tabs
            .iter()
            .find(|tab| tab.capture_session_id.as_deref() == Some(&capture.id))
        {
            return Ok(tab.id.clone());
        }
        let previous = self.active.clone();
        let id = self.open(root, None, true)?;
        self.active = previous;
        let tab = self
            .tabs
            .iter_mut()
            .find(|tab| tab.id == id)
            .ok_or("Recall tab not found")?;
        tab.capture_session_id = Some(capture.id.clone());
        tab.title = capture.title.chars().take(256).collect();
        self.pending_live_id = None;
        Ok(id)
    }

    fn sync_live_capture(
        &mut self,
        root: &Path,
        capture: Option<&ActiveCapture>,
    ) -> Result<(), String> {
        self.sync_live_capture_with(root, capture, completed_capture)
    }

    fn sync_live_capture_with(
        &mut self,
        root: &Path,
        capture: Option<&ActiveCapture>,
        completed: impl Fn(&Tab) -> Option<(String, Option<String>)>,
    ) -> Result<(), String> {
        self.pending_live_id = None;
        if let Some(capture) = capture {
            if let Err(error) = self.admit_capture(root, capture) {
                if error != "RECALL_TAB_LIMIT" {
                    return Err(error);
                }
                self.pending_live_id = Some(capture.id.clone());
            }
        }
        for tab in &mut self.tabs {
            let is_live = capture
                .is_some_and(|capture| tab.capture_session_id.as_deref() == Some(&capture.id));
            if is_live {
                tab.capture_complete = false;
            }
            if !is_live && !tab.capture_complete {
                if let Some((path, title)) = completed(tab) {
                    finish_tab_capture(root, tab, &path, title.as_deref())?;
                }
            }
            if tab.live != is_live {
                tab.live = is_live;
                let dir = workspace_at(root, &tab.id)?;
                if is_live {
                    private_write(&dir.join(".live-tab"), b"live\n")?;
                } else {
                    let _ = fs::remove_file(dir.join(".live-tab"));
                }
                for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
                    crate::commands::update_assistant_live_context_file(&dir.join(name), is_live);
                }
            }
        }
        self.tabs.sort_by_key(|tab| !tab.live);
        Ok(())
    }
}

fn completed_capture(tab: &Tab) -> Option<(String, Option<String>)> {
    if let Some(job) = tab
        .capture_job_id
        .as_deref()
        .filter(|id| valid_id(id))
        .and_then(minutes_core::jobs::load_job)
    {
        if let Some(completed) = completed_job(&job) {
            return Some(completed);
        }
    }
    let id = tab
        .capture_session_id
        .as_deref()
        .filter(|id| id.starts_with("ctx-"))?;
    let links = minutes_core::context_store::list_links_for_session(id).ok()?;
    links
        .into_iter()
        .rev()
        .find(|link| {
            link.kind == minutes_core::context_store::ContextLinkKind::MarkdownArtifact
                && Path::new(&link.target).is_file()
        })
        .map(|link| (link.target, None))
}

fn completed_job(job: &minutes_core::jobs::ProcessingJob) -> Option<(String, Option<String>)> {
    if matches!(
        job.state,
        minutes_core::jobs::JobState::Complete | minutes_core::jobs::JobState::NeedsReview
    ) {
        if let Some(path) = job
            .output_path
            .as_ref()
            .filter(|path| Path::new(path).is_file())
        {
            return Some((path.clone(), job.title.clone()));
        }
    }
    None
}

fn finish_tab_capture(
    root: &Path,
    tab: &mut Tab,
    path: &str,
    title: Option<&str>,
) -> Result<(), String> {
    // Only a real completion receipt may turn live focus into a saved meeting.
    if !Path::new(path).is_file() {
        return Ok(());
    }
    let dir = workspace_at(root, &tab.id)?;
    set_focus_target(&dir, Some(path), false)?;
    crate::context::clear_active_meeting_context(&dir)?;
    crate::context::assistant_skills::queue_focus_notice(&dir, crate::context::assistant_skills::FocusNotice::Meeting,
        "Recording ended. This conversation is now the meeting debrief. Read the tab's CURRENT_MEETING.md if Minutes has shared it for this question.")?;
    tab.capture_complete = true;
    tab.phase = Some("debrief".into());
    tab.meeting_path = Some(path.into());
    tab.title = title.map(str::to_string).unwrap_or_else(|| {
        Path::new(path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("Meeting")
            .replace('-', " ")
    });
    Ok(())
}

/// Capture has already been preserved/queued. Never wait for an optional Recall
/// lock or filesystem write on capture/Stop; this consumer owns its own worker.
pub(crate) fn capture_finished(
    app: &tauri::AppHandle,
    capture_id: Option<String>,
    job_id: Option<String>,
    meeting: Option<String>,
) {
    let Some(capture_id) = capture_id else {
        return;
    };
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("recall-completion".into())
        .spawn(move || {
            let state = app.state::<crate::commands::AppState>();
            let Ok(mut manager) = state.pty_manager.lock() else {
                return;
            };
            if !manager.tabs.loaded {
                return;
            }
            let root = crate::context::workspace_dir();
            let _ = manager.tabs.record_capture_completion(
                &root,
                &capture_id,
                job_id.as_deref(),
                meeting.as_deref(),
            );
            let _ = manager.tabs.save(&root);
            let _ = app.emit_to("main", "recall:tabs-changed", &manager.tabs);
        });
}

/// Poll only tiny tab state files, on a failure-isolated optional worker. No
/// capture locks, channels, or Stop dependencies. UI can be closed while it runs.
pub fn start_observer(app: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_millis(750));
        let state = app.state::<crate::commands::AppState>();
        let Ok(mut manager) = state.pty_manager.lock() else {
            continue;
        };
        if !manager.tabs.loaded {
            continue;
        }
        let root = crate::context::workspace_dir();
        let before = serde_json::to_vec(&manager.tabs).ok();
        let _ = manager.tabs.refresh(&root);
        // Read live state on this worker, never from capture or Stop.
        let live = minutes_core::live_transcript::session_status();
        let recording = minutes_core::pid::status();
        let capture = active_capture(&live, &recording);
        let _ = manager.tabs.sync_live_capture(&root, capture.as_ref());
        if before != serde_json::to_vec(&manager.tabs).ok() {
            let _ = manager.tabs.save(&root);
            let _ = app.emit_to("main", "recall:tabs-changed", &manager.tabs);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolated_workspaces_cap_and_resume_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut store = TabStore::default();
        let a = store.open(root, Some("/a.md"), false).unwrap();
        let b = store.open(root, Some("/b.md"), false).unwrap();
        assert_eq!(store.open(root, Some("/a.md"), false).unwrap(), a);
        let wa = workspace_at(root, &a).unwrap();
        let wb = workspace_at(root, &b).unwrap();
        fs::create_dir_all(&wa).unwrap();
        fs::create_dir_all(&wb).unwrap();
        fs::write(wa.join("CURRENT_MEETING.md"), "a").unwrap();
        fs::write(wb.join("CURRENT_MEETING.md"), "b").unwrap();
        crate::context::assistant_skills::queue_focus_notice(
            &wa,
            crate::context::assistant_skills::FocusNotice::Meeting,
            "a focus",
        )
        .unwrap();
        assert_eq!(
            crate::context::assistant_skills::take_focus_notices(&wb).unwrap(),
            ""
        );
        assert!(crate::context::assistant_skills::take_focus_notices(&wa)
            .unwrap()
            .contains("a focus"));
        assert_eq!(
            fs::read_to_string(wb.join("CURRENT_MEETING.md")).unwrap(),
            "b"
        );
        store.open(root, None, true).unwrap();
        store.open(root, None, true).unwrap();
        assert_eq!(
            store.open(root, None, true).unwrap_err(),
            "RECALL_TAB_LIMIT"
        );
        let hook = HookState {
            claude_active: true,
            status: Status::Waiting,
            session_id: Some("session-a".into()),
        };
        private_write(&wa.join(STATE), &serde_json::to_vec(&hook).unwrap()).unwrap();
        store.refresh(root).unwrap();
        store.save(root).unwrap();
        let mut restored = TabStore::default();
        restored.ensure(root).unwrap();
        assert_eq!(
            restored.tabs[0].claude_session_id.as_deref(),
            Some("session-a")
        );
        assert!(restored.tabs[0].meeting_path.is_none());
        assert!(restored.tabs[0].note.as_ref().unwrap().contains("deleted"));
        assert!(workspace_at(root, "../other").is_err());
    }
    #[test]
    fn status_comes_only_from_hook_events() {
        assert_eq!(
            status_for_event("UserPromptSubmit", &serde_json::json!({})),
            Some(Status::Working)
        );
        assert_eq!(
            status_for_event(
                "Notification",
                &serde_json::json!({"notification_type":"permission_prompt"})
            ),
            Some(Status::Waiting)
        );
        assert_eq!(
            status_for_event(
                "Notification",
                &serde_json::json!({"notification_type":"auth_success"})
            ),
            None
        );
        assert_eq!(
            status_for_event("Stop", &serde_json::json!({})),
            Some(Status::Done)
        );
        assert_eq!(
            status_for_event("PreToolUse", &serde_json::json!({})),
            Some(Status::Working)
        );
    }
}

#[tauri::command]
pub fn cmd_recall_tabs(state: tauri::State<crate::commands::AppState>) -> Result<TabStore, String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    let root = crate::context::workspace_dir();
    manager.tabs.ensure(&root)?;
    manager.tabs.refresh(&root)?;
    manager.tabs.save(&root)?;
    Ok(manager.tabs.clone())
}

#[tauri::command]
pub fn cmd_recall_tab_select(
    state: tauri::State<crate::commands::AppState>,
    tab_id: String,
    meeting_title: Option<String>,
    phase: Option<String>,
) -> Result<(), String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    if !manager.tabs.tabs.iter().any(|t| t.id == tab_id) {
        return Err("Recall tab not found".into());
    }
    manager
        .tabs
        .set_presentation(&tab_id, meeting_title.as_deref(), phase.as_deref());
    manager.tabs.active = Some(tab_id);
    manager.tabs.save(&crate::context::workspace_dir())
}

/// Called only by the explicit in-panel action, never restoration or focus.
#[tauri::command]
pub fn cmd_recall_tab_resume(
    state: tauri::State<crate::commands::AppState>,
    tab_id: String,
) -> Result<(), String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    manager.tabs.refresh(&crate::context::workspace_dir())?;
    let tab = manager
        .tabs
        .tabs
        .iter()
        .find(|t| t.id == tab_id)
        .ok_or("Recall tab not found")?;
    let resume = shell_resume_command(tab)?;
    if !manager
        .session_command(&tab_id)
        .is_some_and(|command| crate::commands::is_shell_command(&command))
        || !manager.shell_is_foreground(&tab_id)
    {
        return Err(
            "Return to an idle shell before resuming. You can also run claude --resume yourself."
                .into(),
        );
    }
    manager.write_input(&tab_id, resume.as_bytes())
}

fn shell_resume_command(tab: &Tab) -> Result<String, String> {
    if !tab.shell || tab.claude_active {
        return Err("Claude is already running in this tab".into());
    }
    let id = tab
        .claude_session_id
        .as_deref()
        .filter(|id| valid_id(id))
        .ok_or("No saved Claude conversation for this tab")?;
    Ok(format!("claude --resume {id}\r"))
}

#[tauri::command]
pub fn cmd_recall_tab_close(
    app: tauri::AppHandle,
    state: tauri::State<crate::commands::AppState>,
    tab_id: String,
    confirmed: Option<bool>,
) -> Result<TabStore, String> {
    let (session, tabs) = {
        let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
        let root = crate::context::workspace_dir();
        manager.tabs.refresh(&root)?;
        let tab = manager
            .tabs
            .tabs
            .iter()
            .find(|t| t.id == tab_id)
            .ok_or("Recall tab not found")?;
        if requires_close_confirmation(tab) && !confirmed.unwrap_or(false) {
            return Err("RECALL_CLOSE_CONFIRM".into());
        }
        manager.tabs.remove(&tab_id);
        manager.tabs.save(&root)?;
        (manager.take_session(&tab_id), manager.tabs.clone())
    };
    if let Some(session) = session {
        crate::pty::kill_session(session);
    }
    let _ = app.emit_to("main", "recall:tabs-changed", &tabs);
    Ok(tabs)
}

#[tauri::command]
pub fn cmd_recall_tab_open(
    state: tauri::State<crate::commands::AppState>,
    meeting_path: Option<String>,
    new_tab: Option<bool>,
    meeting_title: Option<String>,
    phase: Option<String>,
    capture_id: Option<String>,
) -> Result<TabStore, String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    let root = crate::context::workspace_dir();
    manager.tabs.ensure(&root)?;
    let id = if let Some(capture_id) = capture_id {
        if let Some(tab) = manager
            .tabs
            .tabs
            .iter()
            .find(|tab| tab.capture_session_id.as_deref() == Some(&capture_id))
        {
            tab.id.clone()
        } else {
            let capture = active_capture(
                &minutes_core::live_transcript::session_status(),
                &minutes_core::pid::status(),
            )
            .filter(|capture| capture.id == capture_id)
            .ok_or("The recording has ended; no live tab was opened.")?;
            let id = manager.tabs.admit_capture(&root, &capture)?;
            manager.tabs.sync_live_capture(&root, Some(&capture))?;
            id
        }
    } else {
        manager
            .tabs
            .open(&root, meeting_path.as_deref(), new_tab.unwrap_or(false))?
    };
    manager.tabs.active = Some(id.clone());
    manager
        .tabs
        .set_presentation(&id, meeting_title.as_deref(), phase.as_deref());
    manager.tabs.save(&crate::context::workspace_dir())?;
    Ok(manager.tabs.clone())
}

fn requires_close_confirmation(tab: &Tab) -> bool {
    tab.status == Status::Working
        || tab.live
        || tab.claude_active
        || tab.claude_session_id.is_some()
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn legacy_migration_preserves_context_and_neutralizes_parent_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("assistant");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("CLAUDE.md"), "old instructions\n<!-- LIVE_TRANSCRIPT_START -->\nLive content\n<!-- LIVE_TRANSCRIPT_END -->").unwrap();
        fs::write(root.join("CURRENT_MEETING.md"), "legacy meeting").unwrap();
        let meeting = temp.path().join("board-prep.md");
        fs::write(&meeting, "authorized legacy meeting").unwrap();
        fs::write(
            temp.path().join("recall-workspace.json"),
            serde_json::to_vec(&serde_json::json!({"currentMeetingPath": meeting})).unwrap(),
        )
        .unwrap();
        let mut store = TabStore::default();
        store.ensure(&root).unwrap();
        assert_eq!(store.tabs.len(), 1);
        assert_eq!(store.tabs[0].id, "assistant");
        assert_eq!(store.tabs[0].meeting_path.as_deref(), meeting.to_str());
        assert_eq!(store.tabs[0].title, "board prep");
        assert_eq!(
            fs::read_to_string(root.join("tabs/assistant/CURRENT_MEETING.md")).unwrap(),
            "legacy meeting"
        );
        assert!(!fs::read_to_string(root.join("CLAUDE.md"))
            .unwrap()
            .contains("Live content"));
        assert!(!root.join("CURRENT_MEETING.md").exists());
        let mut restored = TabStore::default();
        restored.ensure(&root).unwrap();
        assert_eq!(restored.tabs.len(), 1);
    }
    #[test]
    fn legacy_conversation_uses_transcript_filename_without_reading_content() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("assistant with spaces");
        let key: String = root
            .to_string_lossy()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let project = temp.path().join(".claude/projects").join(key);
        fs::create_dir_all(&project).unwrap();
        fs::write(
            project.join("12345678-session.jsonl"),
            "not JSON, never parsed",
        )
        .unwrap();
        assert_eq!(
            legacy_session(&root, temp.path()).as_deref(),
            Some("12345678-session")
        );
    }
    #[test]
    fn only_live_tab_preserves_active_guidance_when_instructions_refresh() {
        let temp = tempfile::tempdir().unwrap();
        let live = temp.path().join("live");
        let prep = temp.path().join("prep");
        fs::create_dir_all(&live).unwrap();
        fs::create_dir_all(&prep).unwrap();
        fs::write(live.join(".live-tab"), "live").unwrap();
        for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
            crate::commands::update_assistant_live_context_file(&live.join(name), true);
        }
        crate::context::write_deferred_assistant_context(&live).unwrap();
        crate::context::write_deferred_assistant_context(&prep).unwrap();
        for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
            assert!(fs::read_to_string(live.join(name))
                .unwrap()
                .contains("## Live Transcript Active"));
            assert!(!fs::read_to_string(prep.join(name))
                .unwrap()
                .contains("## Live Transcript Active"));
        }
    }
    #[test]
    fn hook_settings_are_direct_arguments_with_tab_local_paths() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Mat's tab with spaces");
        let path = settings(&dir).unwrap();
        let settings: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["args"][1],
            dir.to_string_lossy().as_ref()
        );
        assert_eq!(
            status_for_event(
                "PreToolUse",
                &serde_json::json!({"tool_name":"AskUserQuestion"})
            ),
            Some(Status::Waiting)
        );
        assert_eq!(
            status_for_event("StopFailure", &serde_json::json!({})),
            Some(Status::Error)
        );
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    #[test]
    fn saved_or_active_idle_conversations_require_close_confirmation() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TabStore::default();
        store.open(temp.path(), None, true).unwrap();
        let tab = &mut store.tabs[0];
        assert!(!requires_close_confirmation(tab));
        tab.claude_active = true;
        assert!(requires_close_confirmation(tab));
        tab.claude_active = false;
        tab.claude_session_id = Some("saved-session".into());
        assert!(requires_close_confirmation(tab));
    }
    #[test]
    fn selected_meeting_title_and_phase_survive_restore() {
        let temp = tempfile::tempdir().unwrap();
        let meeting = temp.path().join("2026-03-04-northwind-call.md");
        fs::write(
            &meeting,
            "not read: metadata was already authorized in the UI inventory",
        )
        .unwrap();
        let mut store = TabStore::default();
        let id = store.open(temp.path(), meeting.to_str(), false).unwrap();
        store.set_presentation(
            &id,
            Some("Northwind Customer Call — SSO Friction"),
            Some("prep"),
        );
        store.save(temp.path()).unwrap();
        let mut restored = TabStore::default();
        restored.ensure(temp.path()).unwrap();
        assert_eq!(
            restored.tabs[0].title,
            "Northwind Customer Call — SSO Friction"
        );
        assert_eq!(restored.tabs[0].phase.as_deref(), Some("prep"));
    }
    #[test]
    fn closing_active_tab_selects_its_neighbour_and_keeps_unrelated_focus() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TabStore::default();
        let first = store.open(temp.path(), None, true).unwrap();
        let middle = store.open(temp.path(), None, true).unwrap();
        let last = store.open(temp.path(), None, true).unwrap();
        store.remove(&last);
        assert_eq!(store.active.as_deref(), Some(middle.as_str()));
        store.remove(&first);
        assert_eq!(store.active.as_deref(), Some(middle.as_str()));
        store.remove(&middle);
        assert_eq!(store.active, None);
    }
    #[test]
    fn free_chat_titles_are_distinct_and_survive_restore() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TabStore::default();
        for _ in 0..3 {
            store.open(temp.path(), None, true).unwrap();
        }
        assert_eq!(
            store
                .tabs
                .iter()
                .map(|tab| tab.title.as_str())
                .collect::<Vec<_>>(),
            ["Chat 1", "Chat 2", "Chat 3"]
        );
        // Upgrade the generic titles saved by the first tabs implementation,
        // without changing a name that is already distinct.
        store.tabs[1].title = "Chat".into();
        store.tabs[2].title = "Chat".into();
        store.save(temp.path()).unwrap();
        let mut restored = TabStore::default();
        restored.ensure(temp.path()).unwrap();
        let titles: Vec<_> = restored.tabs.iter().map(|tab| tab.title.clone()).collect();
        assert_eq!(titles, ["Chat 1", "Chat 2", "Chat 3"]);
        let mut again = TabStore::default();
        again.ensure(temp.path()).unwrap();
        assert_eq!(
            again.tabs.iter().map(|tab| &tab.title).collect::<Vec<_>>(),
            titles.iter().collect::<Vec<_>>()
        );
    }
    #[test]
    fn project_hooks_are_idempotent_and_preserve_custom_settings() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        fs::create_dir_all(dir.join(".claude")).unwrap();
        fs::write(dir.join(".claude/settings.json"), r#"{"model":"custom-model","hooks":{"UserPromptSubmit":[{"matcher":"custom","hooks":[{"type":"command","command":"custom-hook"}]}]}}"#).unwrap();
        settings(dir).unwrap();
        settings(dir).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(value["model"], "custom-model");
        assert_eq!(
            value["hooks"]["UserPromptSubmit"].as_array().unwrap().len(),
            2
        );
        assert_eq!(
            value["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            "custom-hook"
        );
        assert!(!dir.join(".claude/settings.local.json").exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(dir.join(".claude/settings.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn project_hooks_never_follow_global_settings_symlinks() {
        let temp = tempfile::tempdir().unwrap();
        let global = temp.path().join("global");
        let tab = temp.path().join("tab");
        fs::create_dir_all(&global).unwrap();
        fs::create_dir_all(&tab).unwrap();
        fs::write(global.join("settings.json"), "untouched").unwrap();
        std::os::unix::fs::symlink(&global, tab.join(".claude")).unwrap();
        assert!(settings(&tab).is_err());
        assert_eq!(
            fs::read_to_string(global.join("settings.json")).unwrap(),
            "untouched"
        );
    }
    #[test]
    fn manual_claude_defers_context_until_real_authenticated_prompt() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path();
        set_focus_target(dir, Some("/private/meeting.md"), false).unwrap();
        for prompt in ["", " /login", "/model opus"] {
            prepare_prompt_context_with(dir, &serde_json::json!({"prompt":prompt}), |_| {
                panic!("account command must not read context")
            })
            .unwrap();
        }
        assert!(prepare_prompt_context_with(
            dir,
            &serde_json::json!({"prompt":"What did we decide?"}),
            |_| Err("not authenticated".into())
        )
        .is_err());
        let target: FocusTarget =
            serde_json::from_slice(&read_small(&dir.join(".recall-focus.json")).unwrap()).unwrap();
        assert!(!target.context_ready);
        prepare_prompt_context_with(
            dir,
            &serde_json::json!({"prompt":"What did we decide?"}),
            |meeting| {
                assert_eq!(meeting, Some("/private/meeting.md"));
                Ok(())
            },
        )
        .unwrap();
        prepare_prompt_context_with(dir, &serde_json::json!({"prompt":"Next question"}), |_| {
            panic!("do not rematerialize dropped/ready context")
        })
        .unwrap();
    }
    #[test]
    fn explicit_shell_resume_validates_id_and_active_conversation() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TabStore::default();
        store.open(temp.path(), None, true).unwrap();
        let tab = &mut store.tabs[0];
        tab.shell = true;
        assert!(shell_resume_command(tab).is_err());
        tab.claude_session_id = Some("saved-123".into());
        assert_eq!(
            shell_resume_command(tab).unwrap(),
            "claude --resume saved-123\r"
        );
        tab.claude_active = true;
        assert!(shell_resume_command(tab).is_err());
        tab.claude_active = false;
        tab.claude_session_id = Some("x;touch bad".into());
        assert!(shell_resume_command(tab).is_err());
    }
    #[test]
    fn hook_refresh_tracks_manual_claude_lifecycle_and_session() {
        let temp = tempfile::tempdir().unwrap();
        let mut store = TabStore::default();
        let id = store.open(temp.path(), None, true).unwrap();
        let dir = workspace_at(temp.path(), &id).unwrap();
        for active in [true, false] {
            private_write(
                &dir.join(STATE),
                &serde_json::to_vec(&HookState {
                    status: Status::Done,
                    session_id: Some("manual-session".into()),
                    claude_active: active,
                })
                .unwrap(),
            )
            .unwrap();
            store.refresh(temp.path()).unwrap();
            assert_eq!(store.tabs[0].claude_active, active);
            assert_eq!(
                store.tabs[0].claude_session_id.as_deref(),
                Some("manual-session")
            );
        }
    }
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn recording_identity_survives_relay_shutdown_without_guessing_an_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("recording.pid");
        assert!(recording_capture_id_at(&path).is_none());
        fs::write(&path, "42").unwrap();
        let first = recording_capture_id_at(&path).unwrap();
        assert!(first.starts_with("recording:42:"));
        assert_eq!(recording_capture_id_at(&path).as_ref(), Some(&first));
        fs::write(&path, "43").unwrap();
        assert_ne!(recording_capture_id_at(&path).as_ref(), Some(&first));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn live_without_a_saved_meeting_never_materializes_general_history() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("live");
        fs::create_dir_all(&workspace).unwrap();
        fs::write(workspace.join(".live-tab"), "live").unwrap();
        fs::write(workspace.join("CURRENT_MEETING.md"), "stale meeting").unwrap();
        fs::write(workspace.join("ASSISTANT_CONTEXT.md"), "stale history").unwrap();
        let config = minutes_core::Config {
            output_dir: temp.path().join("missing-corpus-must-not-be-read"),
            ..Default::default()
        };
        crate::commands::materialize_recall_terminal_context(&workspace, &config, None).unwrap();
        assert!(!workspace.join("CURRENT_MEETING.md").exists());
        assert!(!workspace.join("ASSISTANT_CONTEXT.md").exists());
    }

    #[test]
    fn live_admission_at_cap_is_idempotent_and_has_no_invented_meeting() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let mut store = TabStore::default();
        for _ in 0..LIMIT {
            store.open(root, None, true).unwrap();
        }
        let capture = ActiveCapture {
            id: "relay:producer-a".into(),
            title: "Live call".into(),
        };
        store
            .sync_live_capture_with(root, Some(&capture), |_| None)
            .unwrap();
        assert_eq!(store.tabs.len(), LIMIT);
        assert_eq!(store.pending_live_id.as_deref(), Some("relay:producer-a"));
        let closed = store.tabs[0].id.clone();
        store.remove(&closed);
        store
            .sync_live_capture_with(root, Some(&capture), |_| None)
            .unwrap();
        let id = store.tabs[0].id.clone();
        assert!(store.tabs[0].live);
        assert_eq!(store.tabs[0].title, "Live call");
        assert!(store.tabs[0].meeting_path.is_none());
        assert!(store.pending_live_id.is_none());
        assert_eq!(store.admit_capture(root, &capture).unwrap(), id);
        assert_eq!(store.tabs.len(), LIMIT);
        store.sync_live_capture_with(root, None, |_| None).unwrap();
        assert!(!store.tabs[0].live);
        assert!(store.tabs[0].phase.is_none());
        assert!(store.tabs[0].meeting_path.is_none());
        assert!(!workspace_at(root, &id).unwrap().join(".live-tab").exists());
    }

    #[test]
    fn failed_then_retried_exact_job_moves_only_its_tab_and_focus_on_success() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("assistant");
        let meeting = temp.path().join("renamed-meeting.md");
        fs::write(
            &meeting,
            "synthetic artifact; never materialized by completion",
        )
        .unwrap();
        let mut store = TabStore::default();
        let prep = store.open(&root, None, true).unwrap();
        let capture = ActiveCapture {
            id: "relay:producer-a".into(),
            title: "Live call".into(),
        };
        store
            .sync_live_capture_with(&root, Some(&capture), |_| None)
            .unwrap();
        let id = store.tabs[0].id.clone();
        store
            .record_capture_completion(&root, "relay:other-producer", Some("wrong-job"), None)
            .unwrap();
        assert!(store.tabs[0].capture_job_id.is_none());
        store
            .record_capture_completion(&root, &capture.id, Some("job-a"), None)
            .unwrap();
        let mut job: minutes_core::jobs::ProcessingJob = serde_json::from_value(serde_json::json!({
            "id":"job-a", "mode":"live-transcript", "content_type":"meeting", "title":"Saved call",
            "audio_path":"synthetic.wav", "output_path":meeting, "state":"failed", "created_at":chrono::Local::now()
        })).unwrap();
        for state in [
            minutes_core::jobs::JobState::Failed,
            minutes_core::jobs::JobState::Queued,
            minutes_core::jobs::JobState::Saving,
        ] {
            job.state = state;
            store
                .sync_live_capture_with(&root, None, |tab| {
                    if tab.capture_job_id.as_deref() == Some(&job.id) {
                        completed_job(&job)
                    } else {
                        None
                    }
                })
                .unwrap();
            assert!(store.tabs[0].meeting_path.is_none());
            assert!(!store.tabs[0].capture_complete);
        }
        job.state = minutes_core::jobs::JobState::Complete;
        store
            .sync_live_capture_with(&root, None, |tab| {
                if tab.capture_job_id.as_deref() == Some(&job.id) {
                    completed_job(&job)
                } else {
                    None
                }
            })
            .unwrap();
        let tab = store.tabs.iter().find(|tab| tab.id == id).unwrap();
        assert!(tab.capture_complete);
        assert_eq!(tab.phase.as_deref(), Some("debrief"));
        assert_eq!(tab.title, "Saved call");
        assert_eq!(tab.meeting_path.as_deref(), meeting.to_str());
        let dir = workspace_at(&root, &id).unwrap();
        let focus: FocusTarget =
            serde_json::from_slice(&fs::read(dir.join(".recall-focus.json")).unwrap()).unwrap();
        assert_eq!(focus.meeting_path.as_deref(), meeting.to_str());
        assert!(!focus.context_ready);
        assert!(
            !dir.join("CURRENT_MEETING.md").exists(),
            "completion never reads the meeting into prompt context"
        );
        assert!(store
            .tabs
            .iter()
            .find(|tab| tab.id == prep)
            .unwrap()
            .meeting_path
            .is_none());
        store.save(&root).unwrap();
        fs::remove_file(&meeting).unwrap();
        let mut restored = TabStore::default();
        restored.ensure(&root).unwrap();
        let tab = restored.tabs.iter().find(|tab| tab.id == id).unwrap();
        assert!(tab.meeting_path.is_none());
        assert!(tab.note.as_ref().unwrap().contains("deleted"));
        let focus: FocusTarget =
            serde_json::from_slice(&fs::read(dir.join(".recall-focus.json")).unwrap()).unwrap();
        assert!(focus.meeting_path.is_none());
    }

    #[test]
    fn missing_artifact_or_failed_focus_write_cannot_mark_capture_complete() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("assistant");
        let mut store = TabStore::default();
        let id = store.open(&root, None, true).unwrap();
        let tab = store.tabs.iter_mut().find(|tab| tab.id == id).unwrap();
        finish_tab_capture(
            &root,
            tab,
            temp.path().join("missing.md").to_str().unwrap(),
            None,
        )
        .unwrap();
        assert!(!tab.capture_complete);
        let meeting = temp.path().join("saved.md");
        fs::write(&meeting, "synthetic").unwrap();
        let blocked = temp.path().join("not-a-directory");
        fs::write(&blocked, "blocked").unwrap();
        assert!(finish_tab_capture(&blocked, tab, meeting.to_str().unwrap(), None).is_err());
        assert!(
            !tab.capture_complete,
            "observer must be able to retry a focus-file failure"
        );
    }
}
