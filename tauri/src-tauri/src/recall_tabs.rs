//! Recall tab metadata and Claude hook protocol. No capture thread calls this module.
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
    pub meeting_path: Option<String>,
    pub claude_session_id: Option<String>,
    #[serde(default)]
    pub capture_session_id: Option<String>,
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
    pub pending_live_path: Option<String>,
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
                    meeting_path: fs::read(
                        root.parent().unwrap_or(root).join("recall-workspace.json"),
                    )
                    .ok()
                    .and_then(|raw| serde_json::from_slice::<serde_json::Value>(&raw).ok())
                    .and_then(|value| value["current_meeting_path"].as_str().map(str::to_string)),
                    claude_session_id: legacy,
                    capture_session_id: None,
                    capture_complete: false,
                    status: Status::Done,
                    live: false,
                    note: None,
                });
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
        self.loaded = true;
        self.refresh(root)?;
        for tab in &mut self.tabs {
            tab.live = false;
            tab.status = Status::Done;
            let dir = workspace_at(root, &tab.id)?;
            let _ = fs::remove_file(dir.join(STATE));
            let _ = fs::remove_file(dir.join(".live-tab"));
            for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
                if dir.join(name).exists() {
                    crate::commands::update_assistant_live_context_file(&dir.join(name), false);
                }
            }
            if (tab.capture_session_id.is_none() || tab.capture_complete)
                && tab
                    .meeting_path
                    .as_ref()
                    .is_some_and(|p| !Path::new(p).is_file())
            {
                tab.meeting_path = None;
                tab.capture_session_id = None;
                tab.title = "Chat".into();
                tab.note = Some(
                    "The meeting for this tab was deleted. This conversation is now a plain chat."
                        .into(),
                );
                let dir = workspace_at(root, &tab.id)?;
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
        self.save(root)
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
            .unwrap_or_else(|| "Chat".into());
        self.tabs.push(Tab {
            id: id.clone(),
            title,
            meeting_path: meeting.map(str::to_string),
            claude_session_id: None,
            capture_session_id: None,
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
        hooks.insert(event.into(), serde_json::json!([{ "hooks": [{ "type": "command", "command": executable, "args": ["--recall-hook", workspace, event], "timeout": 5 }] }]));
    }
    let path = workspace.join(".recall-hooks.json");
    private_write(
        &path,
        &serde_json::to_vec_pretty(&serde_json::json!({"hooks": hooks}))
            .map_err(|e| e.to_string())?,
    )?;
    Ok(path)
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
        let live_path = if recording.recording {
            recording
                .wav_path
                .as_deref()
                .map(|p| Path::new(p).with_extension("md"))
        } else if live.active {
            live.jsonl_path
                .as_deref()
                .map(|p| Path::new(p).with_extension("md"))
        } else {
            None
        };
        manager.tabs.pending_live_path = None;
        if let Some(path) = &live_path {
            let path = path.to_string_lossy().into_owned();
            if manager.tabs.tabs.len() >= LIMIT
                && !manager
                    .tabs
                    .tabs
                    .iter()
                    .any(|t| t.meeting_path.as_deref() == Some(&path))
            {
                manager.tabs.pending_live_path = Some(path.clone());
            }
            if !manager
                .tabs
                .tabs
                .iter()
                .any(|t| t.meeting_path.as_deref() == Some(&path))
                && manager.tabs.tabs.len() < LIMIT
            {
                let active = manager.tabs.active.clone();
                let _ = manager.tabs.open(&root, Some(&path), false);
                manager.tabs.active = active;
            }
        }
        let capture_id = live.session_id.clone().or_else(|| {
            recording
                .wav_path
                .as_deref()
                .and_then(|p| {
                    minutes_core::context_store::get_session_for_artifact(p)
                        .ok()
                        .flatten()
                })
                .map(|s| s.id)
        });
        for tab in &mut manager.tabs.tabs {
            let is_live = live_path
                .as_ref()
                .is_some_and(|p| tab.meeting_path.as_deref() == p.to_str());
            if is_live {
                tab.capture_session_id = capture_id.clone();
                tab.capture_complete = false;
            }
            if !is_live && !tab.capture_complete {
                if let Some(id) = &tab.capture_session_id {
                    if let Ok(links) = minutes_core::context_store::list_links_for_session(id) {
                        if let Some(link) = links.iter().find(|l| {
                            l.kind == minutes_core::context_store::ContextLinkKind::MarkdownArtifact
                        }) {
                            tab.capture_complete = true;
                            if tab.meeting_path.as_deref() != Some(&link.target) {
                                tab.meeting_path = Some(link.target.clone());
                                tab.title = Path::new(&link.target)
                                    .file_stem()
                                    .and_then(|p| p.to_str())
                                    .unwrap_or("Debrief")
                                    .replace('-', " ");
                                if let Ok(dir) = workspace_at(&root, &tab.id) {
                                    let _ = crate::context::clear_active_meeting_context(&dir);
                                    let _ = crate::context::assistant_skills::queue_focus_notice(&dir, crate::context::assistant_skills::FocusNotice::Meeting,
                                        "Recording ended. This conversation is now the meeting debrief. Read the tab's CURRENT_MEETING.md if Minutes has shared it for this question.");
                                }
                            }
                        }
                    }
                }
            }
            if tab.live != is_live {
                tab.live = is_live;
                if let Ok(dir) = workspace_at(&root, &tab.id) {
                    if is_live {
                        let _ = private_write(&dir.join(".live-tab"), b"live\n");
                    } else {
                        let _ = fs::remove_file(dir.join(".live-tab"));
                    }
                    for name in crate::context::ASSISTANT_INSTRUCTION_FILES {
                        crate::commands::update_assistant_live_context_file(
                            &dir.join(name),
                            is_live,
                        );
                    }
                }
            }
        }
        manager.tabs.tabs.sort_by_key(|t| !t.live);
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
) -> Result<(), String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    if !manager.tabs.tabs.iter().any(|t| t.id == tab_id) {
        return Err("Recall tab not found".into());
    }
    manager.tabs.active = Some(tab_id);
    manager.tabs.save(&crate::context::workspace_dir())
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
        if (tab.status == Status::Working || tab.live) && !confirmed.unwrap_or(false) {
            return Err("RECALL_CLOSE_CONFIRM".into());
        }
        manager.tabs.tabs.retain(|t| t.id != tab_id);
        if manager.tabs.active.as_deref() == Some(&tab_id) {
            manager.tabs.active = manager.tabs.tabs.first().map(|t| t.id.clone());
        }
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
) -> Result<TabStore, String> {
    let mut manager = state.pty_manager.lock().map_err(|_| "Recall unavailable")?;
    manager.tabs.open(
        &crate::context::workspace_dir(),
        meeting_path.as_deref(),
        new_tab.unwrap_or(false),
    )?;
    Ok(manager.tabs.clone())
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
        let mut store = TabStore::default();
        store.ensure(&root).unwrap();
        assert_eq!(store.tabs.len(), 1);
        assert_eq!(store.tabs[0].id, "assistant");
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
