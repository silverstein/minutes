//! Background ownership for short-lived desktop helpers.

use std::process::{Child, Command};

/// Launch an OS helper without blocking the caller, and reap it after exit.
///
/// One shared worker polls all helpers, so a slow `open` / `osascript` never
/// blocks retirement of another. Initialize the worker before spawning anything:
/// a thread-creation failure must not leave an unowned child behind.
pub fn spawn_reaped(command: &mut Command) -> std::io::Result<u32> {
    use std::sync::{mpsc, OnceLock};
    static REAPER: OnceLock<Result<mpsc::Sender<Child>, String>> = OnceLock::new();
    let sender = REAPER
        .get_or_init(|| {
            let (sender, receiver) = mpsc::channel::<Child>();
            std::thread::Builder::new()
                .name("minutes-helper-reaper".into())
                .spawn(move || {
                    let mut children = Vec::<Child>::new();
                    loop {
                        let received = if children.is_empty() {
                            receiver
                                .recv()
                                .map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                        } else {
                            receiver.recv_timeout(std::time::Duration::from_millis(50))
                        };
                        match received {
                            Ok(child) => children.push(child),
                            Err(mpsc::RecvTimeoutError::Disconnected) if children.is_empty() => {
                                break
                            }
                            Err(_) => {}
                        }
                        children.retain_mut(|child| match child.try_wait() {
                            Ok(Some(_)) => false,
                            Ok(None) => true,
                            Err(error) => {
                                tracing::warn!(%error, "OS helper wait failed; retiring child");
                                let _ = child.kill();
                                // This fallback runs only on the reaper, never the UI.
                                let _ = child.wait();
                                false
                            }
                        });
                    }
                })
                .map(|_| sender)
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|error| std::io::Error::other(error.clone()))?;
    let child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let pid = child.id();
    if let Err(error) = sender.send(child) {
        let mut child = error.0;
        let _ = child.kill();
        let _ = child.wait();
        return Err(std::io::Error::other("OS helper reaper is unavailable"));
    }
    Ok(pid)
}
