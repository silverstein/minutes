# Plan: Bundle CLI Inside the Tauri App

## Problem

When a user updates the Tauri desktop app, the CLI binary (`~/.cargo/bin/minutes`,
`~/.local/bin/minutes`, or a Homebrew install) is not updated. Any CLI command added
in the new release (`minutes vocabulary`, for example) silently uses the old binary.
The user has to know to run `cargo install`, `brew upgrade`, or equivalent — with no
signal in the app telling them to do so.

Non-developers who use only the desktop app are **unaffected** (confirmed by audit:
the app calls `minutes-core` directly as a Rust library for all operations and never
execs the PATH `minutes` binary). This problem is scoped to users who use both the
app and the CLI.

## Proposed Solution

Bundle the compiled CLI binary inside the `.app` bundle and offer a one-time, opt-in
setup that creates `~/.local/bin/minutes` as a symlink into the bundle, plus a PATH
entry for `~/.local/bin`. After that one setup step, every app update automatically
updates the CLI (the symlink target is inside the bundle, which is replaced on update).

This is the same pattern Postgres.app uses for `psql`, `pg_dump`, etc.

**Why a symlink in `~/.local/bin` and not `Contents/MacOS` directly on PATH:**

1. The bundled binary must be named `minutes-aarch64-apple-darwin` (Tauri sidecar
   convention, required for build-time discovery and notarization). Adding
   `Contents/MacOS` to PATH would leave `which minutes` returning nothing because the
   on-disk name is the arch-suffixed one.
2. We cannot rename or symlink *inside* `Contents/MacOS/` post-build without breaking
   the code signature on the bundle.
3. `~/.local/bin` is a stable, well-understood location, already commonly on user
   PATH, and the symlink can be replaced/removed without touching the signed bundle.

## Implementation

### 1. Bundle the CLI binary as a Tauri sidecar

Use Tauri's `externalBin` / `sidecar` mechanism. Binaries placed in `Contents/MacOS/`
outside the sidecar flow do not inherit the app's notarization and trigger "developer
cannot be verified" the first time the binary is exec'd from a terminal.

In `tauri.conf.json`:
```json
{
  "bundle": {
    "externalBin": ["binaries/minutes"],
    "macOS": {
      "entitlements": "entitlements.plist"
    }
  }
}
```

The binary must be named with a target triple suffix at build time
(`minutes-aarch64-apple-darwin`, `minutes-x86_64-apple-darwin`). Tauri's sidecar
naming requires arch-suffixed files — a single fat binary via `lipo` is NOT an
option for sidecars.

**v1 ships aarch64-only sidecars.** Apple Silicon is ~95% of the user base and
cross-compiling x86_64 from aarch64 requires `rustup target add x86_64-apple-darwin`
plus C++ deps (whisper.cpp, parakeet) cross-compiling cleanly with x86_64 SDK
flags — non-trivial and not currently wired into the `Release macOS` workflow.
Intel-Mac users continue to use brew/cargo for v1; the in-app UI for `install_method
!= "none" + cargo/brew` already handles them gracefully (§4 table).

`scripts/build.sh` (v1):

```bash
# CLI is built for the host arch only; ship one sidecar.
TARGET="$(rustc -Vv | awk '/host:/ {print $2}')"
mkdir -p tauri/src-tauri/binaries
cp target/release/minutes "tauri/src-tauri/binaries/minutes-${TARGET}"
```

x86_64 sidecar bundling is a v2 follow-up once the cross-compile pipeline is in
place. When v2 ships both, the runtime arch-glob in `bundled_cli_path()` already
handles the case correctly — no runtime code change needed, only the build script
expands to a loop over both targets.

**Runtime arch resolution (do not hardcode the host triple).** The runtime symlink
target name must match the architecture of the *running CLI process*, which on
Apple Silicon may be x86_64 if launched under Rosetta. Resolve by globbing the
bundle, NOT by `rustc`-style host detection:

