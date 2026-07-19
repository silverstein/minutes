//! Bounded parent-memory, finite-deadline supervision for audio engine children.
//!
//! Callers construct the command (including the executable allow-list and
//! environment policy); this module owns only process-tree lifetime and pipe
//! budgets. A timeout or budget failure terminates the supervised Unix process
//! group or Windows Job. This is not a sandbox or child-RSS limiter: a
//! deliberately daemonizing user-configured Unix executable can call `setsid`
//! after it has already received the user's authority and leave that group.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub(crate) const DEFAULT_STDOUT_LIMIT: u64 = 64 * 1024 * 1024;
#[cfg(feature = "parakeet")]
pub(crate) const DEFAULT_STDERR_TAIL: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChildBudget {
    pub(crate) wall_clock: Duration,
    pub(crate) stderr_tail: usize,
}

pub(crate) enum StdoutTarget {
    Capture {
        max_bytes: u64,
    },
    ExactWriter {
        writer: Box<dyn Write + Send>,
        max_bytes: u64,
    },
}

pub(crate) type StdinSource = Box<dyn Read + Send>;

/// An auditable child-launch description for the bounded supervisor.
///
/// Keeping the program, arguments, environment mutations, and working
/// directory beside the underlying [`std::process::Command`] lets the Unix
/// launch path use `execve` without losing any caller policy. The standard
/// library intentionally exposes no getter for `env_clear`, so accepting an
/// opaque `Command` here would make faithful direct execution impossible.
pub(crate) struct BoundedCommand {
    command: std::process::Command,
    #[cfg(unix)]
    program: OsString,
    arguments: Vec<OsString>,
    environment_clear: bool,
    environment: std::collections::BTreeMap<OsString, Option<OsString>>,
    current_dir: Option<PathBuf>,
}

impl BoundedCommand {
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let program = program.as_ref().to_os_string();
        Self {
            command: crate::engine_process::command(&program),
            #[cfg(unix)]
            program,
            arguments: Vec::new(),
            environment_clear: false,
            environment: std::collections::BTreeMap::new(),
            current_dir: None,
        }
    }

    pub(crate) fn arg<S: AsRef<OsStr>>(&mut self, argument: S) -> &mut Self {
        self.command.arg(argument.as_ref());
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    pub(crate) fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for argument in arguments {
            self.arg(argument);
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.arguments.iter().map(OsString::as_os_str)
    }

    #[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
    pub(crate) fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.env(key.as_ref(), value.as_ref());
        self.environment.insert(
            key.as_ref().to_os_string(),
            Some(value.as_ref().to_os_string()),
        );
        self
    }

    #[allow(dead_code)] // Kept explicit so future callers cannot bypass policy tracking.
    pub(crate) fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.command.env_remove(key.as_ref());
        self.environment.insert(key.as_ref().to_os_string(), None);
        self
    }

    #[allow(dead_code)] // Exercised by the Unix policy regressions.
    pub(crate) fn env_clear(&mut self) -> &mut Self {
        self.command.env_clear();
        self.environment_clear = true;
        self.environment.clear();
        self
    }

    #[allow(dead_code)] // Exercised by the Unix path-resolution regressions.
    pub(crate) fn current_dir<P: AsRef<Path>>(&mut self, directory: P) -> &mut Self {
        self.command.current_dir(directory.as_ref());
        self.current_dir = Some(directory.as_ref().to_path_buf());
        self
    }

    #[cfg(unix)]
    #[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
    pub(crate) unsafe fn pre_exec<F>(&mut self, function: F) -> &mut Self
    where
        F: FnMut() -> std::io::Result<()> + Send + Sync + 'static,
    {
        use std::os::unix::process::CommandExt;

        // SAFETY: the caller owns the same async-signal-safety obligation as
        // `CommandExt::pre_exec`; this wrapper only retains the explicit launch
        // policy that the bounded supervisor needs for its final execve.
        unsafe {
            self.command.pre_exec(function);
        }
        self
    }
}

#[derive(Debug)]
struct ChildSpawnFailure {
    source: std::io::Error,
    context: Option<String>,
}

impl std::fmt::Display for ChildSpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)?;
        if let Some(context) = &self.context {
            write!(formatter, "; {context}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ChildSpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn child_spawn_failure(source: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        source.kind(),
        ChildSpawnFailure {
            source,
            context: None,
        },
    )
}

pub(crate) fn is_spawn_failure(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.downcast_ref::<ChildSpawnFailure>().is_some())
}

