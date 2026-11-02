//! Serial, bounded runner for the user-configured commands in `[hooks]`.
//!
//! Hooks run one at a time, in order, on a background thread, so a slow or
//! hung command never holds up the capture or dictation that triggered it.
//! Each run has a deadline, and failures are logged rather than surfaced. A
//! hook runs with the user's own authority, so this is not a sandbox.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::bounded_child::{self, BoundedCommand, ChildBudget, ChildRun, StdoutTarget};

/// Longest a single hook may run before its process tree is killed.
pub const HOOK_DEADLINE: Duration = Duration::from_secs(5 * 60);
const HOOK_STDOUT_LIMIT: u64 = 1024 * 1024;
const HOOK_STDERR_TAIL: usize = 4 * 1024;
const HOOK_QUEUE_CAPACITY: usize = 64;

/// One hook invocation.
pub struct HookRequest {
    /// Hook name for logs. The command itself is never logged because it may
    /// hold a token, and `stdin` may hold the user's words.
    pub name: &'static str,
    /// User-configured command line.
    pub command: String,
    /// Passed to the command as its last argument. An empty value still
    /// arrives as an empty `$1` on Unix and is omitted on Windows.
    pub argument: String,
    /// Bytes written to the command's stdin. Stdin is closed when absent.
    pub stdin: Option<Vec<u8>>,
}

#[derive(Default)]
struct QueueState {
    waiting: VecDeque<HookRequest>,
    running: bool,
    worker_started: bool,
}

struct HookQueue {
    state: Mutex<QueueState>,
    changed: Condvar,
    capacity: usize,
    deadline: Duration,
}

