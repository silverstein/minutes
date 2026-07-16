//! The only boundary for constructing child processes in `minutes-core`.
//!
//! Keeping process construction here makes Clippy's `disallowed-methods`
//! policy mechanically enforceable. Callers may configure the returned
//! [`Command`], while timeout-sensitive engines should use
//! [`output_with_timeout`] so pipes are drained and timed-out children are
//! killed and reaped.

use std::ffi::OsStr;
#[cfg(any(test, feature = "parakeet"))]
use std::io::{self, Read};
use std::process::Command;
#[cfg(any(test, feature = "parakeet"))]
use std::process::{Output, Stdio};
#[cfg(any(test, feature = "parakeet"))]
use std::time::{Duration, Instant};

/// Construct a child process through the repository's audited boundary.
#[allow(clippy::disallowed_methods)]
pub(crate) fn command<S: AsRef<OsStr>>(program: S) -> Command {
    Command::new(program)
}

/// Run a command to completion, killing and reaping it after `timeout`.
///
/// Stdout and stderr are drained concurrently so output larger than the OS
/// pipe buffer cannot deadlock the child. The boolean is true only when the
/// timeout path killed the process. Its returned status is obtained from
/// `Child::wait`, which is also the guarantee that the child was reaped.
#[cfg(any(test, feature = "parakeet"))]
pub(crate) fn output_with_timeout(
    command: Command,
    timeout: Duration,
) -> io::Result<(Output, bool)> {
    let (output, timed_out, _) = output_with_timeout_impl(command, timeout)?;
    Ok((output, timed_out))
}

#[cfg(any(test, feature = "parakeet"))]
fn output_with_timeout_impl(
    mut command: Command,
    timeout: Duration,
) -> io::Result<(Output, bool, u32)> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let child_id = child.id();

    let mut stdout_pipe = child.stdout.take();
    let stdout_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stdout_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });
    let mut stderr_pipe = child.stderr.take();
    let stderr_handle = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(ref mut pipe) = stderr_pipe {
            let _ = pipe.read_to_end(&mut buf);
        }
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                // `wait` is mandatory after `kill`: returning its status proves
                // the child was reaped instead of leaving a zombie behind.
                let status = child.wait()?;

                // Descendants can inherit these descriptors. Do not let an
                // unrelated descendant defeat the timeout after the direct
                // child has been killed and reaped.
                drop(stdout_handle);
                drop(stderr_handle);
                return Ok((
                    Output {
                        status,
                        stdout: Vec::new(),
                        stderr: Vec::new(),
                    },
                    true,
                    child_id,
                ));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok((
        Output {
            status,
            stdout,
            stderr,
        },
        false,
        child_id,
    ))
}

#[cfg(test)]
const CHILD_MODE_ENV: &str = "MINUTES_ENGINE_PROCESS_TEST_CHILD";

#[cfg(test)]
fn fixture_command(mode: &str) -> Command {
    let mut child = command(std::env::current_exe().expect("current test executable"));
    child
        .args(["subprocess_fixture", "--nocapture"])
        .env(CHILD_MODE_ENV, mode);
    child
}

#[cfg(test)]
pub(crate) fn aborted_fixture_command() -> Command {
    fixture_command("abort")
}

#[cfg(test)]
pub(crate) fn blocked_fixture_command() -> Command {
    fixture_command("block")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn subprocess_fixture() {
        match std::env::var(CHILD_MODE_ENV).as_deref() {
            Ok("block") => loop {
                std::thread::park();
            },
            Ok("abort") => std::process::abort(),
            Ok("oversized") => {
                let bytes = vec![b'x'; 2 * 1024 * 1024];
                std::io::stdout().write_all(&bytes).unwrap();
                std::io::stderr().write_all(&bytes).unwrap();
            }
            _ => {}
        }
    }

    #[test]
    fn timeout_kills_and_reaps_child() {
        let started = Instant::now();
        let (output, timed_out, child_id) =
            output_with_timeout_impl(fixture_command("block"), Duration::from_millis(150)).unwrap();

        assert!(timed_out);
        assert!(!output.status.success());
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "timeout wrapper exceeded its hard test deadline"
        );
        #[cfg(unix)]
        {
            unsafe extern "C" {
                fn waitpid(pid: i32, status: *mut i32, options: i32) -> i32;
            }
            const WNOHANG: i32 = 1;
            let mut status = 0;
            // SAFETY: `child_id` came from the direct child just returned by
            // the wrapper. `waitpid(WNOHANG)` does not dereference pointers
            // other than this valid local status slot.
            let wait_result = unsafe { waitpid(child_id as i32, &mut status, WNOHANG) };
            assert_eq!(wait_result, -1, "timed-out child was still waitable");
        }
    }

    #[test]
    fn oversized_stdout_and_stderr_are_drained_without_deadlock() {
        let (output, timed_out) =
            output_with_timeout(fixture_command("oversized"), Duration::from_secs(3)).unwrap();

        assert!(!timed_out);
        assert!(output.status.success());
        assert!(output.stdout.len() >= 2 * 1024 * 1024);
        assert!(output.stderr.len() >= 2 * 1024 * 1024);
    }
}
