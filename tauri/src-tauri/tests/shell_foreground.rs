// Real PTY spawning runs in a fresh executable, without the desktop unit
// suite's accumulated native runtimes/background threads. The parent supplies
// a hard deadline even if a platform blocks before spawn_command returns.
#![cfg(unix)]
#[path = "../src/pty_foreground.rs"]
mod foreground;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use std::io::Write;
use std::time::{Duration, Instant};

fn wait_for(mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn exercise_foreground() {
    eprintln!("PTY fixture: opening terminal");
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-i");
    eprintln!("PTY fixture: spawning shell");
    let mut child = pair.slave.spawn_command(command).unwrap();
    eprintln!("PTY fixture: shell started");
    drop(pair.slave);
    let mut writer = pair.master.take_writer().unwrap();
    // Like production, drain output while the shell runs. On macOS a dying
    // shell can wait for unread terminal output, leaving child.wait() blocked.
    let mut reader = pair.master.try_clone_reader().unwrap();
    let drain = std::thread::spawn(move || {
        let _ = std::io::copy(&mut reader, &mut std::io::sink());
    });
    let idle_before = wait_for(|| foreground::shell_is_foreground(&*pair.master, &*child));
    writer.write_all(b"sleep 1\n").unwrap();
    writer.flush().unwrap();
    let child_owns_terminal = wait_for(|| !foreground::shell_is_foreground(&*pair.master, &*child));
    let idle_after = wait_for(|| foreground::shell_is_foreground(&*pair.master, &*child));
    eprintln!("PTY fixture: terminating shell after foreground checks");
    let _ = child.kill();
    let _ = child.wait();
    drop(writer);
    drop(pair.master);
    drain.join().unwrap();
    assert!(idle_before, "initial shell must own terminal");
    assert!(child_owns_terminal, "resume must refuse a foreground child");
    assert!(
        idle_after,
        "resume becomes available when shell regains foreground"
    );
}

#[test]
fn explicit_resume_refuses_a_foreground_child_and_allows_the_returned_shell() {
    const CHILD_MARKER: &str = "MINUTES_PTY_FOREGROUND_FIXTURE";
    if std::env::var_os(CHILD_MARKER).is_some() {
        exercise_foreground();
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "explicit_resume_refuses_a_foreground_child_and_allows_the_returned_shell",
            "--nocapture",
        ])
        .env(CHILD_MARKER, "1")
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "PTY fixture failed: {status}");
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PTY foreground fixture exceeded 20 seconds");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
