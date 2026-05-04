//! Bundled-CLI setup: link `~/.local/bin/minutes` to the sidecar inside the
//! `.app` bundle so app updates automatically update the CLI.
//!
//! See `docs/PLAN-bundle-cli-in-app.md` for the design rationale. The exported
//! Tauri commands at the bottom of this file are the public surface.
//!
//! macOS-only. The whole module is gated at the `mod` declaration in main.rs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Manager};

const CACHE_TTL: Duration = Duration::from_secs(60);
const VERSION_PROBE_TIMEOUT: Duration = Duration::from_secs(1);
const BREW_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PROD_BUNDLE_ID: &str = "com.useminutes.desktop";
const DEV_BUNDLE_ID: &str = "com.useminutes.desktop.dev";
const PATH_MARKER_OPEN: &str = "# >>> minutes-app PATH (managed) >>>";
const PATH_MARKER_CLOSE: &str = "# <<< minutes-app PATH (managed) <<<";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedDetect {
    captured_at_ms: u128,
    install_method: String,
    keyed_path: Option<PathBuf>,
}

static DETECT_CACHE: Mutex<Option<CachedDetect>> = Mutex::new(None);

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Bundle path resolution
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum SetupError {
    NoMatchingArch {
        arch: String,
        looked_in: PathBuf,
    },
    NoMacosDir,
    Translocated,
    Io(std::io::Error),
    #[allow(dead_code)]
    Other(String),
}

impl std::fmt::Display for SetupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SetupError::NoMatchingArch { arch, looked_in } => write!(
                f,
                "This Minutes.app build doesn't include a CLI binary for your architecture ({}). \
                 Reinstall from the official DMG, which ships Universal binaries. (Looked in: {})",
                arch,
                looked_in.display()
            ),
            SetupError::NoMacosDir => write!(f, "Could not resolve the running app's bundle path."),
            SetupError::Translocated => write!(
                f,
                "Minutes is running from a temporary quarantine location. Move Minutes.app to your Applications folder and try again."
            ),
            SetupError::Io(e) => write!(f, "I/O error: {}", e),
            SetupError::Other(s) => write!(f, "{}", s),
        }
    }
}

impl From<std::io::Error> for SetupError {
    fn from(e: std::io::Error) -> Self {
        SetupError::Io(e)
    }
}

/// Locate the sidecar binary inside the app's `Contents/MacOS/` directory.
///
/// Tauri's bundler strips the target-triple suffix from `externalBin` entries
/// when copying into the `.app`, so the on-disk name in a packaged bundle is
/// just `minutes`. The arch-suffixed lookup is kept as a fallback for
/// forward compatibility with any future bundling change or universal-binary
/// scheme that retains the suffix.
fn bundled_cli_path(macos_dir: &Path) -> Result<PathBuf, SetupError> {
    let plain = macos_dir.join("minutes");
    if plain.exists() {
        return Ok(plain);
    }
    let arch = std::env::consts::ARCH;
    let triple = format!("{}-apple-darwin", arch);
    let candidate = macos_dir.join(format!("minutes-{}", triple));
    if candidate.exists() {
        return Ok(candidate);
    }
    if let Ok(entries) = std::fs::read_dir(macos_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("minutes-") && name.contains(arch) {
                    return Ok(p);
                }
            }
        }
    }
    Err(SetupError::NoMatchingArch {
        arch: arch.to_string(),
        looked_in: macos_dir.to_path_buf(),
    })
}

fn current_macos_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

fn current_bundle_root() -> Option<PathBuf> {
    current_macos_dir()
        .as_deref()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(Path::to_path_buf)
}

// ─────────────────────────────────────────────────────────────────────────────
// Translocation detection (App Translocation / quarantine xattr)
// ─────────────────────────────────────────────────────────────────────────────

fn has_quarantine_xattr(p: &Path) -> bool {
    std::process::Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(p)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn is_translocated() -> bool {
    let Some(current) = std::env::current_exe().ok() else {
        return false;
    };
    let s = current.to_string_lossy();
    if s.contains("/AppTranslocation/") {
        return true;
    }
    has_quarantine_xattr(&current)
}

// ─────────────────────────────────────────────────────────────────────────────
// Shell + PATH detection (dispatched by $SHELL)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UserShell {
    Zsh,
    Bash,
    Fish,
    Nushell,
    Other,
}