pub(crate) fn with_context_preserving_spawn_failure(
    error: std::io::Error,
    context: impl Into<String>,
) -> std::io::Error {
    let kind = error.kind();
    if is_spawn_failure(&error) {
        std::io::Error::new(
            kind,
            ChildSpawnFailure {
                source: error,
                context: Some(context.into()),
            },
        )
    } else {
        std::io::Error::new(kind, format!("{error}; {}", context.into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StdinCompletion {
    /// Authenticated audio consumers must read through EOF so a corrupt suffix
    /// cannot be hidden behind a valid prefix.
    RequireComplete,
    /// A bounded metadata probe may intentionally stop after enough input.
    /// Only child-side BrokenPipe is accepted; source read errors still fail.
    AllowChildEarlyClose,
}

#[derive(Debug)]
pub(crate) struct ChildRun {
    pub(crate) output: Output,
    pub(crate) timed_out: bool,
}

enum StdinTransferError {
    SourceRead(std::io::Error),
    ChildSink(std::io::Error),
    Cancelled(std::io::Error),
}

impl StdinTransferError {
    fn into_io_error(self) -> std::io::Error {
        match self {
            Self::SourceRead(error) | Self::ChildSink(error) | Self::Cancelled(error) => error,
        }
    }
}

enum PipeEvent {
    Stdout(std::io::Result<Vec<u8>>),
    Stderr(std::io::Result<Vec<u8>>),
    Stdin(Result<(), StdinTransferError>),
    Limit(&'static str),
}

fn resource_error(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}

#[cfg(unix)]
fn make_pipe_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn make_pipe_nonblocking<T>(_pipe: &T) -> std::io::Result<()> {
    Ok(())
}

fn read_capture_bounded(
    mut source: ChildStdout,
    max_bytes: u64,
    cancel: &AtomicBool,
    events: &mpsc::Sender<PipeEvent>,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let retained_capacity = usize::try_from(max_bytes.min(1024 * 1024)).unwrap_or(1024 * 1024);
    let mut retained = Vec::with_capacity(retained_capacity);
    let mut total = 0_u64;
    let mut exceeded = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "child stdout was cancelled",
            ));
        }
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if !cancel.load(Ordering::Acquire) && !exceeded && total <= max_bytes {
            retained.extend_from_slice(&buffer[..read]);
        } else if !exceeded && total > max_bytes {
            exceeded = true;
            retained.clear();
            let _ = events.send(PipeEvent::Limit("child stdout resource budget exceeded"));
        }
    }
    if exceeded {
        Err(resource_error("child stdout resource budget exceeded"))
    } else {
        Ok(retained)
    }
}

fn stream_file_bounded(
    mut source: ChildStdout,
    mut destination: Box<dyn Write + Send>,
    max_bytes: u64,
    cancel: &AtomicBool,
    events: &mpsc::Sender<PipeEvent>,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let mut total = 0_u64;
    let mut exceeded = false;
    let mut buffer = Zeroizing::new([0_u8; 256 * 1024]);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "private audio stdout was cancelled",
            ));
        }
        let read = match source.read(buffer.as_mut()) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if !cancel.load(Ordering::Acquire) && !exceeded && total <= max_bytes {
            destination.write_all(&buffer[..read])?;
        } else if !exceeded && total > max_bytes {
            exceeded = true;
            let _ = events.send(PipeEvent::Limit(
                "private audio output resource budget exceeded",
            ));
        }
    }
    if exceeded {
        Err(resource_error(
            "private audio output resource budget exceeded",
        ))
    } else if cancel.load(Ordering::Acquire) {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "private audio output was cancelled",
        ))
    } else {
        destination.flush()?;
        Ok(Vec::new())
    }
}

fn read_tail(
    mut source: ChildStderr,
    max_bytes: usize,
    cancel: &AtomicBool,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(retained);
        }
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        retained.extend_from_slice(&buffer[..read]);
        if retained.len() > max_bytes {
            let overflow = retained.len() - max_bytes;
            retained.drain(..overflow);
        }
    }
    Ok(retained)
}

fn write_input_cancelable(
    mut input: StdinSource,
    mut sink: ChildStdin,
    cancel: &AtomicBool,
) -> Result<(), StdinTransferError> {
    make_pipe_nonblocking(&sink).map_err(StdinTransferError::ChildSink)?;
    let mut buffer = Zeroizing::new([0_u8; 256 * 1024]);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(StdinTransferError::Cancelled(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "child stdin was cancelled",
            )));
        }
        let read = input
            .read(buffer.as_mut())
            .map_err(StdinTransferError::SourceRead)?;
        if read == 0 {
            return sink.flush().map_err(StdinTransferError::ChildSink);
        }
        let mut written = 0;
        while written < read {
            if cancel.load(Ordering::Acquire) {
                return Err(StdinTransferError::Cancelled(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "child stdin was cancelled",
                )));
            }
            match sink.write(&buffer[written..read]) {
                Ok(0) => {
                    return Err(StdinTransferError::ChildSink(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "child stdin accepted zero bytes",
                    )));
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(StdinTransferError::ChildSink(error)),
            }
        }
    }
}

