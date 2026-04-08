use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallCaptureAvailability {
    Available { backend: String },
    PermissionRequired { detail: String },
    Unavailable { detail: String },
    Unsupported { detail: String },
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct CallSourceHealth {
    pub backend: String,
    pub mic_live: bool,
    pub call_audio_live: bool,
    pub mic_level: u32,
    pub call_audio_level: u32,
    pub last_update: String,
}

/// Paths to per-source audio stems written by the native call helper.
#[derive(Debug, Clone)]
pub struct StemPaths {
    pub voice: PathBuf,
    pub system: PathBuf,
}

pub struct NativeCallCaptureSession {
    child: Child,
    output_path: PathBuf,
    health: Arc<Mutex<CallSourceHealth>>,
    #[allow(dead_code)] // used once pipeline stem attribution is wired up
    stem_paths: Arc<Mutex<Option<StemPaths>>>,
}

fn parse_macos_major_version(version: &str) -> Option<u32> {
    version.trim().split('.').next()?.parse().ok()
}

#[cfg(target_os = "macos")]
fn macos_major_version() -> Option<u32> {
    let output = Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_macos_major_version(&String::from_utf8_lossy(&output.stdout))
}

impl NativeCallCaptureSession {
    pub fn output_path(&self) -> &Path {
        &self.output_path
    }

    /// Return per-source stem paths if the helper reported them and the files exist.
    #[allow(dead_code)] // used once pipeline stem attribution is wired up
    pub fn stem_paths(&self) -> Option<StemPaths> {
        self.stem_paths
            .lock()
            .ok()
            .and_then(|guard| guard.clone())
            .filter(|stems| stems.voice.exists() && stems.system.exists())
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>, String> {
        self.child.try_wait().map_err(|error| error.to_string())
    }

    pub fn source_health(&self) -> CallSourceHealth {
        self.health
            .lock()
            .map(|health| health.clone())
            .unwrap_or_else(|_| CallSourceHealth {
                backend: "screencapturekit-helper".into(),
                mic_live: false,
                call_audio_live: false,
                mic_level: 0,
                call_audio_level: 0,
                last_update: chrono::Local::now().to_rfc3339(),
            })
    }

    pub fn stop(&mut self) -> Result<(), String> {
        #[cfg(not(target_os = "macos"))]
        {
            return Err("native call capture is unsupported on this platform".into());
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                if status.success() {
                    return Ok(());
                }
                return Err(format!("native call helper exited with status {}", status));
            }

            let pid = self.child.id();
            let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if rc != 0 {
                let error = std::io::Error::last_os_error();
                let _ = self.child.kill();
                return Err(format!(
                    "failed to stop native call helper (PID {}): {}",
                    pid, error
                ));
            }

            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(15) {
                if let Some(status) = self.child.try_wait().map_err(|error| error.to_string())? {
                    if status.success() {
                        return Ok(());
                    }
                    return Err(format!("native call helper exited with status {}", status));
                }
                std::thread::sleep(Duration::from_millis(200));
            }

            let _ = self.child.kill();
            Err("native call helper did not stop within 15 seconds".into())
        }
    }
}

pub fn availability() -> CallCaptureAvailability {
    #[cfg(not(target_os = "macos"))]
    {
        return CallCaptureAvailability::Unsupported {
            detail: "Native call capture is currently implemented on macOS only.".into(),
        };
    }

    #[cfg(target_os = "macos")]
    {
        match macos_major_version() {
            Some(major) if major < 15 => {
                return CallCaptureAvailability::Unsupported {
                    detail: format!(
                        "Native call capture requires macOS 15 or newer. This Mac reports macOS {}.",
                        major
                    ),
                };
            }
            None => {
                return CallCaptureAvailability::Unavailable {
                    detail: "Could not determine the macOS version for native call capture.".into(),
                };
            }
            _ => {}
        }

        match find_native_call_helper_binary() {
            Some(_) => CallCaptureAvailability::Available {
                backend: "screencapturekit-helper".into(),
            },
            None => CallCaptureAvailability::Unavailable {
                detail: "Bundled native call helper is missing from the app bundle.".into(),
            },
        }
    }
}

#[cfg(target_os = "macos")]
pub fn start_native_call_capture() -> Result<NativeCallCaptureSession, String> {
    if let Some(major) = macos_major_version() {
        if major < 15 {
            return Err(format!(
                "native call capture requires macOS 15 or newer (found macOS {})",
                major
            ));
        }
    }

    let helper = find_native_call_helper_binary()
        .ok_or_else(|| "native call helper binary is unavailable".to_string())?;
    let output_path = native_call_output_path()?;
    let health = Arc::new(Mutex::new(CallSourceHealth {
        backend: "screencapturekit-helper".into(),
        mic_live: false,
        call_audio_live: false,
        mic_level: 0,
        call_audio_level: 0,
        last_update: chrono::Local::now().to_rfc3339(),
    }));
    let mut child = Command::new(helper)
        .arg(&output_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start native call helper: {}", error))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "native call helper did not expose stdout".to_string())?;
    let (tx, rx) = mpsc::channel();
    let stem_paths: Arc<Mutex<Option<StemPaths>>> = Arc::new(Mutex::new(None));
    let health_for_thread = Arc::clone(&health);
    let stems_for_thread = Arc::clone(&stem_paths);
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let mut ready_sent = false;

        loop {
            line.clear();
            let read = match reader.read_line(&mut line) {
                Ok(read) => read,
                Err(error) => {
                    if !ready_sent {
                        let _ = tx.send(Err(format!(
                            "failed to read native call helper output: {}",
                            error
                        )));
                    }
                    break;
                }
            };

            if read == 0 {
                if !ready_sent {
                    let _ = tx.send(Err(
                        "native call helper exited before signaling readiness".into()
                    ));
                }
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            if !ready_sent {
                ready_sent = true;
                let _ = tx.send(Ok(trimmed.to_string()));
                continue;
            }

            if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
                match value.get("event").and_then(|v| v.as_str()) {
                    Some("health") => {
                        if let Ok(mut current) = health_for_thread.lock() {
                            current.mic_live = value
                                .get("mic_live")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            current.call_audio_live = value
                                .get("call_audio_live")
                                .and_then(|v| v.as_bool())
                                .unwrap_or(false);
                            current.mic_level = value
                                .get("mic_level")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32)
                                .unwrap_or(0);
                            current.call_audio_level = value
                                .get("call_audio_level")
                                .and_then(|v| v.as_u64())
                                .map(|v| v as u32)
                                .unwrap_or(0);
                            current.last_update = chrono::Local::now().to_rfc3339();
                        }
                    }
                    Some("stems") => {
                        let voice = value
                            .get("voice_stem")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let system = value
                            .get("system_stem")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        if !voice.is_empty() && !system.is_empty() {
                            if let Ok(mut guard) = stems_for_thread.lock() {
                                *guard = Some(StemPaths {
                                    voice: PathBuf::from(voice),
                                    system: PathBuf::from(system),
                                });
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    });

    match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(line)) if line == "ready" => Ok(NativeCallCaptureSession {
            child,
            output_path,
            health,
            stem_paths,
        }),
        Ok(Ok(line)) => {
            let _ = child.kill();
            Err(format!(
                "native call helper returned unexpected readiness output: {}",
                line
            ))
        }
        Ok(Err(error)) => {
            let _ = child.kill();
            Err(error)
        }
        Err(_) => {
            let _ = child.kill();
            Err("native call helper timed out waiting for ScreenCaptureKit readiness".into())
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_native_call_capture() -> Result<NativeCallCaptureSession, String> {
    Err("native call capture is unsupported on this platform".into())
}

#[cfg(target_os = "macos")]
fn native_call_output_path() -> Result<PathBuf, String> {
    let dir = minutes_core::Config::minutes_dir().join("native-captures");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    Ok(dir.join(format!(
        "{}-call.mov",
        chrono::Local::now().format("%Y-%m-%d-%H%M%S")
    )))
}

#[cfg(target_os = "macos")]
fn find_native_call_helper_binary() -> Option<PathBuf> {
    if let Ok(exe) = std::env::current_exe() {
        let beside_exe = exe
            .parent()
            .unwrap_or(exe.as_ref())
            .join("system_audio_record");
        if beside_exe.exists() {
            return Some(beside_exe);
        }
    }

    let dev_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin/system_audio_record");
    if dev_path.exists() {
        return Some(dev_path);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::parse_macos_major_version;

    #[test]
    fn parses_major_version_from_product_version() {
        assert_eq!(parse_macos_major_version("15.0.1"), Some(15));
        assert_eq!(parse_macos_major_version("14.7"), Some(14));
        assert_eq!(parse_macos_major_version(""), None);
        assert_eq!(parse_macos_major_version("not-a-version"), None);
    }
}
