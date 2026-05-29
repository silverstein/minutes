use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::thread::JoinHandle;
use tauri::Emitter;

pub const ASSISTANT_SESSION_ID: &str = "assistant";
const MAX_SESSIONS: usize = 1;

pub struct PtySession {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub reader_handle: Option<JoinHandle<()>>,
    pub child: Box<dyn portable_pty::Child + Send>,
    pub context_dir: PathBuf,
    pub title: String,
    pub command: String,
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

    pub fn session_command(&self, session_id: &str) -> Option<String> {
        self.sessions
            .get(session_id)
            .map(|session| session.command.clone())
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

        // Build a rich PATH so agent CLIs are found from a GUI app.
        // macOS GUI processes get a stripped PATH by default.
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
        // Append existing PATH entries that aren't already in our list
        if let Ok(existing) = std::env::var("PATH") {
            for p in existing.split(':') {
                if !path_dirs.contains(&p.to_string()) {
                    path_dirs.push(p.to_string());
                }
            }
        }
        // Also check for npm global bin
        let npm_global = home.join(".npm-global/bin");
        if npm_global.exists() {
            path_dirs.insert(0, npm_global.display().to_string());
        }

        cmd.env("PATH", path_dirs.join(":"));
        cmd.env("HOME", home.display().to_string());
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        cmd.env("LANG", "en_US.UTF-8");

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

    pub fn kill_session(&mut self, session_id: &str) -> Option<PathBuf> {
        if let Some(mut session) = self.sessions.remove(session_id) {
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
                let deadline =
                    std::time::Instant::now() + std::time::Duration::from_millis(300);
                while !handle.is_finished() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                if handle.is_finished() {
                    let _ = handle.join();
                }
                // else: detach — never block on a ConPTY reader that won't see EOF.
            }
            Some(session.context_dir)
        } else {
            None
        }
    }

    pub fn kill_all(&mut self) -> Vec<PathBuf> {
        let ids: Vec<String> = self.sessions.keys().cloned().collect();
        let mut dirs = Vec::new();
        for id in ids {
            if let Some(dir) = self.kill_session(&id) {
                dirs.push(dir);
            }
        }
        dirs
    }
}