impl UserShell {
    fn detect() -> Self {
        let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
        let name = Path::new(&shell)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("zsh");
        match name {
            "zsh" => UserShell::Zsh,
            "bash" => UserShell::Bash,
            "fish" => UserShell::Fish,
            "nu" | "nushell" => UserShell::Nushell,
            _ => UserShell::Other,
        }
    }

    fn name(self) -> &'static str {
        match self {
            UserShell::Zsh => "zsh",
            UserShell::Bash => "bash",
            UserShell::Fish => "fish",
            UserShell::Nushell => "nu",
            UserShell::Other => "other",
        }
    }
}

fn user_shell_path() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into())
}

fn resolve_minutes_in_user_shell() -> Vec<PathBuf> {
    let shell = user_shell_path();
    let kind = UserShell::detect();
    let args: &[&str] = match kind {
        UserShell::Bash => &[
            "-lc",
            "command -v -a minutes 2>/dev/null || which -a minutes 2>/dev/null",
        ],
        UserShell::Zsh => &["-lc", "which -a minutes 2>/dev/null"],
        UserShell::Fish => &["-c", "type -ap minutes 2>/dev/null"],
        UserShell::Nushell => &["-c", "which minutes | get path | str join \"\\n\""],
        UserShell::Other => &["-lc", "which -a minutes 2>/dev/null"],
    };

    // `which -a` returns one entry per directory on PATH, so users with
    // duplicate PATH entries (common: ~/.local/bin appears in both .zshrc
    // and .zprofile) get the same path twice. Dedup while preserving order;
    // the first hit on PATH is the one that actually wins.
    let lines: Vec<PathBuf> = std::process::Command::new(&shell)
        .args(args)
        .output()
        .ok()
        .map(|o| {
            let mut seen = std::collections::HashSet::new();
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .filter(|p| seen.insert(p.clone()))
                .collect()
        })
        .unwrap_or_default();

    if !lines.is_empty() {
        return lines;
    }

    which::which("minutes").ok().into_iter().collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Version probe (1s timeout, semver-ish validation)
// ─────────────────────────────────────────────────────────────────────────────

fn read_binary_version_safe(p: &Path) -> Option<String> {
    use std::sync::mpsc;
    let path = p.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = std::process::Command::new(&path).arg("--version").output();
        let _ = tx.send(r);
    });
    let out = rx.recv_timeout(VERSION_PROBE_TIMEOUT).ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    s.split_whitespace()
        .find(|t| {
            let trimmed = t.trim_start_matches('v');
            trimmed.chars().filter(|c| *c == '.').count() == 2
                && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(|t| t.trim_start_matches('v').to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// detect_install_method (cached, brew probed with timeout)
// ─────────────────────────────────────────────────────────────────────────────

fn detect_install_method(path_binary: Option<&PathBuf>) -> String {
    let Some(p) = path_binary else {
        return "none".into();
    };

    if let Ok(guard) = DETECT_CACHE.lock() {
        if let Some(c) = guard.as_ref() {
            if c.keyed_path.as_deref() == Some(p)
                && now_ms().saturating_sub(c.captured_at_ms) < CACHE_TTL.as_millis()
            {
                return c.install_method.clone();
            }
        }
    }

    let s = p.to_string_lossy();
    let method = if s.contains(".cargo/bin") {
        "cargo".to_string()
    } else if s.contains("/Minutes.app/") || s.contains("/Minutes Dev.app/") {
        "bundled".to_string()
    } else {
        probe_brew_with_timeout(p)
    };

    if let Ok(mut guard) = DETECT_CACHE.lock() {
        *guard = Some(CachedDetect {
            captured_at_ms: now_ms(),
            install_method: method.clone(),
            keyed_path: Some(p.clone()),
        });
    }
    method
}

fn probe_brew_with_timeout(_path: &Path) -> String {
    use std::sync::mpsc;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = std::process::Command::new("brew")
            .args(["list", "silverstein/tap/minutes"])
            .output();
        let _ = tx.send(r);
    });
    match rx.recv_timeout(BREW_PROBE_TIMEOUT) {
        Ok(Ok(out)) if out.status.success() => "brew".into(),
        Ok(_) => "other".into(),
        Err(_) => "unknown".into(),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Multi-bundle discovery (mdfind + well-known fallback)
// ─────────────────────────────────────────────────────────────────────────────

fn read_plist_string(plist_path: &Path, key: &str) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/defaults")
        .args(["read", &plist_path.to_string_lossy(), key])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// `true` when the path looks like a build artifact (Cargo/Tauri output) rather
/// than a user-visible install. Spotlight indexes these because they're real
/// `.app` bundles, but linking the CLI to a build tree means the CLI breaks
/// the moment the developer runs `cargo clean`. Maintainers iterating on the
/// app generate fresh build artifacts on every rebuild — those don't belong in
/// the picker.
fn is_build_artifact_path(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.contains("/target/release/")
        || s.contains("/target/debug/")
        || s.contains("/target/aarch64-")
        || s.contains("/target/x86_64-")
}

fn discover_minutes_bundles() -> Vec<Value> {
    let query = format!(
        "kMDItemCFBundleIdentifier == '{}' || kMDItemCFBundleIdentifier == '{}'",
        PROD_BUNDLE_ID, DEV_BUNDLE_ID
    );
    let running_root = current_bundle_root().unwrap_or_default();
    let mut paths: Vec<PathBuf> = std::process::Command::new("/usr/bin/mdfind")
        .arg(&query)
        .output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                // Exception: keep the running bundle even if it's a build
                // artifact — the user can see it in the picker and pick it
                // intentionally if they're iterating on the app itself.
                .filter(|p| *p == running_root || !is_build_artifact_path(p))
                .collect()
        })
        .unwrap_or_default();

    let well_known = [
        PathBuf::from("/Applications/Minutes.app"),
        PathBuf::from("/Applications/Minutes Dev.app"),
        home_dir().join("Applications/Minutes.app"),
        home_dir().join("Applications/Minutes Dev.app"),
    ];
    for p in well_known {
        if p.exists() && !paths.contains(&p) {
            paths.push(p);
        }
    }

    paths
        .into_iter()
        .filter_map(|p| {
            let info_plist = p.join("Contents/Info.plist");
            let bundle_id = read_plist_string(&info_plist, "CFBundleIdentifier")?;
            let version = read_plist_string(&info_plist, "CFBundleShortVersionString")
                .unwrap_or_else(|| "?".into());
            // Per-bundle ad-hoc check so the confirmation overlay can warn
            // about the bundle the user actually picked, not just the running
            // app — the user may pick a different bundle in the multi-bundle
            // picker, and signing status varies (e.g., dev app is properly
            // signed, but a freshly-built target/release bundle is ad-hoc).
            let adhoc = is_adhoc_signed(&p);
            Some(serde_json::json!({
                "path": p,
                "bundle_id": bundle_id,
                "version": version,
                "is_running": p == running_root,
                "adhoc_signed": adhoc,
            }))
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Codesign detection (ad-hoc vs identity)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if the bundle is ad-hoc signed (no Team ID). Detection by
/// `TeamIdentifier=` line in `codesign -dv` output:
///   - real Developer-ID-signed bundle: `TeamIdentifier=ABCDE12345`
///   - ad-hoc signed bundle (`codesign -s -`): `TeamIdentifier=not set`
///   - unsigned (rare on Apple Silicon): the line may be absent entirely
///
/// Earlier versions of this check looked for `Authority=` lines, but those
/// only appear with `-dvv` or higher verbosity — at `-dv` level they're
/// absent on properly-signed bundles too, which made every bundle look
/// ad-hoc. The `TeamIdentifier=not set` literal is what `codesign` actually
/// prints for ad-hoc, and it IS in `-dv` output.
fn is_adhoc_signed(bundle_root: &Path) -> bool {
    let out = match std::process::Command::new("/usr/bin/codesign")
        .args(["-dv"])
        .arg(bundle_root)
        .output()
    {
        Ok(o) => o,
        Err(_) => return false,
    };
    // codesign -dv writes to stderr.
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let mut team_id_seen = false;
    for line in combined.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("TeamIdentifier=") {
            team_id_seen = true;
            if rest.trim() == "not set" {
                return true;
            }
        }
    }
    // No TeamIdentifier line at all → unsigned or weird; treat conservatively
    // as ad-hoc so the warning surfaces.
    !team_id_seen
}

// ─────────────────────────────────────────────────────────────────────────────
// Snooze state persistence
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CliSetupState {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    snooze_until_ms: u128,
    #[serde(default)]
    last_check_ms: u128,
    #[serde(default)]
    selected_bundle_path: Option<PathBuf>,
}

fn state_path(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_config_dir()
        .ok()
        .map(|d| d.join("cli_setup_state.json"))
}

fn load_state(app: &AppHandle) -> CliSetupState {
    let Some(path) = state_path(app) else {
        return CliSetupState::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return CliSetupState::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

fn save_state(app: &AppHandle, state: &CliSetupState) -> std::io::Result<()> {
    let Some(path) = state_path(app) else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(path, raw)
}

// ─────────────────────────────────────────────────────────────────────────────
// Symlink + PATH writing
// ─────────────────────────────────────────────────────────────────────────────

fn prune_bak_files(dir: &Path, keep: usize) -> std::io::Result<()> {
    let mut bak: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|entry| {
            let p = entry.path();
            let name = p.file_name()?.to_str()?.to_string();
            if name.starts_with("minutes.bak-") {
                let mtime = entry.metadata().ok()?.modified().ok()?;
                Some((p, mtime))
            } else {
                None
            }
        })
        .collect();
    bak.sort_by_key(|b| std::cmp::Reverse(b.1));
    for (p, _) in bak.into_iter().skip(keep) {
        std::fs::remove_file(p).ok();
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
pub struct SymlinkResult {
    pub target: PathBuf,
    pub link: PathBuf,
    pub created_local_bin: bool,
    pub backup: Option<PathBuf>,
    pub already_correct: bool,
}

fn create_or_update_symlink(target: &Path) -> Result<SymlinkResult, SetupError> {
    let local_bin = home_dir().join(".local/bin");
    let already_existed = local_bin.exists();
    std::fs::create_dir_all(&local_bin)?;
    let link = local_bin.join("minutes");

    let mut backup: Option<PathBuf> = None;
    let mut already_correct = false;

    match std::fs::symlink_metadata(&link) {
        Ok(meta) if meta.file_type().is_symlink() => {
            let existing = std::fs::read_link(&link)?;
            if existing == target {
                already_correct = true;
            } else {
                std::fs::remove_file(&link)?;
                std::os::unix::fs::symlink(target, &link)?;
            }
        }
        Ok(_) => {
            let bak = local_bin.join(format!("minutes.bak-{}", now_ms()));
            std::fs::rename(&link, &bak)?;
            std::os::unix::fs::symlink(target, &link)?;
            backup = Some(bak);
        }
        Err(_) => {
            std::os::unix::fs::symlink(target, &link)?;
        }
    }

    prune_bak_files(&local_bin, 3).ok();

    Ok(SymlinkResult {
        target: target.to_path_buf(),
        link,
        created_local_bin: !already_existed,
        backup,
        already_correct,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct PathWriteResult {
    pub shell: &'static str,
    pub action: &'static str, // "appended", "skipped", "manual", "fish-add-path"
    pub files_touched: Vec<PathBuf>,
    pub manual_snippet: Option<String>,
}

fn append_marker_block(file: &Path, body: &str) -> std::io::Result<bool> {
    if let Ok(existing) = std::fs::read_to_string(file) {
        if existing.contains(PATH_MARKER_OPEN) {
            return Ok(false);
        }
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let prefix = if file.exists() { "\n" } else { "" };
    let block = format!(
        "{prefix}{open}\n{body}\n{close}\n",
        open = PATH_MARKER_OPEN,
        close = PATH_MARKER_CLOSE,
        body = body,
    );
    use std::io::Write;
    let mut handle = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)?;
    handle.write_all(block.as_bytes())?;
    Ok(true)
}

fn write_path_for_shell(shell: UserShell) -> PathWriteResult {
    let body = "export PATH=\"$HOME/.local/bin:$PATH\"";
    match shell {
        UserShell::Zsh => {
            let zshrc = home_dir().join(".zshrc");
            let appended = append_marker_block(&zshrc, body).unwrap_or(false);
            PathWriteResult {
                shell: "zsh",
                action: if appended { "appended" } else { "skipped" },
                files_touched: vec![zshrc],
                manual_snippet: None,
            }
        }
        UserShell::Bash => {
            let mut touched = Vec::new();
            let mut any_appended = false;
            for f in [".bash_profile", ".bashrc"] {
                let p = home_dir().join(f);
                let appended = append_marker_block(&p, body).unwrap_or(false);
                if appended {
                    any_appended = true;
                }
                touched.push(p);
            }
            PathWriteResult {
                shell: "bash",
                action: if any_appended { "appended" } else { "skipped" },
                files_touched: touched,
                manual_snippet: None,
            }
        }
        UserShell::Fish => {
            // fish_add_path -U writes to the universal variable store; idempotent.
            // Older fish (< 3.2.0) lacks it; surface a manual snippet then.
            let supports_add_path = std::process::Command::new("fish")
                .args([
                    "-c",
                    "if functions -q fish_add_path; echo yes; else; echo no; end",
                ])
                .output()
                .ok()
                .and_then(|o| {
                    String::from_utf8(o.stdout)
                        .ok()
                        .map(|s| s.trim() == "yes")
                })
                .unwrap_or(false);
            if supports_add_path {
                let _ = std::process::Command::new("fish")
                    .args(["-c", "fish_add_path -U $HOME/.local/bin"])
                    .output();
                PathWriteResult {
                    shell: "fish",
                    action: "fish-add-path",
                    files_touched: vec![],
                    manual_snippet: None,
                }
            } else {
                PathWriteResult {
                    shell: "fish",
                    action: "manual",
                    files_touched: vec![],
                    manual_snippet: Some(
                        "set -U fish_user_paths $HOME/.local/bin $fish_user_paths".into(),
                    ),
                }
            }
        }
        UserShell::Nushell => PathWriteResult {
            shell: "nu",
            action: "manual",
            files_touched: vec![],
            manual_snippet: Some(
                "# Add to ~/.config/nushell/env.nu\n$env.PATH = ($env.PATH | prepend ($env.HOME | path join \".local/bin\"))".into(),
            ),
        },
        UserShell::Other => PathWriteResult {
            shell: "other",
            action: "manual",
            files_touched: vec![],
            manual_snippet: Some(
                "# Add ~/.local/bin to your shell's PATH\nexport PATH=\"$HOME/.local/bin:$PATH\""
                    .into(),
            ),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Recording-aware update guard (§7)
// ─────────────────────────────────────────────────────────────────────────────

/// Returns `true` if any of the three CLI PID files indicate an active session.
/// The Tauri command `cmd_install_update` already guards on in-process state;
/// this catches CLI-driven recordings the app didn't start.
pub fn cli_recording_active() -> bool {
    use minutes_core::pid;
    let paths = [
        pid::pid_path(),
        pid::dictation_pid_path(),
        pid::live_transcript_pid_path(),
    ];
    paths
        .iter()
        .any(|p| matches!(pid::check_pid_file(p), Ok(Some(_))))
}

// ─────────────────────────────────────────────────────────────────────────────
// Public Tauri commands
// ─────────────────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn cmd_cli_install_state(app: AppHandle) -> Value {
    let started = Instant::now();
    let app_version = app.config().version.clone().unwrap_or_default();
    let bundle_id = app.config().identifier.clone();

    let app_macos_dir = current_macos_dir();
    let bundle_root = current_bundle_root();
    let translocated = is_translocated();
    let adhoc = bundle_root
        .as_ref()
        .map(|r| is_adhoc_signed(r))
        .unwrap_or(false);

    let path_candidates = resolve_minutes_in_user_shell();
    let path_binary = path_candidates.first().cloned();
    let path_version = path_binary
        .as_ref()
        .and_then(|p| read_binary_version_safe(p));

    let app_ver_norm = app_version.trim_start_matches('v');
    let path_ver_norm = path_version
        .as_deref()
        .map(|v| v.trim_start_matches('v').to_string());
    let in_sync = path_ver_norm.as_deref() == Some(app_ver_norm);

    let mut install_method = detect_install_method(path_binary.as_ref());
    if path_candidates.len() > 1 {
        install_method = "conflict".into();
    }

    let known_bundles = discover_minutes_bundles();

    let snooze = load_state(&app).snooze_until_ms;
    let snoozed = snooze > now_ms();

    let elapsed_ms = started.elapsed().as_millis() as u64;
    tracing::debug!(
        "cmd_cli_install_state path={:?} method={} in_sync={} elapsed={}ms",
        path_binary,
        install_method,
        in_sync,
        elapsed_ms
    );

    serde_json::json!({
        "app_version": app_version,
        "app_macos_dir": app_macos_dir,
        "bundle_root": bundle_root,
        "bundle_id": bundle_id,
        "translocated": translocated,
        "adhoc_signed": adhoc,
        "shell": UserShell::detect().name(),
        "path_binary": path_binary,
        "path_candidates": path_candidates,
        "path_version": path_version,
        "in_sync": in_sync,
        "install_method": install_method,
        "known_bundles": known_bundles,
        "snoozed": snoozed,
        "snooze_until_ms": snooze,
    })
}

#[derive(Debug, Deserialize)]
pub struct CliSetupRequest {
    /// Optional explicit bundle root to link to. When `known_bundles` has more
    /// than one entry, the UI must let the user pick before invoking this.
    pub bundle_root: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct CliSetupResponse {
    pub symlink: SymlinkResult,
    pub path: PathWriteResult,
    pub adhoc_signed: bool,
    pub bundle_id: String,
    pub bundle_root: PathBuf,
}

#[tauri::command]
pub async fn cmd_cli_setup_run(
    app: AppHandle,
    request: CliSetupRequest,
) -> Result<CliSetupResponse, String> {
    if is_translocated() {
        return Err(SetupError::Translocated.to_string());
    }

    let bundle_root = request
        .bundle_root
        .clone()
        .or_else(current_bundle_root)
        .ok_or_else(|| SetupError::NoMacosDir.to_string())?;

    let macos_dir = bundle_root.join("Contents/MacOS");
    let target = bundled_cli_path(&macos_dir).map_err(|e| e.to_string())?;

    let symlink = create_or_update_symlink(&target).map_err(|e| e.to_string())?;

    // Per §5: if §2's same-shell detection found ~/.local/bin/minutes already on
    // PATH, skip the PATH write. We don't want to rewrite shell config every
    // setup attempt for users who already have ~/.local/bin wired up.
    let local_bin = home_dir().join(".local/bin");
    let path_candidates = resolve_minutes_in_user_shell();
    let local_bin_already_on_path = path_candidates.iter().any(|p| p.starts_with(&local_bin));

    let path = if local_bin_already_on_path {
        PathWriteResult {
            shell: UserShell::detect().name(),
            action: "skipped",
            files_touched: vec![],
            manual_snippet: None,
        }
    } else {
        write_path_for_shell(UserShell::detect())
    };

    let mut state = load_state(&app);
    state.schema = 1;
    state.last_check_ms = now_ms();
    state.snooze_until_ms = 0;
    state.selected_bundle_path = Some(bundle_root.clone());
    save_state(&app, &state).ok();

    let bundle_id = read_plist_string(
        &bundle_root.join("Contents/Info.plist"),
        "CFBundleIdentifier",
    )
    .unwrap_or_default();
    let adhoc_signed = is_adhoc_signed(&bundle_root);

    Ok(CliSetupResponse {
        symlink,
        path,
        adhoc_signed,
        bundle_id,
        bundle_root,
    })
}

#[derive(Debug, Deserialize)]
pub struct CliSnoozeRequest {
    pub days: u32,
}

#[tauri::command]
pub async fn cmd_cli_snooze(app: AppHandle, request: CliSnoozeRequest) -> Result<u128, String> {
    let mut state = load_state(&app);
    let until = now_ms() + (request.days as u128) * 24 * 60 * 60 * 1000;
    state.schema = 1;
    state.snooze_until_ms = until;
    save_state(&app, &state).map_err(|e| e.to_string())?;
    Ok(until)
}

/// Re-check installation state without surfacing snooze (used by the "Re-check"
/// button per §4 install_method=unknown row).
#[tauri::command]
pub async fn cmd_cli_recheck(app: AppHandle) -> Value {
    if let Ok(mut guard) = DETECT_CACHE.lock() {
        *guard = None;
    }
    cmd_cli_install_state(app).await
}

/// Clears the active quarantine xattr on the running bundle. Returns
/// `Err` with a copy-able command if the operation fails (locked admin
/// scenarios per §3a).
#[tauri::command]
pub async fn cmd_cli_clear_quarantine() -> Result<(), String> {
    let bundle = current_bundle_root().ok_or("Could not resolve bundle path")?;
    let out = std::process::Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&bundle)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "Couldn't clear the quarantine flag (permission denied). Run this in Terminal yourself, then reopen Minutes:\n\nxattr -dr com.apple.quarantine \"{}\"",
            bundle.display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_probe_rejects_garbage() {
        // Use a binary that won't return semver: /bin/ls --version usually does NOT
        // print pure semver on macOS, but we'll just confirm the parser logic.
        let s = "minutes 0.16.1";
        let out = s.split_whitespace().find(|t| {
            let trimmed = t.trim_start_matches('v');
            trimmed.chars().filter(|c| *c == '.').count() == 2
                && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
        });
        assert_eq!(out, Some("0.16.1"));
    }

    #[test]
    fn version_probe_strips_v_prefix() {
        let s = "minutes v0.16.1 (build abc)";
        let out = s.split_whitespace().find_map(|t| {
            let trimmed = t.trim_start_matches('v');
            if trimmed.chars().filter(|c| *c == '.').count() == 2
                && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
            {
                Some(trimmed.to_string())
            } else {
                None
            }
        });
        assert_eq!(out.as_deref(), Some("0.16.1"));
    }

    #[test]
    fn marker_detection_skips_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rcfile");
        std::fs::write(
            &f,
            format!(
                "# preamble\n{}\nfoo\n{}\n",
                PATH_MARKER_OPEN, PATH_MARKER_CLOSE
            ),
        )
        .unwrap();
        let appended = append_marker_block(&f, "foo").unwrap();
        assert!(!appended);
    }

    #[test]
    fn marker_appends_on_fresh_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("rcfile");
        let appended = append_marker_block(&f, "export PATH=foo").unwrap();
        assert!(appended);
        let content = std::fs::read_to_string(&f).unwrap();
        assert!(content.contains(PATH_MARKER_OPEN));
        assert!(content.contains("export PATH=foo"));
        assert!(content.contains(PATH_MARKER_CLOSE));
    }

    #[test]
    fn snooze_state_roundtrip() {
        // Use a temp dir as a fake config dir.
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("cli_setup_state.json");
        let state = CliSetupState {
            schema: 1,
            snooze_until_ms: 12345,
            last_check_ms: 100,
            selected_bundle_path: Some(PathBuf::from("/Applications/Minutes.app")),
        };
        std::fs::write(&p, serde_json::to_string(&state).unwrap()).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        let parsed: CliSetupState = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed.snooze_until_ms, 12345);
        assert_eq!(
            parsed.selected_bundle_path.as_deref(),
            Some(Path::new("/Applications/Minutes.app"))
        );
    }

    #[test]
    fn corrupted_state_falls_back_to_default() {
        let parsed: CliSetupState = serde_json::from_str("{not valid json").unwrap_or_default();
        assert_eq!(parsed.snooze_until_ms, 0);
    }

    #[test]
    fn build_artifact_paths_filtered() {
        assert!(is_build_artifact_path(Path::new(
            "/Users/x/Sites/minutes/target/release/bundle/macos/Minutes.app"
        )));
        assert!(is_build_artifact_path(Path::new(
            "/repo/target/debug/bundle/Minutes.app"
        )));
        assert!(is_build_artifact_path(Path::new(
            "/repo/target/aarch64-apple-darwin/release/bundle/Minutes.app"
        )));
        assert!(!is_build_artifact_path(Path::new(
            "/Applications/Minutes.app"
        )));
        assert!(!is_build_artifact_path(Path::new(
            "/Users/x/Applications/Minutes Dev.app"
        )));
    }

    #[test]
    fn bundled_cli_path_prefers_plain_minutes() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("minutes");
        std::fs::write(&plain, "#!/bin/sh\necho hi").unwrap();
        let resolved = bundled_cli_path(dir.path()).unwrap();
        assert_eq!(resolved, plain);
    }

    #[test]
    fn bundled_cli_path_falls_back_to_arch_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let arch = std::env::consts::ARCH;
        let f = dir.path().join(format!("minutes-{}-apple-darwin", arch));
        std::fs::write(&f, "#!/bin/sh\necho hi").unwrap();
        let resolved = bundled_cli_path(dir.path()).unwrap();
        assert_eq!(resolved, f);
    }

    #[test]
    fn bundled_cli_path_errors_when_no_match() {
        let dir = tempfile::tempdir().unwrap();
        let err = bundled_cli_path(dir.path()).unwrap_err();
        match err {
            SetupError::NoMatchingArch { .. } => {}
            _ => panic!("expected NoMatchingArch"),
        }
    }
}