impl HookQueue {
    fn new(capacity: usize, deadline: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(QueueState::default()),
            changed: Condvar::new(),
            capacity,
            deadline,
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, QueueState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Queue a request without blocking. A full queue drops the request.
    fn enqueue(self: &Arc<Self>, request: HookRequest) {
        let mut state = self.lock();
        if state.waiting.len() >= self.capacity {
            tracing::warn!(hook = request.name, "hook queue is full; dropping this run");
            return;
        }
        let name = request.name;
        state.waiting.push_back(request);
        if !state.worker_started {
            let queue = Arc::clone(self);
            match std::thread::Builder::new()
                .name("minutes-hooks".into())
                .spawn(move || queue.work())
            {
                Ok(_) => state.worker_started = true,
                Err(error) => {
                    state.waiting.clear();
                    tracing::warn!(hook = name, %error, "hook worker failed to start");
                    return;
                }
            }
        }
        self.changed.notify_all();
    }

    fn work(&self) {
        loop {
            let request = {
                let mut state = self.lock();
                state.running = false;
                self.changed.notify_all();
                loop {
                    if let Some(request) = state.waiting.pop_front() {
                        state.running = true;
                        break request;
                    }
                    state = self
                        .changed
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            };
            execute(request, self.deadline);
        }
    }

    /// Wait until nothing is queued or running. Returns false on timeout.
    fn drain(&self, timeout: Duration) -> bool {
        let give_up = Instant::now() + timeout;
        let mut state = self.lock();
        while state.running || !state.waiting.is_empty() {
            let Some(remaining) = give_up.checked_duration_since(Instant::now()) else {
                return false;
            };
            state = self
                .changed
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
        true
    }
}

fn global_queue() -> &'static Arc<HookQueue> {
    static QUEUE: OnceLock<Arc<HookQueue>> = OnceLock::new();
    QUEUE.get_or_init(|| HookQueue::new(HOOK_QUEUE_CAPACITY, HOOK_DEADLINE))
}

/// Queue a hook to run after the ones already waiting. Never blocks.
pub fn enqueue(request: HookRequest) {
    global_queue().enqueue(request);
}

/// Wait for queued hooks to finish. A short-lived process must call this
/// before exiting, or its hooks die with it. Returns false on timeout.
pub fn drain(timeout: Duration) -> bool {
    global_queue().drain(timeout)
}

fn execute(request: HookRequest, deadline: Duration) {
    let name = request.name;
    tracing::info!(hook = name, "running hook");
    match run_request(request, deadline) {
        Ok(run) if run.timed_out => {
            tracing::warn!(hook = name, ?deadline, "hook timed out and was killed");
        }
        Ok(run) if run.output.status.success() => {
            tracing::info!(hook = name, "hook completed");
        }
        Ok(run) => {
            tracing::warn!(
                hook = name,
                exit_code = run.output.status.code(),
                stderr = %String::from_utf8_lossy(&run.output.stderr),
                "hook failed"
            );
        }
        Err(error) => tracing::warn!(hook = name, %error, "hook could not run"),
    }
}

fn run_request(request: HookRequest, deadline: Duration) -> std::io::Result<ChildRun> {
    let mut command = shell_command(&request.command, &request.argument);
    let stdout = StdoutTarget::Capture {
        max_bytes: HOOK_STDOUT_LIMIT,
    };
    let budget = ChildBudget {
        wall_clock: deadline,
        stderr_tail: HOOK_STDERR_TAIL,
    };
    match request.stdin {
        // A hook may reasonably ignore stdin and exit without reading it.
        Some(bytes) => bounded_child::run_allowing_child_to_close_stdin(
            &mut command,
            Box::new(std::io::Cursor::new(bytes)),
            stdout,
            budget,
        ),
        None => bounded_child::run(&mut command, None, stdout, budget),
    }
}

#[cfg(not(windows))]
fn shell_command(user_command: &str, argument: &str) -> BoundedCommand {
    let mut command = BoundedCommand::new("sh");
    command
        .arg("-c")
        .arg(format!("{user_command} \"$1\""))
        .arg("--")
        .arg(argument);
    let home = std::env::var_os("HOME");
    if let Some(path) = hook_search_path(std::env::var_os("PATH"), home.as_deref().map(Path::new)) {
        command.env("PATH", path);
    }
    command
}

#[cfg(windows)]
fn shell_command(user_command: &str, argument: &str) -> BoundedCommand {
    let mut command = BoundedCommand::new("cmd");
    command.raw_arg("/S").raw_arg("/C");
    if argument.is_empty() {
        command.raw_arg(format!("\"{user_command}\""));
    } else {
        // cmd.exe expands the variable before parsing, so the surrounding
        // quotes are what keep `&` and similar characters in a file name from
        // acting as syntax. Double quotes cannot appear in Windows paths, so
        // the name cannot close them.
        command
            .env("MINUTES_HOOK_ARGUMENT", argument)
            .raw_arg(format!("\"{user_command} \"%MINUTES_HOOK_ARGUMENT%\"\""));
    }
    command
}

/// `PATH` for a hook: the inherited value, then common user tool folders.
///
/// Mac apps opened from the desktop inherit a minimal `PATH` that omits the
/// folders where `claude`, `codex`, and Homebrew tools live. Existing entries
/// stay first, so they keep precedence.
#[cfg_attr(windows, allow(dead_code))]
fn hook_search_path(existing: Option<OsString>, home: Option<&Path>) -> Option<OsString> {
    let mut entries: Vec<std::path::PathBuf> = existing
        .as_deref()
        .map(|path| std::env::split_paths(path).collect())
        .unwrap_or_default();
    let mut extra: Vec<std::path::PathBuf> = Vec::new();
    if let Some(home) = home {
        extra.push(home.join(".local/bin"));
        extra.push(home.join(".cargo/bin"));
    }
    extra.push("/opt/homebrew/bin".into());
    extra.push("/usr/local/bin".into());
    for entry in extra {
        if !entries.contains(&entry) {
            entries.push(entry);
        }
    }
    std::env::join_paths(entries).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn request(command: String, argument: &str, stdin: Option<&str>) -> HookRequest {
        HookRequest {
            name: "test",
            command,
            argument: argument.into(),
            stdin: stdin.map(|text| text.as_bytes().to_vec()),
        }
    }

    #[cfg(unix)]
    fn script(dir: &Path, name: &str, body: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    #[test]
    #[cfg(unix)]
    fn hook_gets_path_argument_and_text_only_on_stdin() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("out");
        let body = format!(
            "printf '%s' \"$1\" > '{0}.arg'\ncat > '{0}.stdin'\nprintf '%s' \"$*\" > '{0}.argv'\nenv > '{0}.env'",
            out.display()
        );
        let command = script(dir.path(), "hook.sh", &body);
        let run = run_request(
            request(
                command,
                "/tmp/some note.md",
                Some("{\"text\":\"canary words\"}"),
            ),
            Duration::from_secs(10),
        )
        .unwrap();
        assert!(run.output.status.success());

        let read =
            |suffix: &str| std::fs::read_to_string(format!("{}.{suffix}", out.display())).unwrap();
        assert_eq!(read("arg"), "/tmp/some note.md");
        assert_eq!(read("stdin"), "{\"text\":\"canary words\"}");
        assert!(!read("argv").contains("canary"));
        assert!(!read("env").contains("canary"));
    }

    #[test]
    #[cfg(unix)]
    fn hook_that_ignores_stdin_still_succeeds() {
        let dir = TempDir::new().unwrap();
        let command = script(dir.path(), "hook.sh", "exit 0");
        let large = "x".repeat(4 * 1024 * 1024);
        let run = run_request(request(command, "", Some(&large)), Duration::from_secs(10)).unwrap();
        assert!(run.output.status.success());
    }

    #[test]
    #[cfg(unix)]
    fn hung_hook_is_killed_at_the_deadline_with_its_children() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("survived");
        let body = format!("(sleep 2; touch '{}') &\nsleep 30", marker.display());
        let command = script(dir.path(), "hook.sh", &body);
        let started = Instant::now();
        let run = run_request(request(command, "", None), Duration::from_millis(300)).unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the hook's children must die with it");
    }

    #[test]
    #[cfg(unix)]
    fn hooks_run_one_at_a_time_and_in_order() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("log");
        let body = format!(
            "echo \"start $1\" >> '{0}'\nsleep 0.1\necho \"end $1\" >> '{0}'",
            log.display()
        );
        let command = script(dir.path(), "hook.sh", &body);
        let queue = HookQueue::new(8, Duration::from_secs(10));
        for name in ["a", "b", "c"] {
            queue.enqueue(request(command.clone(), name, None));
        }
        assert!(queue.drain(Duration::from_secs(10)));
        assert_eq!(
            std::fs::read_to_string(&log).unwrap(),
            "start a\nend a\nstart b\nend b\nstart c\nend c\n"
        );
    }