#[cfg(unix)]
fn configure_process_tree(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;

    // SAFETY: `setpgid` is async-signal-safe and touches no Rust-managed state.
    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

/// Replace Rust's Unix `execvp` launch with direct `execve` after the standard
/// child setup and process-group hook. `execvp` may hand `ENOEXEC` files to
/// `/bin/sh`, which makes an invalid decoder look like a decoder that started
/// and rejected the user's audio. Direct exec keeps every kernel launch error
/// on the parent's spawn handshake, where callers can fail closed honestly,
/// while the explicit launch description preserves environment and cwd policy.
#[cfg(unix)]
fn configure_direct_exec(command: &mut BoundedCommand) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::process::CommandExt;

    fn c_string(value: &OsStr, label: &str) -> std::io::Result<CString> {
        CString::new(value.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("child {label} contains an interior NUL byte"),
            )
        })
    }

    fn default_search_path() -> OsString {
        const FALLBACK: &str = "/bin:/usr/bin";
        let required = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
        if required <= 1 {
            return OsString::from(FALLBACK);
        }
        let mut buffer = vec![0_u8; required];
        let written = unsafe {
            libc::confstr(
                libc::_CS_PATH,
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
            )
        };
        if written == 0 || written > buffer.len() {
            return OsString::from(FALLBACK);
        }
        buffer.truncate(written);
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        OsString::from_vec(buffer)
    }

    let mut effective_environment = if command.environment_clear {
        std::collections::BTreeMap::new()
    } else {
        std::env::vars_os().collect::<std::collections::BTreeMap<_, _>>()
    };
    for (key, value) in &command.environment {
        match value {
            Some(value) => {
                effective_environment.insert(key.clone(), value.clone());
            }
            None => {
                effective_environment.remove(key);
            }
        }
    }

    let parent_dir = std::env::current_dir()?;
    let effective_dir = match &command.current_dir {
        Some(directory) if directory.is_absolute() => directory.clone(),
        Some(directory) => parent_dir.join(directory),
        None => parent_dir,
    };
    let requested_program = command.program.clone();
    let executable_candidates: Vec<OsString> = if requested_program.as_bytes().contains(&b'/') {
        let path = PathBuf::from(&requested_program);
        if path.is_absolute() {
            vec![requested_program.clone()]
        } else {
            vec![effective_dir.join(path).into_os_string()]
        }
    } else {
        let search_path = effective_environment
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_else(default_search_path);
        let mut candidates = std::env::split_paths(&search_path)
            .map(|entry| {
                let directory = if entry.as_os_str().is_empty() {
                    effective_dir.clone()
                } else if entry.is_absolute() {
                    entry
                } else {
                    effective_dir.join(entry)
                };
                directory.join(&requested_program).into_os_string()
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            candidates.push(effective_dir.join(&requested_program).into_os_string());
        }
        candidates
    };
    let executable_candidates = executable_candidates
        .iter()
        .map(|candidate| c_string(candidate, "executable path"))
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut arguments = Vec::with_capacity(command.arguments.len() + 1);
    arguments.push(c_string(&requested_program, "argv[0]")?);
    for argument in &command.arguments {
        arguments.push(c_string(argument, "argument")?);
    }
    // Store exposed addresses instead of raw pointers so the pre-exec closure
    // remains Send + Sync. The captured CString allocations keep every target
    // byte buffer stable until execv replaces the child image or returns.
    let mut argument_addresses = arguments
        .iter()
        .map(|argument| argument.as_ptr() as usize)
        .collect::<Vec<_>>();
    argument_addresses.push(0);

    let mut environment = Vec::with_capacity(effective_environment.len());
    for (key, value) in effective_environment {
        if key.as_bytes().contains(&b'=') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "child environment key contains '='",
            ));
        }
        let mut entry = OsString::with_capacity(key.len() + value.len() + 1);
        entry.push(key);
        entry.push("=");
        entry.push(value);
        environment.push(c_string(&entry, "environment entry")?);
    }
    let mut environment_addresses = environment
        .iter()
        .map(|entry| entry.as_ptr() as usize)
        .collect::<Vec<_>>();
    environment_addresses.push(0);

    // SAFETY: all allocation and CString validation happened in the parent.
    // The child closure performs only async-signal-safe execve and errno access.
    // Both address vectors are null-terminated and point into captured
    // CStrings whose allocations remain alive in the closure. Searching the
    // already-split candidates directly preserves legal ':' bytes in cwd/path
    // components and avoids `join_paths` round-tripping. Like execvp, EACCES
    // wins if at least one candidate was denied; unlike execvp, ENOEXEC is
    // returned directly and is never handed to a shell.
    unsafe {
        command.command.pre_exec(move || {
            let _keep_arguments_alive = &arguments;
            let _keep_environment_alive = &environment;
            let _keep_executables_alive = &executable_candidates;
            let mut permission_denied = false;
            for executable in &executable_candidates {
                libc::execve(
                    executable.as_ptr(),
                    argument_addresses.as_ptr().cast::<*const libc::c_char>(),
                    environment_addresses.as_ptr().cast::<*const libc::c_char>(),
                );
                let error = std::io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EACCES) => permission_denied = true,
                    Some(libc::ENOENT) | Some(libc::ENOTDIR) => {}
                    _ => return Err(error),
                }
            }
            Err(std::io::Error::from_raw_os_error(if permission_denied {
                libc::EACCES
            } else {
                libc::ENOENT
            }))
        });
    }
    Ok(())
}