```rust
fn bundled_cli_path(macos_dir: &Path) -> Result<PathBuf, SetupError> {
    // std::env::consts::ARCH is "aarch64" or "x86_64" — matches the running
    // process's arch even under Rosetta translation.
    let arch = std::env::consts::ARCH;
    let triple = format!("{}-apple-darwin", arch);
    let candidate = macos_dir.join(format!("minutes-{}", triple));
    if candidate.exists() { return Ok(candidate); }
    // Fallback: glob and pick anything matching the arch (handles future triples).
    for entry in std::fs::read_dir(macos_dir)? {
        let p = entry?.path();
        if let Some(n) = p.file_name().and_then(|n| n.to_str()) {
            if n.starts_with("minutes-") && n.contains(arch) { return Ok(p); }
        }
    }
    Err(SetupError::NoMatchingArch { arch: arch.to_string(), looked_in: macos_dir.into() })
}
```

If no matching arch binary is found, refuse setup with: "This Minutes.app build
doesn't include a CLI binary for your architecture (`{arch}`). Reinstall from the
official DMG, which ships Universal binaries."

#### 1a. Sidecar entitlements (load-bearing — not optional)

The CLI uses microphone access (`com.apple.security.device.audio-input`). When the
sidecar binary is exec'd from a terminal it is the *process requesting access*, not
the Tauri app, so it must carry its own entitlement. Without this, `minutes record`
silently fails TCC on first terminal run with no user prompt.

Create `tauri/src-tauri/minutes-cli.entitlements`:
```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>com.apple.security.device.audio-input</key>
  <true/>
  <key>com.apple.security.cs.allow-jit</key>
  <false/>
</dict>
</plist>
```