    #[test]
    #[cfg(unix)]
    fn enqueue_does_not_wait_and_a_full_queue_drops_new_runs() {
        let dir = TempDir::new().unwrap();
        let log = dir.path().join("log");
        let body = format!("sleep 0.5\necho \"$1\" >> '{}'", log.display());
        let command = script(dir.path(), "hook.sh", &body);
        let queue = HookQueue::new(1, Duration::from_secs(10));

        let started = Instant::now();
        queue.enqueue(request(command.clone(), "running", None));
        while !queue.lock().running {
            std::thread::sleep(Duration::from_millis(5));
        }
        queue.enqueue(request(command.clone(), "waiting", None));
        queue.enqueue(request(command, "dropped", None));
        assert!(started.elapsed() < Duration::from_millis(400));

        assert!(!queue.drain(Duration::from_millis(1)));
        assert!(queue.drain(Duration::from_secs(10)));
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "running\nwaiting\n");
    }

    #[test]
    #[cfg(unix)]
    fn failed_or_missing_commands_do_not_stop_the_queue() {
        let dir = TempDir::new().unwrap();
        let marker = dir.path().join("ran");
        let queue = HookQueue::new(8, Duration::from_secs(10));
        queue.enqueue(request("exit 3 #".into(), "", None));
        queue.enqueue(request("/definitely/not/a/command".into(), "", None));
        let command = script(
            dir.path(),
            "ok.sh",
            &format!("touch '{}'", marker.display()),
        );
        queue.enqueue(request(command, "", None));
        assert!(queue.drain(Duration::from_secs(10)));
        assert!(marker.exists());
    }

    #[test]
    #[cfg(windows)]
    fn windows_hook_gets_a_path_with_spaces_and_shell_characters() {
        let dir = TempDir::new().unwrap();
        let out = dir.path().join("out.txt");
        let script = dir.path().join("hook.cmd");
        // `%1` keeps the quotes the runner added. `%~1` would strip them and
        // let the script itself run whatever follows an `&` in the name.
        std::fs::write(
            &script,
            format!("@echo off\r\necho %1>\"{}\"\r\n", out.display()),
        )
        .unwrap();
        let command = format!("\"{}\"", script.display());
        let argument = r"C:\tmp\a b & c %PATH% d.md";
        let run = run_request(
            request(command, argument, Some("{}")),
            Duration::from_secs(30),
        )
        .unwrap();
        assert!(
            run.output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&run.output.stderr)
        );
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().trim(),
            format!("\"{argument}\"")
        );
    }

    #[test]
    #[cfg(unix)]
    fn search_path_keeps_existing_entries_first_and_avoids_duplicates() {
        let existing = std::env::join_paths(["/usr/bin", "/opt/homebrew/bin"]).unwrap();
        let path = hook_search_path(Some(existing), Some(Path::new("/home/u"))).unwrap();
        let entries: Vec<_> = std::env::split_paths(&path).collect();
        let expected: Vec<std::path::PathBuf> = [
            "/usr/bin",
            "/opt/homebrew/bin",
            "/home/u/.local/bin",
            "/home/u/.cargo/bin",
            "/usr/local/bin",
        ]
        .iter()
        .map(Into::into)
        .collect();
        assert_eq!(entries, expected);
    }
}