#[cfg(windows)]
fn configure_process_tree(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    // A live CreateProcess→AssignToJob gap lets a fast child escape before
    // containment. Start suspended; `ProcessTree::attach` assigns the job and
    // resumes only after KILL_ON_JOB_CLOSE is authoritative.
    command.creation_flags(CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_tree(_command: &mut std::process::Command) {}

#[cfg(unix)]
fn synthetic_terminated_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    ExitStatus::from_raw(libc::SIGKILL)
}

#[cfg(windows)]
fn synthetic_terminated_status() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    ExitStatus::from_raw(1)
}

#[cfg(not(any(unix, windows)))]
fn synthetic_terminated_status() -> ExitStatus {
    panic!("bounded child supervision is unsupported on this platform")
}

/// Observe an exited Unix leader without reaping it. The unreaped zombie keeps
/// its PID—and therefore its dedicated PGID—reserved until `ProcessTree` has
/// retired every descendant. Calling `Child::try_wait` here would reap the
/// leader and open a window in which the numeric PGID could be recycled before
/// the later group kill.
#[cfg(unix)]
fn observe_child_exit(child: &mut std::process::Child) -> std::io::Result<bool> {
    loop {
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            let info = unsafe { info.assume_init() };
            return match info.si_signo {
                libc::SIGCHLD => Ok(true),
                0 => Ok(false),
                signal => Err(std::io::Error::other(format!(
                    "waitid returned unexpected signal {signal}"
                ))),
            };
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(not(unix))]
fn observe_child_exit(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

struct ProcessTree {
    #[cfg(unix)]
    process_group: Option<i32>,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

impl ProcessTree {
    fn attach(child: &mut std::process::Child) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                process_group: Some(i32::try_from(child.id()).map_err(|_| {
                    std::io::Error::other("child pid exceeded process-group range")
                })?),
            })
        }

        #[cfg(windows)]
        {
            use std::mem::{size_of, zeroed};
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
            use windows_sys::Win32::System::JobObjects::{
                AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
                SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
                JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            };

            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if job.is_null() || job == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut limits = unsafe { zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if unsafe {
                SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            } == 0
            {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(error);
            }
            if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } == 0 {
                let error = std::io::Error::last_os_error();
                unsafe { CloseHandle(job) };
                return Err(error);
            }
            let resume_status = unsafe { NtResumeProcess(child.as_raw_handle() as _) };
            if resume_status < 0 {
                unsafe { CloseHandle(job) };
                return Err(std::io::Error::other(format!(
                    "failed to resume supervised child (NTSTATUS 0x{:08x})",
                    resume_status as u32
                )));
            }
            Ok(Self { job })
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = child;
            Ok(Self {})
        }
    }

    fn terminate(&self, child: &mut std::process::Child) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group {
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        } else {
            // Killing the shared outer group here would also kill the CLI
            // supervisor. Retire the direct child; the MCP parent retires the
            // complete outer group when the CLI exits or exceeds its budget.
            let _ = child.kill();
        }

        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = child.kill();
        }

        #[cfg(windows)]
        let _ = child;
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

pub(crate) fn run(
    command: &mut BoundedCommand,
    input: Option<StdinSource>,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
) -> std::io::Result<ChildRun> {
    run_with_stdin_completion(
        command,
        input,
        stdout_target,
        budget,
        StdinCompletion::RequireComplete,
    )
}

pub(crate) fn run_allowing_child_to_close_stdin(
    command: &mut BoundedCommand,
    input: StdinSource,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
) -> std::io::Result<ChildRun> {
    run_with_stdin_completion(
        command,
        Some(input),
        stdout_target,
        budget,
        StdinCompletion::AllowChildEarlyClose,
    )
}