In `scripts/build.sh`, after Tauri produces the bundle but before notarization, sign
the sidecar explicitly with its own entitlements. Fall back to ad-hoc signing for
OSS contributors (per `CLAUDE.md`'s OSS contributor note):

```bash
SIGN_ID="${MINUTES_DEV_SIGNING_IDENTITY:--}"   # "-" = ad-hoc
codesign --force --options runtime \
  --entitlements tauri/src-tauri/minutes-cli.entitlements \
  --sign "$SIGN_ID" \
  "Minutes.app/Contents/MacOS/minutes-${TARGET}"
```

This must happen *before* the outer `codesign` of the `.app` and *before* notarization
submission.

**Ad-hoc-signed sidecars do not get TCC entitlements honored** — entitlements without
a Team ID identity are largely ignored by macOS. Contributor builds will hit the TCC
denial issue described in §4b on first terminal `minutes record`. This is expected
and documented; only release builds with the real cert get the clean flow. Surface
this in the Settings → About row when the running bundle is ad-hoc-signed (detect
via `codesign -dv` checking for `Authority=` lines).

### 2. Detect CLI state at runtime

```rust
#[tauri::command]
pub async fn cmd_cli_install_state(app: tauri::AppHandle) -> serde_json::Value {
    let app_version = app.config().version.clone().unwrap_or_default();

    // Resolve the running app's MacOS directory.
    // Note: Tauri v2's app.path().app_dir() returns ~/Library/Application Support/
    // <bundle_id> (user data dir), NOT the .app bundle path. Use current_exe()
    // and walk up. current_exe() returns <bundle>/Contents/MacOS/<binary>, so
    // .parent() is Contents/MacOS — exactly what we want.
    let app_macos_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    let bundle_root = app_macos_dir
        .as_ref()
        .and_then(|p| p.parent().and_then(|c| c.parent()).map(Path::to_path_buf));

    // App Translocation detection (belt-and-suspenders).
    // Apple has shifted these paths between macOS versions, so check both the
    // path substring AND the quarantine xattr.
    // current_exe() is intentional here — translocation specifically affects the
    // executing path, not the bundle root that app_dir() returns.
    let current = std::env::current_exe().ok();
    let translocated = current.as_ref().map(|p| {
        let s = p.to_string_lossy();
        s.contains("/AppTranslocation/") || has_quarantine_xattr(p)
    }).unwrap_or(false);

    // PATH detection: must use the *user's interactive shell*, dispatched by $SHELL,
    // not always zsh. A bash/fish/nushell user with ~/.local/bin in their config
    // appears to lack it under zsh -l, which would falsely trigger §3c writes to a
    // shell config they don't source.
    //
    // Returns ALL candidates (which -a equivalent), so we can detect conflicts
    // (multiple `minutes` binaries on PATH).
    let path_candidates = resolve_minutes_in_user_shell();
    let path_binary = path_candidates.first().cloned();
    let path_version = path_binary.as_ref().and_then(read_binary_version_safe);

    let app_ver_norm = app_version.trim_start_matches('v');
    let path_ver_norm = path_version.as_deref().map(|v| v.trim_start_matches('v').to_string());
    let in_sync = path_ver_norm.as_deref() == Some(app_ver_norm);

    let mut install_method = detect_install_method(&path_binary).await;
    // Conflict surfaces when multiple `minutes` binaries are on PATH. The active
    // one (first on PATH) may not be the one the user just symlinked.
    if path_candidates.len() > 1 {
        install_method = "conflict";
    }

    // Discover other Minutes installs on disk (for Repair UX in §6).
    let bundle_id = app.config().identifier.clone();
    let known_bundles = discover_minutes_bundles(&app);

    serde_json::json!({
        "app_version": app_version,
        "app_macos_dir": app_macos_dir,
        "bundle_id": bundle_id,                 // "com.useminutes.desktop" or .dev
        "translocated": translocated,
        "path_binary": path_binary,
        "path_candidates": path_candidates,     // all `minutes` on PATH
        "path_version": path_version,
        "in_sync": in_sync,
        "install_method": install_method,
        "known_bundles": known_bundles,         // see §6
    })
}

// Dispatch detection by the user's actual shell. Each shell sources its own
// startup files; cross-shell guesses are wrong about half the time.
fn resolve_minutes_in_user_shell() -> Vec<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let shell_name = std::path::Path::new(&shell)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("zsh");

    // `which -a` semantics: print every match on PATH, in PATH order.
    let cmd = match shell_name {
        "bash" => vec!["-lc", "command -v -a minutes 2>/dev/null || which -a minutes 2>/dev/null"],
        "zsh"  => vec!["-lc", "which -a minutes 2>/dev/null"],
        "fish" => vec!["-c",  "type -ap minutes 2>/dev/null"],
        "nu"   => vec!["-c",  "which minutes | get path | str join \"\\n\""],
        _      => vec!["-lc", "which -a minutes 2>/dev/null"],
    };

    let out = std::process::Command::new(&shell)
        .args(&cmd)
        .output()
        .ok();

    let lines: Vec<PathBuf> = out
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect())
        .unwrap_or_default();

    if lines.is_empty() {
        // Last resort: which::which with our process PATH augmented with common
        // locations. Better than returning nothing.
        which::which("minutes").ok().into_iter().collect()
    } else {
        lines
    }
}

fn has_quarantine_xattr(p: &Path) -> bool {
    std::process::Command::new("/usr/bin/xattr")
        .args(["-p", "com.apple.quarantine"])
        .arg(p)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// Safe version probe: 1s timeout, semver-ish output validation.
// Without these guards, an unrelated `minutes` binary could hang or return junk.
fn read_binary_version_safe(p: &Path) -> Option<String> {
    use std::sync::mpsc;
    use std::time::Duration;
    let path = p.to_path_buf();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let r = std::process::Command::new(&path).arg("--version").output();
        let _ = tx.send(r);
    });
    let out = rx.recv_timeout(Duration::from_secs(1)).ok()?.ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    // Expect "minutes 0.16.1" or similar. No need for regex — pick the first
    // whitespace-separated token with two dots and only digits/dots.
    s.split_whitespace()
        .find(|t| {
            let trimmed = t.trim_start_matches('v');
            trimmed.chars().filter(|c| *c == '.').count() == 2
                && trimmed.chars().all(|c| c.is_ascii_digit() || c == '.')
        })
        .map(|t| t.trim_start_matches('v').to_string())
}
```

#### 2a. `detect_install_method` with timeout + cache

`brew list` shells out to Homebrew (Ruby) and can hang 5-10s on cold state.
`cmd_cli_install_state` runs every Settings → About open. Wrap in a timeout and cache.

```rust
// 60s TTL cache, keyed by (path_binary). Background-thread the brew check with a
// 2-second hard timeout. On timeout, return install_method="unknown" and let the
// next check refresh.
async fn detect_install_method(path_binary: &Option<PathBuf>) -> &'static str {
    let Some(p) = path_binary else { return "none"; };
    let s = p.to_string_lossy();
    if s.contains(".cargo/bin") { return "cargo"; }
    if s.contains("/Minutes.app/") || s.contains("/Minutes Dev.app/") { return "bundled"; }
    // Brew check — must use the fully qualified tap name to avoid false positives
    // from unrelated `minutes` formulae in other taps.
    match tokio::time::timeout(
        Duration::from_secs(2),
        tokio::process::Command::new("brew")
            .args(["list", "silverstein/tap/minutes"])
            .output(),
    ).await {
        Ok(Ok(out)) if out.status.success() => "brew",
        Ok(_) => "other",
        Err(_) => "unknown",
    }
}
```

### 3. Setup: symlink, PATH, and shell

The setup action does three things, in order. Each step is idempotent.

#### 3a. Refuse if translocated

If `cmd_cli_install_state` returned `translocated=true`, do not write anything.
Show: "Minutes is running from a temporary quarantine location. Move Minutes.app to
your Applications folder, then reopen and try again."

Optional "Fix automatically" button runs `xattr -dr com.apple.quarantine "<app_path>"`
then prompts relaunch. **The xattr command requires write permission on the bundle**
— usually fine in `/Applications/`, but can fail on system-managed installs or
locked admin scenarios. On non-zero exit:

- Surface the failure: "Couldn't clear the quarantine flag (permission denied).
  Run this in Terminal yourself, then reopen Minutes:"
- Show the exact `xattr -dr com.apple.quarantine "/path/to/Minutes.app"` command
  in a copy-able code block.

Never silently fail this step.

#### 3b. Create the symlink

```rust
// Bundle's MacOS dir from current_exe() (Tauri's app_dir() is user data, not the
// bundle path). If /Applications/Minutes.app is itself a symlink to a dev tree,
// current_exe() may return the realpath — that's acceptable: the dev tree IS the
// ground truth in that case, and the user can re-run setup after they install
// the production app.
let app_macos_dir = std::env::current_exe()?
    .parent()
    .ok_or(SetupError::NoMacosDir)?
    .to_path_buf();
// bundled_cli_path() globs for an arch-matching binary (see §1) — must NOT
// hardcode the host triple at build time.
let target = bundled_cli_path(&app_macos_dir)?;
let local_bin = home_dir().join(".local/bin");

// Note: creating ~/.local/bin can have side effects on macOS (other dev tools may
// start writing there). Surface this in the success UI (§5).
let local_bin_existed = local_bin.exists();
std::fs::create_dir_all(&local_bin)?;
let link = local_bin.join("minutes");

// Idempotency: if a symlink already points at our target, do nothing.
// If a different file exists, back it up to minutes.bak-<timestamp> rather than
// overwriting (could be a user-installed binary).
match std::fs::symlink_metadata(&link) {
    Ok(meta) if meta.file_type().is_symlink() => {
        let existing = std::fs::read_link(&link)?;
        if existing == target { /* already correct */ } else {
            std::fs::remove_file(&link)?;
            std::os::unix::fs::symlink(&target, &link)?;
        }
    }
    Ok(_) => {
        // Real file, not a symlink. Back it up before replacing.
        // Use ms precision so back-to-back setup runs (during testing) don't collide.
        let backup = local_bin.join(format!("minutes.bak-{}", unix_ts_ms()));
        std::fs::rename(&link, &backup)?;
        std::os::unix::fs::symlink(&target, &link)?;
        // Surface the backup path in the success UI.
    }
    Err(_) => std::os::unix::fs::symlink(&target, &link)?,
}

// Cap retention: keep the 3 most recent .bak-* files; delete older ones.
prune_bak_files(&local_bin, 3)?;
```

The symlink target is *inside the bundle*. When the app is updated (new bundle
replaces old at the same path), the symlink still resolves correctly — the bundle
location doesn't change, only its contents.

#### 3c. Add `~/.local/bin` to PATH if missing

**Detection AND write must use the same shell.** §2 resolved which `minutes` is on
PATH using the user's actual `$SHELL`. The write step uses the same dispatch:

| `$SHELL` basename | Detection (§2) | Write (§3c) |
|-------------------|----------------|-------------|
| `zsh` | `zsh -lc "which -a minutes"` | Append marker block to `~/.zshrc` |
| `bash` | `bash -lc "command -v -a minutes"` | Append marker block to BOTH `~/.bash_profile` (login: macOS Terminal.app) AND `~/.bashrc` (non-login: VS Code, IntelliJ, tmux without `-l`). Idempotent within each file via marker check; cross-file duplicate `export` is a no-op. |
| `fish` | `fish -c "type -ap minutes"` | Exec `fish -c "fish_add_path -U $HOME/.local/bin"` (universal var, no file write) |
| `nu` (nushell) | `nu -c "which minutes ..."` | Show copy-able snippet for `~/.config/nushell/env.nu`; do not write |
| other / unknown | `which::which("minutes")` | Show copy-able snippet; do not write |

For zsh/bash, the appended block uses unambiguous markers for idempotency and
clean removal:

```sh
# >>> minutes-app PATH (managed) >>>
export PATH="$HOME/.local/bin:$PATH"
# <<< minutes-app PATH (managed) <<<
```

Before writing, scan the file for the opening marker. If present, skip the write.
On uninstall, the block can be removed by matching the marker pair.

For fish, `fish_add_path -U` writes to the universal variable store
(`fish_user_paths`). Idempotent by design — re-running does not duplicate.
Older fish (< 3.2.0) lacks `fish_add_path`; if `fish --version` reports < 3.2.0,
fall back to a copy-able `set -U fish_user_paths $HOME/.local/bin $fish_user_paths`.

If §2's same-shell detection found `~/.local/bin/minutes` on PATH already, skip §3c
entirely (just create/update the symlink). The cross-shell trap from earlier drafts
is now closed: detection and write always agree because they run in the same shell.

#### 3d. Dev app: show the setup, don't suppress it

Earlier draft suppressed setup for `Minutes Dev.app`. That means the feature is
literally untestable before release. Instead, show it with a clear label:

> **Dev build.** This will link `~/.local/bin/minutes` to your Dev app's bundled
> CLI. Use this for QA only — uninstall before testing the production setup flow.

The symlink and PATH-write logic is identical; only the UI copy differs.

### 4. UI: where and when to show the offer

**Rule by install_method:**

| install_method | in_sync | Show |
|---------------|---------|------|
| `"none"` | — | "Set up CLI" button (creates symlink + PATH) |
| `"bundled"` | true | Nothing (already correct) |
| `"bundled"` | false | "Repair CLI link" (re-create symlink — points at the new bundle path) |
| `"brew"` | false | "I'll update it myself" with `brew upgrade silverstein/tap/minutes`; no setup offer — do not shadow a managed install |
| `"brew"` | true | Nothing |
| `"cargo"` | false | "I'll update it myself" with `cargo install minutes-cli`; no setup offer |
| `"cargo"` | true | Nothing |
| `"other"` | false | "I'll update it myself" with a generic upgrade hint; no setup offer |
| `"conflict"` | — | "Multiple `minutes` binaries on PATH" — list `path_candidates`; offer "Remove stale symlink" if our symlink is shadowed |
| `"unknown"` | — | Show the row with a "Re-check" button; suppress setup until method is determined |

If `translocated=true`, override every row above with the App Translocation message
from §3a.

**Two surfaces:**

1. **"What's New" modal** — after every app update, if `in_sync` is false or
   `install_method` is `"none"`, add a section below the release notes. "I'll do it
   myself" snoozes this modal for 7 days but does NOT clear the Settings row.

2. **Settings → About** — persistent row: "CLI: 0.13.3 (app: 0.16.1) · Set up" that
   stays visible until `in_sync` becomes true. Snoozing the modal does not affect
   this row. This is the safety net that catches users who dismiss the modal without
   updating.

#### 4a. Snooze persistence

State lives at `app.path().app_config_dir().join("cli_setup_state.json")`:

```json
{ "schema": 1, "snooze_until_ms": 1717800000000, "last_check_ms": 1717195200000 }
```

- Per-bundle: prod (`com.useminutes.desktop`) and dev (`com.useminutes.desktop.dev`)
  get separate config dirs automatically — no cross-contamination.
- UTC milliseconds — DST and travel safe.
- `~/Library/Application Support/<bundle_id>/` — the app updater never touches this on
  update, so snooze state survives.
- On corrupted JSON, treat as no snooze (fail open). Log and overwrite on next write.

#### 4b. First-run TCC prompt UX (document, don't try to merge)

When the user first runs `minutes record` from a terminal after setup, macOS will
prompt for microphone access **separately from the desktop app's existing grant**,
because TCC scopes by signed-binary identity (Team ID + bundle ID), and the CLI
sidecar has its own bundle ID-less identity. The prompt may show "minutes" with a
generic icon rather than the Minutes app icon.

The setup confirmation (§5) MUST surface this expectation up front:

> The first time you run `minutes record` from a terminal, macOS will ask for
> microphone access. This is a separate grant from the desktop app — that's
> normal. Click Allow.

Do NOT try to invoke the CLI via `open -a Minutes.app --args record` to inherit the
app's grant. That breaks the `minutes` → terminal mental model and complicates
shell scripting. Two grants is the honest tradeoff.

### 5. Setup happy path

1. Check `translocated` flag; if true, show the move-app message and stop.
2. Resolve target sidecar via `bundled_cli_path()` (§1, arch-glob from
   `current_exe().parent()` — which IS `Contents/MacOS`).
3. **Confirm bundle identity before linking** (see §6 for multi-bundle handling):
   - Show the resolved bundle path and `bundle_id` in the confirmation dialog.
   - For `bundle_id == com.useminutes.desktop.dev`, require a second click with
     warning copy: "This is the Dev build. The CLI on your terminal will run the dev
     binary. Continue?"
4. Create or update `~/.local/bin/minutes` symlink (§3b).
5. If §2 same-shell detection found `~/.local/bin/minutes` on PATH, skip §3c.
6. Otherwise, detect shell and write the PATH block (§3c).
7. Show confirmation:
   - "Linked `~/.local/bin/minutes` → `<bundle_path>/Contents/MacOS/minutes-<arch>-apple-darwin`"
   - "Added `~/.local/bin` to your `<shell>` PATH" (or fish/nushell equivalent)
   - If `~/.local/bin` was just created (`!local_bin_existed` in §3b): "Created
     `~/.local/bin` — other developer tools (pipx, npm, etc.) may also use this
     directory."
   - "Open a new terminal and run `minutes --version` to confirm."
   - The first-run TCC prompt note from §4b.
   - If a backup was made (§3b), surface the backup path.
8. Update Settings → About to re-check state. **In-session `in_sync` works without
   waiting for shell PATH propagation:** when `install_method="bundled"`, the check
   reads the symlink target directly (`fs::read_link("~/.local/bin/minutes")`) and
   probes it with `read_binary_version_safe`, comparing to `app_version`. Skip the
   shell-PATH detection for the in_sync verdict. Use shell-PATH detection only to
   determine `install_method` and `path_candidates` for conflict surfacing. This
   makes the Settings row clear immediately after setup, no relaunch needed.

### 6. Multiple installs and Repair UX

Power users and maintainers commonly have multiple Minutes bundles on disk —
e.g., `/Applications/Minutes.app` (production) and `~/Applications/Minutes Dev.app`
(dev). The setup flow MUST be explicit about which bundle it is linking to, or the
symlink will silently flip between prod and dev as the user opens whichever app
happens to be in front.

`cmd_cli_install_state` populates `known_bundles` via Spotlight:

```rust
fn discover_minutes_bundles(app: &tauri::AppHandle) -> Vec<serde_json::Value> {
    // Spotlight may be disabled (power users, locked-down corp Macs); union mdfind
    // results with a probe of well-known install locations so we don't silently miss
    // a bundle and let setup link to the wrong one.
    // Wildcard syntax verified for mdfind: `==[c]` is case-insensitive equals,
    // `==` with `*` glob requires the `*` outside the quotes or use `like`. Simplest
    // reliable form is the explicit OR query below.
    let query = "kMDItemCFBundleIdentifier == 'com.useminutes.desktop' \
                 || kMDItemCFBundleIdentifier == 'com.useminutes.desktop.dev'";
    let out = std::process::Command::new("/usr/bin/mdfind")
        .arg(query)
        .output();
    let mut paths: Vec<PathBuf> = out
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines()
            .map(PathBuf::from)
            .collect())
        .unwrap_or_default();

    // Fallback probe: well-known install paths.
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

    // Bundle root of the running app (NOT app_dir — that's user data dir in Tauri v2).
    let running = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().and_then(|c| c.parent()).and_then(|c| c.parent()).map(Path::to_path_buf))
        .unwrap_or_default();
    paths.into_iter().filter_map(|p| {
        let info_plist = p.join("Contents/Info.plist");
        let bundle_id = read_plist_string(&info_plist, "CFBundleIdentifier")?;
        let version = read_plist_string(&info_plist, "CFBundleShortVersionString")?;
        Some(serde_json::json!({
            "path": p,
            "bundle_id": bundle_id,
            "version": version,
            "is_running": p == running,
        }))
    }).collect()
}
```

**Multi-bundle picker rules** (apply to BOTH initial setup AND Repair):

- If `known_bundles.len() == 1`: link against that bundle without asking. Show:
  "Linked CLI → `<path>` (v<version>)."
- If `known_bundles.len() > 1`: show a picker listing each bundle (path, version,
  bundle_id, is_running). **Default selection prefers the production bundle**
  (`com.useminutes.desktop`) when present; falls back to the running app otherwise.
  Reasoning: the running app is often Dev (developer is QA'ing setup itself), but
  the user almost always wants their CLI pointed at Prod. Require explicit
  confirmation. Persist the user's pick so subsequent auto-repairs don't re-prompt
  unless their pick disappears.

The picker runs on initial setup too — not just Repair — whenever `known_bundles`
has more than one entry. The previous draft only triggered the picker in the
Repair path, which let initial setup silently link to whichever bundle happened
to be running.

### 7. App update interaction with active recordings

The setup flow's promise — "every app update automatically updates the CLI" — has
a sharp edge: when the app updater replaces `Minutes.app` in-place,
any running CLI process holding the old inode keeps executing fine, but **child
processes spawned after the swap exec the new binary**. If the new version has
incompatible IPC, JSONL schema, or PID-file semantics, mid-recording sessions
corrupt silently.

**Mitigation: defer app updates while a recording is active.**

Before triggering or accepting an app-updater install, check
`PidGuard::is_recording_active()` (the existing flock-backed check used by the CLI
mutual-exclusion logic). If true:

- Defer the install. Show a non-modal banner in Settings: "Update <version> is
  ready. It will install when you stop the current recording."
- Re-check on every recording state transition; install once `recording.pid` is
  released.
- For `minutes live` (live transcript), apply the same guard.
- Document in release notes for any version that changes JSONL schema or PID
  semantics: "Stop any active recording before updating."

This is a small change (one guard, one event listener) but closes the silent
mid-recording corruption window.

## What does NOT change

- Separate CLI release artifacts (GitHub releases, Homebrew formula, `cargo install`)
  are unchanged. CLI-only and MCP/MCPB users are unaffected.
- The app never shells out to the PATH `minutes` binary internally (confirmed by
  audit of all `Command::new` calls in the Tauri source).
- Setup is always opt-in. The app never silently modifies shell config or creates
  symlinks without an explicit user click.
- Brew/cargo-managed installs are never touched or shadowed without explicit user choice.
- We never write into `Contents/MacOS/` post-build (would invalidate the signature).

## Bundle size impact

The CLI binary is ~6.7MB (release build). Because the app already links `minutes-core`,
the additional compressed payload on the DMG is estimated at ~1-2MB (linker
deduplication of shared code). Exact delta to be measured at build time.

## Out of scope

- **Windows:** no `.app` bundle; PATH setup mechanism differs. Defer to follow-up.
- **Linux:** similar to Windows. The detection and "I'll do it myself" prompt still
  works cross-platform; the symlink+PATH write step is deferred.
- **Automatic silent update of brew/cargo installs.** We never touch managed installs.

## Helpers assumed

The code snippets reference these utility functions without inline definitions —
they're either trivial or use existing crates:

- `home_dir()` → `dirs::home_dir().expect("HOME unset")` (the `dirs` crate; std's
  `env::home_dir` is deprecated)
- `unix_ts_ms()` → `SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()`
- `prune_bak_files(dir, keep)` → list `minutes.bak-*` in `dir`, sort by mtime
  descending, `remove_file` everything past index `keep`
- `read_plist_string(plist_path, key)` → shell out to `/usr/bin/defaults read
  <plist> <key>` or use the `plist` crate
- `bundled_cli_path(macos_dir)` → defined inline in §1
- `host_target_triple()` → DELETED in this revision; replaced by arch-glob via
  `bundled_cli_path()`
- `SetupError` → an error enum with at least these variants:
  - `NoMatchingArch { arch: String, looked_in: PathBuf }` (§1)
  - `NoMacosDir` (§3b — `current_exe()` had no parent)
  - plus standard `Io(io::Error)` for the symlink/file ops
- No `regex` crate dependency needed: §2's version probe uses a token-scan instead
  (kept lightweight to avoid pulling regex into the Tauri binary)

## Pre-implementation verification (do these spikes before writing code)

1. **Sidecar entitlement survival under outer codesign.** §1a signs the sidecar
   with its own entitlements, then Tauri's bundler runs `codesign --deep` on the
   `.app`. Verify the inner entitlements survive.

   **Verification:** After the build, run
   `codesign -d --entitlements - "Minutes.app/Contents/MacOS/minutes-${TARGET}"`
   and confirm `com.apple.security.device.audio-input = true` is present.

   **Expected good outcome:** the inner entitlements persist because `codesign --deep`
   only re-signs nested code if it has changed; an already-signed sidecar with valid
   entitlements is left alone.

   **If the inner entitlements are gone:** `--preserve-metadata=entitlements` is the
   wrong flag (that's for re-signing scenarios). The fix is reordering: outer sign
   first WITHOUT `--deep`, then re-sign the sidecar with its own entitlements last,
   then notarize the bundle. This pattern has prior art in Electron + custom helper
   apps. Bash sketch:
   ```bash
   codesign --force --options runtime --sign "$SIGN_ID" "Minutes.app"  # no --deep
   codesign --force --options runtime \
     --entitlements minutes-cli.entitlements --sign "$SIGN_ID" \
     "Minutes.app/Contents/MacOS/minutes-${TARGET}"
   xcrun notarytool submit Minutes.app.zip --wait ...
   ```

2. **App updater: in-place vs relaunch-based.** §7 assumes in-place replacement
   (`rename(2)` over the existing bundle). If Tauri's updater plugin defers to a
   helper that prompts for relaunch instead, the §7 guard moves from "block install"
   to "block relaunch while recording is active" — a different UX. Verify by reading
   `tauri-plugin-updater`'s install flow on macOS.

## Open questions

1. Should `Repair CLI link` (§6) re-check on every app launch automatically, or only
   when the user opens Settings → About? Auto-check on launch is more proactive but
   adds a startup cost. Lean toward on-launch with only the cheap symlink-resolves
   check (brew check is deferred to Settings open per §2a cache).
2. Nushell write-to-config support: §3c shows a copy-able snippet only because
   nushell config (`env.nu`) syntax is brittle to programmatic editing. Worth
   revisiting if nushell adoption grows.
3. The first-run TCC prompt (§4b) appears once per binary identity. If we ever ship
   a CLI binary signed with a different Team ID (e.g., a community fork), users
   will see the prompt again. Document for forks; nothing to do for upstream.
