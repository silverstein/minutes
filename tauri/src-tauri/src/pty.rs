use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::thread::JoinHandle;
use tauri::Emitter;

pub const ASSISTANT_SESSION_ID: &str = "assistant";
const MAX_SESSIONS: usize = 1;

/// A signed app's assistant must use that same app's CLI, including when the
/// regular app and Minutes Dev are installed at different versions.
fn bundled_cli_directory(executable: &std::path::Path) -> Option<PathBuf> {
    let directory = executable.parent()?;
    if directory.file_name()? != "MacOS"
        || directory.parent()?.file_name()? != "Contents"
        || !directory.join("minutes").is_file()
    {
        return None;
    }
    Some(directory.to_path_buf())
}

/// Environment overrides shared by the interactive assistant PTY and any
/// bounded probe that must describe that exact session. GUI apps on macOS get
/// a stripped PATH, so probing with the ambient process environment can report
/// "not authenticated" even while the PTY can launch the same agent normally.
/// Session-identity variables a parent Claude Code process sets for its own
/// children. They describe that parent, never the assistant Minutes starts.
const INHERITED_SESSION_MARKERS: [&str; 4] = [
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SSE_PORT",
];

pub fn assistant_process_environment() -> Vec<(String, String)> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let mut path_dirs: Vec<String> = vec![
        format!("{}/.cargo/bin", home.display()),
        format!(
            "{}/Library/Application Support/fnm/node-versions/default/bin",
            home.display()
        ),
        format!("{}/.local/bin", home.display()),
        "/opt/homebrew/bin".into(),
        "/opt/homebrew/sbin".into(),
        "/usr/local/bin".into(),
        "/usr/bin".into(),
        "/bin".into(),
        "/usr/sbin".into(),
        "/sbin".into(),
    ];
    if let Ok(existing) = std::env::var("PATH") {
        for path in existing.split(':') {
            if !path_dirs.contains(&path.to_string()) {
                path_dirs.push(path.to_string());
            }
        }
    }
    let npm_global = home.join(".npm-global/bin");
    if npm_global.exists() {
        path_dirs.insert(0, npm_global.display().to_string());
    }
    if let Some(directory) = std::env::current_exe()
        .ok()
        .and_then(|executable| bundled_cli_directory(&executable))
    {
        path_dirs.insert(0, directory.display().to_string());
    }

    vec![
        ("PATH".into(), path_dirs.join(":")),
        ("HOME".into(), home.display().to_string()),
        ("TERM".into(), "xterm-256color".into()),
        ("COLORTERM".into(), "truecolor".into()),
        ("LANG".into(), "en_US.UTF-8".into()),
    ]
}

#[cfg(test)]
mod bundled_cli_tests {
    use super::*;

    #[test]
    fn app_assistant_resolves_its_own_cli_without_changing_the_global_link() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("Minutes Dev.app/Contents/MacOS");
        std::fs::create_dir_all(&directory).unwrap();
        let executable = directory.join("minutes-app");
        assert_eq!(bundled_cli_directory(&executable), None);
        std::fs::write(directory.join("minutes"), b"fixture").unwrap();
        assert_eq!(bundled_cli_directory(&executable), Some(directory));
        assert_eq!(
            bundled_cli_directory(&root.path().join("target/debug/minutes-app")),
            None
        );
    }
}