fn run_with_stdin_completion(
    command: &mut BoundedCommand,
    input: Option<StdinSource>,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
    stdin_completion: StdinCompletion,
) -> std::io::Result<ChildRun> {
    if budget.wall_clock.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "child wall-clock budget must be positive",
        ));
    }

    command
        .command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    configure_process_tree(&mut command.command);
    #[cfg(unix)]
    configure_direct_exec(command).map_err(child_spawn_failure)?;

    let mut child = command.command.spawn().map_err(child_spawn_failure)?;
    let tree = match ProcessTree::attach(&mut child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("child stderr pipe was unavailable"))?;

    let (events_tx, events_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));

    let stdout_events = events_tx.clone();
    let stdout_cancel = Arc::clone(&cancel);
    let stdout_thread = std::thread::spawn(move || {
        let result = match stdout_target {
            StdoutTarget::Capture { max_bytes } => {
                read_capture_bounded(stdout, max_bytes, &stdout_cancel, &stdout_events)
            }
            StdoutTarget::ExactWriter { writer, max_bytes } => {
                stream_file_bounded(stdout, writer, max_bytes, &stdout_cancel, &stdout_events)
            }
        };
        let _ = stdout_events.send(PipeEvent::Stdout(result));
    });

    let stderr_events = events_tx.clone();
    let stderr_cancel = Arc::clone(&cancel);
    let stderr_thread = std::thread::spawn(move || {
        let _ = stderr_events.send(PipeEvent::Stderr(read_tail(
            stderr,
            budget.stderr_tail,
            &stderr_cancel,
        )));
    });

    let stdin_thread = input.map(|input| {
        let stdin_events = events_tx.clone();
        let stdin_cancel = Arc::clone(&cancel);
        let stdin = child
            .stdin
            .take()
            .expect("stdin is piped when exact input is present");
        std::thread::spawn(move || {
            let result = write_input_cancelable(input, stdin, &stdin_cancel);
            let _ = stdin_events.send(PipeEvent::Stdin(result));
        })
    });
    drop(events_tx);

    let deadline = Instant::now() + budget.wall_clock;
    let mut leader_exited = false;
    let mut stdout_result: Option<std::io::Result<Vec<u8>>> = None;
    let mut stderr_result: Option<std::io::Result<Vec<u8>>> = None;
    let mut stdin_result: Option<std::io::Result<()>> = stdin_thread.is_none().then_some(Ok(()));
    let mut failure: Option<std::io::Error> = None;
    let mut terminated = false;
    let mut cleanup_deadline: Option<Instant> = None;
    let mut timed_out = false;

    while !leader_exited
        || stdout_result.is_none()
        || stderr_result.is_none()
        || stdin_result.is_none()
    {
        if !leader_exited {
            match observe_child_exit(&mut child) {
                Ok(exited) => leader_exited = exited,
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }

        let now = Instant::now();
        if now >= deadline && failure.is_none() {
            timed_out = true;
            failure = Some(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "child process tree exceeded its wall-clock budget",
            ));
        }
        if failure.is_some() && !terminated {
            cancel.store(true, Ordering::Release);
            tree.terminate(&mut child);
            terminated = true;
            cleanup_deadline = Some(Instant::now() + Duration::from_secs(5));
        }
        if cleanup_deadline.is_some_and(|cleanup| Instant::now() >= cleanup) {
            break;
        }

        let wait = cleanup_deadline
            .unwrap_or(deadline)
            .saturating_duration_since(now)
            .min(Duration::from_millis(25));
        match events_rx.recv_timeout(wait) {
            Ok(PipeEvent::Stdout(result)) => {
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stdout_result = Some(result);
            }
            Ok(PipeEvent::Stderr(result)) => {
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stderr_result = Some(result);
            }
            Ok(PipeEvent::Stdin(result)) => {
                let result = match result {
                    Err(StdinTransferError::ChildSink(error))
                        if stdin_completion == StdinCompletion::AllowChildEarlyClose
                            && error.kind() == std::io::ErrorKind::BrokenPipe =>
                    {
                        Ok(())
                    }
                    Ok(()) => Ok(()),
                    Err(error) => Err(error.into_io_error()),
                };
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stdin_result = Some(result);
            }
            Ok(PipeEvent::Limit(message)) => {
                if failure.is_none() {
                    failure = Some(resource_error(message));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All pipe workers can finish before a still-running leader.
                // Keep enforcing the deadline instead of falling into an
                // unbounded `wait()` after channel closure.
                std::thread::sleep(wait.max(Duration::from_millis(1)));
            }
        }
    }

    // A successful leader is not permission for a detached engine descendant
    // to survive Minutes. The pipes may already be closed/redirected, so make
    // tree retirement unconditional before returning the leader's status.
    if !terminated {
        cancel.store(true, Ordering::Release);
        tree.terminate(&mut child);
    }
    if !leader_exited {
        // The cleanup deadline is a hard caller-latency contract. A process
        // stuck in uninterruptible kernel I/O may not reap even after tree
        // termination, so transfer ownership to a detached reaper rather
        // than turning the bounded call into a blocking `wait()`.
        let _ = std::thread::Builder::new()
            .name("minutes-child-reaper".into())
            .spawn(move || child.wait());

        if timed_out {
            return Ok(ChildRun {
                output: Output {
                    status: synthetic_terminated_status(),
                    stdout: Vec::new(),
                    stderr: stderr_result.and_then(Result::ok).unwrap_or_default(),
                },
                timed_out: true,
            });
        }
        return Err(failure.unwrap_or_else(|| {
            std::io::Error::other("child did not terminate before its cleanup deadline")
        }));
    }
    // On Unix this is deliberately the first reaping operation. The process
    // group has already been retired above while the leader PID still anchored
    // the numeric PGID, so no later signal can target a recycled group.
    let status = child.wait()?;
    if stdout_result.is_some() {
        let _ = stdout_thread.join();
    }
    if stderr_result.is_some() {
        let _ = stderr_thread.join();
    }
    if stdin_result.is_some() {
        if let Some(stdin_thread) = stdin_thread {
            let _ = stdin_thread.join();
        }
    }

    if let Some(error) = failure {
        if timed_out {
            return Ok(ChildRun {
                output: Output {
                    status,
                    stdout: Vec::new(),
                    stderr: stderr_result.and_then(Result::ok).unwrap_or_default(),
                },
                timed_out: true,
            });
        }
        return Err(error);
    }
    let stdout = stdout_result
        .ok_or_else(|| std::io::Error::other("child stdout supervisor disconnected"))??;
    let stderr = stderr_result
        .ok_or_else(|| std::io::Error::other("child stderr supervisor disconnected"))??;
    stdin_result.ok_or_else(|| std::io::Error::other("child stdin supervisor disconnected"))??;

    Ok(ChildRun {
        output: Output {
            status,
            stdout,
            stderr,
        },
        timed_out,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn sh(script: &str) -> BoundedCommand {
        let mut command = BoundedCommand::new("sh");
        command.args(["-c", script]);
        command
    }

    fn budget(milliseconds: u64) -> ChildBudget {
        ChildBudget {
            wall_clock: Duration::from_millis(milliseconds),
            stderr_tail: 32,
        }
    }

    #[test]
    fn every_os_spawn_error_retains_spawn_stage_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = BoundedCommand::new(dir.path().join("missing-executable"));

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("a missing executable must fail during spawn");

        assert!(is_spawn_failure(&error));
        let contextualized = with_context_preserving_spawn_failure(error, "cleanup failed");
        assert!(is_spawn_failure(&contextualized));
        assert!(contextualized.to_string().contains("cleanup failed"));
    }

    #[test]
    fn invalid_binary_image_never_falls_back_to_a_shell() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let invalid_executable = dir.path().join("foreign-image.exe");
        std::fs::write(
            &invalid_executable,
            b"MZ\x90\0synthetic foreign executable image",
        )
        .unwrap();
        std::fs::set_permissions(&invalid_executable, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut command = BoundedCommand::new(&invalid_executable);

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("an invalid image must fail the direct exec handshake");

        assert!(is_spawn_failure(&error));
    }

    #[test]
    fn direct_exec_preserves_environment_add_clear_and_remove_policy() {
        let mut cleared = BoundedCommand::new("/usr/bin/env");
        cleared
            .env("MINUTES_STALE_CHILD_VALUE", "must-disappear")
            .env_clear()
            .env("MINUTES_EXACT_CHILD_VALUE", "preserved");
        let cleared = run(
            &mut cleared,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();
        assert!(cleared.output.status.success());
        assert_eq!(
            cleared.output.stdout,
            b"MINUTES_EXACT_CHILD_VALUE=preserved\n"
        );

        let mut removed = BoundedCommand::new("/usr/bin/env");
        removed
            .env("MINUTES_EXACT_CHILD_VALUE", "preserved")
            .env_remove("PATH");
        let removed = run(
            &mut removed,
            None,
            StdoutTarget::Capture {
                max_bytes: 1024 * 1024,
            },
            budget(5_000),
        )
        .unwrap();
        assert!(removed.output.status.success());
        let output = String::from_utf8(removed.output.stdout).unwrap();
        assert!(output
            .lines()
            .any(|line| { line == "MINUTES_EXACT_CHILD_VALUE=preserved" }));
        assert!(!output.lines().any(|line| line.starts_with("PATH=")));
    }

    #[test]
    fn bare_program_uses_platform_default_path_when_child_path_is_absent() {
        let mut cleared = BoundedCommand::new("sh");
        cleared.env_clear().args(["-c", "exit 0"]);
        let cleared = run(
            &mut cleared,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .expect("env_clear must retain the platform default executable search path");
        assert!(cleared.output.status.success());

        let mut removed = BoundedCommand::new("sh");
        removed.env_remove("PATH").args(["-c", "exit 0"]);
        let removed = run(
            &mut removed,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .expect("removing child PATH must retain the platform default executable search path");
        assert!(removed.output.status.success());
    }

    #[test]
    fn bare_program_uses_command_path_and_relative_entries_use_command_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let working = directory.path().join("working");
        let binaries = working.join("bin");
        std::fs::create_dir_all(&binaries).unwrap();
        let executable = binaries.join("minutes-path-fixture");
        std::fs::write(&executable, b"#!/bin/sh\nprintf exact-path").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut command = BoundedCommand::new("minutes-path-fixture");
        command.env("PATH", "bin").current_dir(&working);
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();

        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"exact-path");
    }

    #[test]
    fn relative_path_survives_separator_in_command_working_directory() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let working = directory.path().join("working:with-separator");
        let binaries = working.join("bin");
        std::fs::create_dir_all(&binaries).unwrap();
        let executable = binaries.join("minutes-colon-path-fixture");
        std::fs::write(&executable, b"#!/bin/sh\nprintf colon-path").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut command = BoundedCommand::new("minutes-colon-path-fixture");
        command.env("PATH", "bin").current_dir(&working);
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();

        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"colon-path");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_not_found_spawn_error_retains_spawn_stage_identity() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let busy_executable = dir.path().join("busy-executable");
        std::fs::write(&busy_executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&busy_executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _write_lease = std::fs::OpenOptions::new()
            .write(true)
            .open(&busy_executable)
            .unwrap();
        let mut command = BoundedCommand::new(&busy_executable);

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("a write-leased executable must fail to spawn on Linux");

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_ne!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(is_spawn_failure(&error));
    }

    #[test]
    fn timeout_kills_a_child_process_tree() {
        let started = Instant::now();
        let run = run(
            &mut sh("sleep 30"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stdout_overflow_is_a_bounded_resource_error() {
        let error = run(
            &mut sh("while :; do printf 0123456789abcdef; done"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stdout resource budget"));
    }

    #[test]
    fn stderr_retains_only_the_configured_tail() {
        let run = run(
            &mut sh("printf 'prefix-abcdefghijklmnopqrstuvwxyz' >&2; printf ok"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"ok");
        assert_eq!(run.output.stderr.len(), 32);
        assert!(run.output.stderr.ends_with(b"abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn descendant_retaining_pipes_cannot_outlive_the_deadline() {
        let started = Instant::now();
        let run = run(
            &mut sh("(sleep 30) & exit 0"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn exact_file_overflow_never_writes_past_the_cap() {
        let temp = tempfile::tempfile().unwrap();
        let inspect = temp.try_clone().unwrap();
        let error = run(
            &mut sh("while :; do printf 0123456789abcdef; done"),
            None,
            StdoutTarget::ExactWriter {
                writer: Box::new(temp),
                max_bytes: 1024,
            },
            budget(5_000),
        )
        .unwrap_err();
        assert!(error.to_string().contains("resource budget"));
        assert!(inspect.metadata().unwrap().len() <= 1024);
    }

    #[test]
    fn child_that_never_reads_stdin_cannot_outlive_the_deadline() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let started = Instant::now();
        let run = run(
            &mut sh("sleep 30"),
            Some(Box::new(input)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn authenticated_default_rejects_child_that_closes_stdin_early() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let error = run(
            &mut sh("dd bs=1 count=1 >/dev/null 2>/dev/null"),
            Some(Box::new(input)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn bounded_metadata_probe_may_stop_reading_after_sufficient_input() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let run = run_allowing_child_to_close_stdin(
            &mut sh("dd bs=1 count=1 >/dev/null 2>/dev/null"),
            Box::new(input),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        assert!(!run.timed_out);
    }

    #[test]
    fn metadata_probe_never_forgives_source_side_broken_pipe() {
        struct PrefixThenBrokenPipe(bool);

        impl std::io::Read for PrefixThenBrokenPipe {
            fn read(&mut self, destination: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "synthetic source failure",
                    ));
                }
                self.0 = true;
                destination[0] = b'x';
                Ok(1)
            }
        }

        let error = run_allowing_child_to_close_stdin(
            &mut sh("cat >/dev/null"),
            Box::new(PrefixThenBrokenPipe(false)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("source-side BrokenPipe must never look like child early-close");

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(error.to_string().contains("synthetic source failure"));
    }

    #[test]
    fn successful_exit_status_and_streams_are_preserved() {
        let run = run(
            &mut sh("printf output; printf diagnostic >&2; exit 7"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert_eq!(run.output.status.code(), Some(7));
        assert_eq!(run.output.stdout, b"output");
        assert_eq!(run.output.stderr, b"diagnostic");
        assert!(!run.timed_out);
        assert_ne!(run.output.status.signal(), Some(9));
    }

    #[test]
    fn dedicated_group_is_retired_before_leader_reap_can_recycle_its_pgid() {
        let mut command = crate::engine_process::command("sh");
        command.args(["-c", "exit 7"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_process_tree(&mut command);

        let mut child = command.spawn().unwrap();
        let child_pid = i32::try_from(child.id()).unwrap();
        let tree = ProcessTree::attach(&mut child).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observe_child_exit(&mut child).unwrap() {
            assert!(
                Instant::now() < deadline,
                "child did not exit before deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            unsafe { libc::kill(child_pid, 0) },
            0,
            "non-reaping observation must keep the leader PID reserved"
        );
        tree.terminate(&mut child);
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn successful_leader_cannot_leave_a_detached_descendant_running() {
        let run = run(
            &mut sh("sleep 30 </dev/null >/dev/null 2>&1 & printf '%s' \"$!\""),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        let pid = String::from_utf8(run.output.stdout)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        for _ in 0..100 {
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("detached engine descendant survived successful leader retirement");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn setsid_escape_cannot_hold_supervisor_pipes_past_the_deadline() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("escaped.pid");
        let mut command = BoundedCommand::new("sh");
        command
            .args([
                "-c",
                "setsid sh -c 'echo $$ > \"$1\"; while :; do sleep 30; done' escaped \"$1\" & exit 0",
                "minutes-setsid-launcher",
            ])
            .arg(&pid_path);

        let started = Instant::now();
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();

        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
        let escaped_pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        unsafe {
            libc::kill(-escaped_pid, libc::SIGKILL);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn continuously_writing_setsid_escape_cannot_hold_pipe_workers() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("escaped-writer.pid");
        let mut command = BoundedCommand::new("sh");
        command
            .args([
                "-c",
                "setsid sh -c 'echo $$ > \"$1\"; while :; do printf x; done' escaped \"$1\" & exit 0",
                "minutes-setsid-writer-launcher",
            ])
            .arg(&pid_path);

        let started = Instant::now();
        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap_err();

        assert!(error.to_string().contains("stdout resource budget"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let escaped_pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        unsafe {
            libc::kill(-escaped_pid, libc::SIGKILL);
        }
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn powershell(script: &str) -> BoundedCommand {
        let mut command = BoundedCommand::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        command
    }

    fn budget(milliseconds: u64) -> ChildBudget {
        ChildBudget {
            wall_clock: Duration::from_millis(milliseconds),
            stderr_tail: 32,
        }
    }

    fn quote_powershell_literal(value: &std::path::Path) -> String {
        value.display().to_string().replace('\'', "''")
    }

    fn child_and_grandchild_script(pid_path: &std::path::Path, leader_tail: &str) -> String {
        format!(
            "$grandchild = Start-Process -FilePath \"$env:SystemRoot\\System32\\ping.exe\" \
             -ArgumentList @('-n','30','127.0.0.1') -WindowStyle Hidden -PassThru; \
             [IO.File]::WriteAllText('{}', [string]$grandchild.Id); {leader_tail}",
            quote_powershell_literal(pid_path)
        )
    }

    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .expect("supervised leader must publish its descendant pid")
            .trim()
            .parse()
            .expect("descendant pid must be numeric")
    }

    fn process_is_active(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code = 0;
            let active =
                GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
            CloseHandle(handle);
            active
        }
    }

    fn assert_process_tree_retired(pid: u32) {
        for _ in 0..200 {
            if !process_is_active(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("supervised Windows descendant {pid} survived Job Object retirement");
    }

    #[test]
    fn timeout_kills_windows_child_and_grandchild_tree() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("timeout-grandchild.pid");
        let script = child_and_grandchild_script(&pid_path, "Start-Sleep -Seconds 30");
        let started = Instant::now();

        let run = run(
            &mut powershell(&script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(3_000),
        )
        .unwrap();

        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(8));
        assert_process_tree_retired(read_pid(&pid_path));
    }

    #[test]
    fn successful_windows_leader_exit_retires_grandchild_and_preserves_status() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("successful-grandchild.pid");
        let script =
            child_and_grandchild_script(&pid_path, "[Console]::Out.Write('leader-output'); exit 7");

        let run = run(
            &mut powershell(&script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();

        assert!(!run.timed_out);
        assert_eq!(run.output.status.code(), Some(7));
        assert_eq!(run.output.stdout, b"leader-output");
        assert_process_tree_retired(read_pid(&pid_path));
    }

    #[test]
    fn windows_stdout_overflow_retires_child_and_grandchild_tree() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("overflow-grandchild.pid");
        let script = child_and_grandchild_script(
            &pid_path,
            "while ($true) { [Console]::Out.Write('0123456789abcdef') }",
        );

        let error = run(
            &mut powershell(&script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();

        assert!(error.to_string().contains("stdout resource budget"));
        assert_process_tree_retired(read_pid(&pid_path));
    }
}
