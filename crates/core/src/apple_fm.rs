//! On-device summarization via Apple Foundation Models (macOS 26+).
//!
//! Mirrors the `apple_speech` helper lifecycle: a small Swift helper is
//! embedded at compile time, written under `~/.minutes/lib/`, compiled once
//! with `swiftc`, and invoked as a subprocess with a JSON contract. The
//! prompt is passed through a 0600 temp file (never argv) so transcript
//! content cannot appear in the process list.
//!
//! Everything runs on-device: the Foundation Models framework is Apple's
//! local Apple Intelligence model. No network traffic is involved at any
//! point in this module.
//!
//! `MINUTES_APPLE_FM_HELPER` overrides helper resolution on every platform,
//! which is how the subprocess contract is unit-tested off-macOS.

use serde::Deserialize;
#[cfg(any(test, target_os = "macos"))]
use std::path::PathBuf;
use std::sync::OnceLock;
#[cfg(any(test, target_os = "macos"))]
use std::time::Duration;

#[cfg(any(test, target_os = "macos"))]
use crate::calendar::output_with_timeout;
#[cfg(any(test, target_os = "macos"))]
use std::process::Command;

#[cfg(target_os = "macos")]
const HELPER_SOURCE: &str = include_str!("../resources/apple-fm-helper.swift");
/// Capability probes are near-instant; generation gets a generous local budget.
#[cfg(any(test, target_os = "macos"))]
const CAPABILITY_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(test, target_os = "macos"))]
const GENERATION_TIMEOUT: Duration = Duration::from_secs(240);

/// Foundation Models exposes a ~4k-token context window. Chunks are capped
/// below the configured `chunk_max_tokens` so system prompt + output fit.
pub const APPLE_FM_MAX_CHUNK_TOKENS: usize = 3000;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
struct CapabilityReport {
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    schema_version: u32,
    runtime_supported: bool,
    availability: String,
    #[allow(dead_code)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(not(any(test, target_os = "macos")), allow(dead_code))]
struct GenerationResult {
    #[allow(dead_code)]
    kind: String,
    #[allow(dead_code)]
    schema_version: u32,
    text: Option<String>,
    error: Option<String>,
}

/// Resolve the helper binary: env override first (any platform, used by
/// tests and power users), then the on-demand compiled helper on macOS.
#[cfg(any(test, target_os = "macos"))]
fn resolve_helper() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("MINUTES_APPLE_FM_HELPER") {
        let path = PathBuf::from(path);
        if path.exists() {
            return Some(path);
        }
        tracing::warn!(?path, "MINUTES_APPLE_FM_HELPER set but does not exist");
        return None;
    }
    #[cfg(target_os = "macos")]
    {
        match ensure_helper_installed() {
            Ok(path) => Some(path),
            Err(error) => {
                tracing::warn!(%error, "apple-fm helper unavailable");
                None
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    None
}

/// Whether Apple Foundation Models generation is usable right now.
///
/// The probe result is cached per-process: availability doesn't change
/// mid-run, and the pipeline may consult this several times per meeting.
pub fn is_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(probe_availability)
}

fn probe_availability() -> bool {
    #[cfg(any(test, target_os = "macos"))]
    {
        let Some(helper) = resolve_helper() else {
            return false;
        };
        let mut command = Command::new(&helper);
        command.arg("capabilities");
        let Some(output) = output_with_timeout(command, CAPABILITY_TIMEOUT) else {
            tracing::warn!("apple-fm capabilities probe timed out");
            return false;
        };
        if !output.status.success() {
            return false;
        }
        match serde_json::from_slice::<CapabilityReport>(&output.stdout) {
            Ok(report) => report.runtime_supported && report.availability == "available",
            Err(error) => {
                tracing::warn!(%error, "apple-fm capabilities probe returned invalid JSON");
                false
            }
        }
    }
    #[cfg(not(any(test, target_os = "macos")))]
    false
}

/// Run one on-device generation: `system_prompt` becomes the session
/// instructions, `prompt` the user turn. Returns the model's text.
pub fn generate(system_prompt: &str, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    #[cfg(any(test, target_os = "macos"))]
    {
        let helper = resolve_helper().ok_or("apple-fm helper not available")?;

        let payload = serde_json::json!({
            "systemPrompt": system_prompt,
            "prompt": prompt,
        });
        // 0600 temp file keeps transcript text out of argv and other users' reach.
        let mut input_file = tempfile::Builder::new()
            .prefix("minutes-apple-fm-")
            .suffix(".json")
            .tempfile()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(input_file.path(), std::fs::Permissions::from_mode(0o600))?;
        }
        use std::io::Write;
        input_file.write_all(payload.to_string().as_bytes())?;
        input_file.flush()?;

        let mut command = Command::new(&helper);
        command
            .arg("generate")
            .arg("--input-file")
            .arg(input_file.path());
        let output = output_with_timeout(command, GENERATION_TIMEOUT)
            .ok_or("apple-fm generation timed out")?;

        let parsed: GenerationResult = serde_json::from_slice(&output.stdout).map_err(|e| {
            format!(
                "apple-fm helper returned invalid JSON ({}): {}",
                e,
                String::from_utf8_lossy(&output.stderr)
            )
        })?;
        if let Some(error) = parsed.error {
            return Err(error.into());
        }
        parsed
            .text
            .ok_or_else(|| "apple-fm helper returned neither text nor error".into())
    }
    #[cfg(not(any(test, target_os = "macos")))]
    {
        let _ = (system_prompt, prompt);
        Err("Apple Foundation Models summarization requires macOS".into())
    }
}

#[cfg(target_os = "macos")]
fn ensure_helper_installed() -> crate::error::Result<PathBuf> {
    use crate::config::Config;
    use crate::error::MinutesError;
    use std::fs;

    let bin_path = Config::minutes_dir().join("bin").join("apple-fm-helper");
    let source_path = Config::minutes_dir()
        .join("lib")
        .join("apple-fm-helper.swift");

    if let Some(parent) = source_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if let Some(parent) = bin_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let needs_source_write = match fs::read_to_string(&source_path) {
        Ok(existing) => existing != HELPER_SOURCE,
        Err(_) => true,
    };
    if needs_source_write {
        fs::write(&source_path, HELPER_SOURCE)?;
    }

    let needs_compile = match (fs::metadata(&source_path), fs::metadata(&bin_path)) {
        (_, Err(_)) => true,
        (Ok(source_meta), Ok(bin_meta)) => source_meta.modified().ok() > bin_meta.modified().ok(),
        _ => true,
    };
    if needs_compile {
        let output = Command::new("xcrun")
            .arg("swiftc")
            .arg("-parse-as-library")
            .arg("-O")
            .arg(&source_path)
            .arg("-o")
            .arg(&bin_path)
            .output()
            .or_else(|_| {
                Command::new("swiftc")
                    .arg("-parse-as-library")
                    .arg("-O")
                    .arg(&source_path)
                    .arg("-o")
                    .arg(&bin_path)
                    .output()
            })?;
        if !output.status.success() {
            return Err(MinutesError::Io(std::io::Error::other(format!(
                "failed to compile apple-fm helper: {}",
                String::from_utf8_lossy(&output.stderr)
            ))));
        }
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&bin_path, fs::Permissions::from_mode(0o700))?;
    }