#[cfg(windows)]
fn terminate_process_tree(process_id: Option<u32>) {
    let Some(process_id) = process_id else {
        return;
    };
    let process_id = process_id.to_string();
    let _ = std::process::Command::new("taskkill")
        .args(["/T", "/F", "/PID", process_id.as_str()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

pub struct PtySession {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub reader_handle: Option<JoinHandle<()>>,
    pub child: Box<dyn portable_pty::Child + Send>,
    pub context_dir: PathBuf,
    pub title: String,
    pub command: String,
    /// The `COLORFGBG` theme the child started with; terminal apps choose
    /// their colours once, at startup.
    pub theme_dark: Option<bool>,
}

pub struct SpawnConfig {
    pub session_id: String,
    pub app_handle: tauri::AppHandle,
    pub command: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub context_dir: PathBuf,
    pub title: String,
    /// Tauri window label to emit PTY data events to. Defaults to "main"
    /// so the embedded Recall panel receives output.
    pub target_window: String,
    /// Effective UI theme at spawn time (`Some(true)` = dark). Exported to the
    /// child as `COLORFGBG` so TUIs can pick a matching palette without a
    /// terminal round trip. `None` leaves it unset.
    pub theme_dark: Option<bool>,
}

#[derive(Default)]
pub struct PtyManager {
    sessions: HashMap<String, PtySession>,
}

impl PtyManager {
    pub fn assistant_session_id(&self) -> Option<String> {
        self.sessions
            .contains_key(ASSISTANT_SESSION_ID)
            .then(|| ASSISTANT_SESSION_ID.to_string())
    }

    pub fn session_title(&self, session_id: &str) -> Option<String> {
        self.sessions
            .get(session_id)
            .map(|session| session.title.clone())
    }

    pub fn session_theme_dark(&self, session_id: &str) -> Option<bool> {
        self.sessions
            .get(session_id)
            .and_then(|session| session.theme_dark)
    }

    pub fn session_command(&self, session_id: &str) -> Option<String> {
        self.sessions
            .get(session_id)
            .map(|session| session.command.clone())
    }

    pub fn session_context_dir(&self, session_id: &str) -> Option<PathBuf> {
        self.sessions
            .get(session_id)
            .map(|session| session.context_dir.clone())
    }

    pub fn set_session_title(
        &mut self,
        session_id: &str,
        title: impl Into<String>,
    ) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or("Session not found")?;
        session.title = title.into();
        Ok(())
    }

    /// Spawn a new PTY session running the given command.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(&mut self, cfg: SpawnConfig, cols: u16, rows: u16) -> Result<(), String> {
        if self.sessions.len() >= MAX_SESSIONS {
            return Err("Minutes only supports one assistant session at a time.".into());
        }

        if self.sessions.contains_key(&cfg.session_id) {
            return Err(format!("Session '{}' is already running.", cfg.session_id));
        }

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Failed to open PTY: {}", e))?;

        let mut cmd = CommandBuilder::new(&cfg.command);
        for arg in &cfg.args {
            cmd.arg(arg);
        }
        cmd.cwd(&cfg.cwd);

        for (name, value) in assistant_process_environment() {
            cmd.env(name, value);
        }
        // Minutes launched from inside a Claude Code session (an agent running
        // the dev installer, or `open` from its shell) inherits that session's
        // markers, and the assistant started here then believes it is a child
        // session and turns off transcript saving. It is a fresh session.
        for name in INHERITED_SESSION_MARKERS {
            cmd.env_remove(name);
        }
        // Identify the host the way real terminals do. Codex's terminal
        // detection keys on TERM_PROGRAM (Ghostty, iTerm, Warp, kitty, ...)
        // and consults COLORFGBG; with neither set it fell back to a light
        // palette inside the dark app. The trailing COLORFGBG field is the
        // background: 0 = dark, 15 = light.
        cmd.env("TERM_PROGRAM", "Minutes");
        // The app version from tauri.conf, not the crate's -- this package
        // is versioned 0.1.0 and the app is not.
        cmd.env(
            "TERM_PROGRAM_VERSION",
            cfg.app_handle.package_info().version.to_string(),
        );
        if let Some(dark) = cfg.theme_dark {
            cmd.env("COLORFGBG", if dark { "15;0" } else { "0;15" });
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .map_err(|e| format!("Failed to spawn {}: {}", cfg.command, e))?;

        let writer = pair
            .master
            .take_writer()
            .map_err(|e| format!("Failed to get PTY writer: {}", e))?;

        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|e| format!("Failed to get PTY reader: {}", e))?;

        // Reader thread: PTY stdout → base64 → Tauri event
        // Emit to the target window (typically "main" for the embedded Recall panel)
        let session_id = cfg.session_id;
        let context_dir = cfg.context_dir;
        let window_label = cfg.target_window;
        let event_name = format!("pty:data:{}", session_id);
        let exit_event = format!("pty:exit:{}", session_id);
        let app_handle = cfg.app_handle;
        let session_id_for_insert = session_id.clone();
        let context_dir_for_insert = context_dir.clone();
        let reader_handle = std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut emit_count: u64 = 0;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        eprintln!("[pty] reader EOF after {} emits", emit_count);
                        app_handle.emit_to(&window_label, &exit_event, ()).ok();
                        break;
                    }
                    Ok(n) => {
                        use base64::Engine;
                        let encoded = base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                        if let Err(e) = app_handle.emit_to(&window_label, &event_name, &encoded) {
                            eprintln!(
                                "[pty] emit_to error: {} (label: {}, event: {})",
                                e, window_label, event_name
                            );
                        }
                        emit_count += 1;
                        if emit_count == 1 {
                            eprintln!(
                                "[pty] first emit_to: label={} event={} bytes={}",
                                window_label, event_name, n
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!("[pty] reader error: {} after {} emits", e, emit_count);
                        app_handle.emit_to(&window_label, &exit_event, ()).ok();
                        break;
                    }
                }
            }
        });

        self.sessions.insert(
            session_id_for_insert,
            PtySession {
                master: pair.master,
                writer,
                reader_handle: Some(reader_handle),
                child,
                context_dir: context_dir_for_insert,
                title: cfg.title,
                command: cfg.command,
                theme_dark: cfg.theme_dark,
            },
        );

        Ok(())
    }

    pub fn write_input(&mut self, session_id: &str, data: &[u8]) -> Result<(), String> {
        let session = self
            .sessions
            .get_mut(session_id)
            .ok_or("Session not found")?;
        session
            .writer
            .write_all(data)
            .map_err(|e| format!("Write failed: {}", e))?;
        session
            .writer
            .flush()
            .map_err(|e| format!("Flush failed: {}", e))
    }

    pub fn resize(&self, session_id: &str, cols: u16, rows: u16) -> Result<(), String> {
        let session = self.sessions.get(session_id).ok_or("Session not found")?;
        session
            .master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(|e| format!("Resize failed: {}", e))
    }

    pub fn take_session(&mut self, session_id: &str) -> Option<PtySession> {
        self.sessions.remove(session_id)
    }

    pub fn take_all_sessions(&mut self) -> Vec<PtySession> {
        self.sessions.drain().map(|(_, session)| session).collect()
    }
}