    Ok(bin_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    /// Write an executable stub script that plays the helper's role.
    fn write_stub(dir: &std::path::Path, body: &str) -> PathBuf {
        let path = dir.join("fake-apple-fm-helper");
        let mut file = std::fs::File::create(&path).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        writeln!(file, "{}", body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    // Env-var tests share a process; serialize them.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvGuard;
    impl EnvGuard {
        fn set(path: &std::path::Path) -> Self {
            std::env::set_var("MINUTES_APPLE_FM_HELPER", path);
            EnvGuard
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var("MINUTES_APPLE_FM_HELPER");
        }
    }

    #[test]
    fn generate_returns_text_from_helper() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let stub = write_stub(
            dir.path(),
            r#"echo '{"kind":"generation","schemaVersion":1,"text":"SUMMARY: a short recap","error":null,"elapsedMs":5}'"#,
        );
        let _guard = EnvGuard::set(&stub);
        let text = generate("system", "prompt").unwrap();
        assert!(text.contains("SUMMARY"));
    }

    #[test]
    fn generate_surfaces_helper_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let stub = write_stub(
            dir.path(),
            r#"echo '{"kind":"generation","schemaVersion":1,"text":null,"error":"Apple Intelligence model unavailable on this system","elapsedMs":1}'"#,
        );
        let _guard = EnvGuard::set(&stub);
        let error = generate("system", "prompt").unwrap_err().to_string();
        assert!(error.contains("unavailable"));
    }

    #[test]
    fn generate_receives_prompt_via_input_file_not_argv() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        // Stub asserts $1/$2 shape, then echoes the input file's contents back
        // as the generated text so the test can confirm file-based transport.
        let stub = write_stub(
            dir.path(),
            r#"[ "$1" = "generate" ] || exit 2
[ "$2" = "--input-file" ] || exit 2
grep -q "the secret transcript" "$3" || exit 3
echo '{"kind":"generation","schemaVersion":1,"text":"file transport confirmed","error":null,"elapsedMs":1}'"#,
        );
        let _guard = EnvGuard::set(&stub);
        let text = generate("SYS", "the secret transcript").unwrap();
        assert_eq!(text, "file transport confirmed");
    }

    #[test]
    fn probe_availability_true_when_helper_reports_available() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let stub = write_stub(
            dir.path(),
            r#"echo '{"kind":"capabilities","schemaVersion":1,"osVersion":"26.0.0","runtimeSupported":true,"availability":"available","reason":null}'"#,
        );
        let _guard = EnvGuard::set(&stub);
        assert!(probe_availability());
    }

    #[test]
    fn probe_availability_false_when_model_unavailable() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let stub = write_stub(
            dir.path(),
            r#"echo '{"kind":"capabilities","schemaVersion":1,"osVersion":"15.5.0","runtimeSupported":false,"availability":"unavailable","reason":"needs macOS 26"}'"#,
        );
        let _guard = EnvGuard::set(&stub);
        assert!(!probe_availability());
    }

    #[test]
    fn probe_availability_false_without_helper() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Point the override at a nonexistent path: resolution must fail closed.
        std::env::set_var("MINUTES_APPLE_FM_HELPER", "/nonexistent/apple-fm-helper");
        let result = probe_availability();
        std::env::remove_var("MINUTES_APPLE_FM_HELPER");
        assert!(!result);
    }
}