pub fn kill_session(mut session: PtySession) -> PathBuf {
    // Kill the full process tree FIRST on Windows: `taskkill /T /F` walks the
    // tree from the live leader PID, so it must run before `child.kill()` reaps
    // the leader. Otherwise a dead or reused PID could orphan the node / npx
    // minutes-mcp grandchildren (thanks @mquinn614). `child.kill()` below is the
    // real kill on Unix and a harmless no-op on Windows.
    #[cfg(windows)]
    let process_id = session.child.process_id();
    #[cfg(windows)]
    terminate_process_tree(process_id);
    session.child.kill().ok();
    if let Some(handle) = session.reader_handle.take() {
        // The reader thread blocks on `reader.read()` of the PTY
        // master and only breaks on EOF / error. On Windows ConPTY,
        // killing the child process does NOT deliver EOF to the
        // master — the pseudoconsole keeps the pipe open — so the
        // read never returns and an unconditional `join()` here hangs
        // forever. Because every quit path runs
        // `cleanup_before_process_exit` -> `kill_all` -> `kill_session`,
        // that wedges the whole app on exit (window hides, process
        // never dies, "Not Responding" — Task Manager required).
        //
        // On macOS/Unix killing the child closes the slave PTY, the
        // master read returns Ok(0), and the thread exits immediately,
        // so the join is instant there (which is why this only bites
        // on Windows).
        //
        // Bounded wait, then detach: give the reader a brief window to
        // exit cleanly (it does wherever EOF is delivered), otherwise
        // drop the handle. The reader holds no lock the rest of the
        // app needs and dies with the process on exit, so detaching is
        // safe and shutdown can never wedge on it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        if handle.is_finished() {
            let _ = handle.join();
        }
        // else: detach — never block on a ConPTY reader that won't see EOF.
    }
    session.context_dir
}

pub fn kill_all(sessions: Vec<PtySession>) -> Vec<PathBuf> {
    sessions.into_iter().map(kill_session).collect()
}
