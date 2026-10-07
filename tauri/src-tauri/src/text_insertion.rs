use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TextInsertionRequest {
    pub text: String,
    pub mode: TextInsertionMode,
    pub restore_clipboard: bool,
    pub clipboard_snapshot: Option<String>,
    pub expected_target: Option<ActiveTargetContext>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInsertionMode {
    CopyOnly,
    BestEffortVerified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertOutcome {
    Typed,
    Pasted,
    Copied,
    Failed,
    Blocked,
}

impl InsertOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            InsertOutcome::Typed => "typed",
            InsertOutcome::Pasted => "pasted",
            InsertOutcome::Copied => "copied",
            InsertOutcome::Failed => "failed",
            InsertOutcome::Blocked => "blocked",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertMethod {
    NativeAx,
    ClipboardOnly,
    ClipboardPaste,
    Unsupported,
}

impl InsertMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            InsertMethod::NativeAx => "native_ax",
            InsertMethod::ClipboardOnly => "clipboard_only",
            InsertMethod::ClipboardPaste => "clipboard_paste",
            InsertMethod::Unsupported => "unsupported",
        }
    }
}

/// Honest active-app insertion capability for the current desktop session.
/// Clipboard support is separate from type/paste-at-cursor support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TextInsertionCapability {
    MacosNativeOrPaste,
    LinuxX11Paste,
    ClipboardOnly,
    Unavailable,
}

impl TextInsertionCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacosNativeOrPaste => "macos_native_or_paste",
            Self::LinuxX11Paste => "linux_x11_paste",
            Self::ClipboardOnly => "clipboard_only",
            Self::Unavailable => "unavailable",
        }
    }

    pub fn supports_active_app_insertion(self) -> bool {
        matches!(self, Self::MacosNativeOrPaste | Self::LinuxX11Paste)
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTargetContext {
    pub platform: String,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
    pub process_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextInsertionResult {
    pub outcome: InsertOutcome,
    pub method: InsertMethod,
    pub verified: bool,
    pub clipboard_restored: bool,
    pub target_context: Option<ActiveTargetContext>,
    pub message: String,
    #[serde(skip)]
    pub timing: TextInsertionTiming,
}

#[derive(Debug, Clone, Default)]
pub struct TextInsertionTiming {
    /// Time from entering insertion until the target app has accepted the
    /// native typing or paste operation. This deliberately excludes clipboard
    /// restoration and verification bookkeeping.
    pub visible_ms: Option<u64>,
    pub clipboard_restore_ms: Option<u64>,
    pub verification_ms: Option<u64>,
    pub total_ms: u64,
    /// Privacy-safe local diagnostic for why a faster insertion path fell
    /// through. The insertion code never includes dictated text in errors.
    pub diagnostic: Option<String>,
}

impl TextInsertionResult {
    pub fn overlay_state(&self) -> &'static str {
        match self.outcome {
            InsertOutcome::Typed => "typed",
            InsertOutcome::Pasted => "pasted",
            InsertOutcome::Copied => "copied",
            InsertOutcome::Blocked => "blocked",
            InsertOutcome::Failed => "error",
        }
    }
}

pub fn read_clipboard() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let output = std::process::Command::new("pbpaste")
            .output()
            .map_err(|error| format!("Could not read clipboard: {error}"))?;
        if !output.status.success() {
            return Err("pbpaste failed to read the clipboard.".into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    #[cfg(target_os = "linux")]
    {
        linux_read_clipboard()
    }

    #[cfg(target_os = "windows")]
    {
        arboard::Clipboard::new()
            .map_err(|error| format!("Could not open clipboard: {error}"))?
            .get_text()
            .map_err(|error| format!("Could not read clipboard: {error}"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Err("Clipboard snapshot is not implemented on this platform.".into())
    }
}

pub fn insert_text(request: TextInsertionRequest) -> TextInsertionResult {
    let target_context = capture_target_context();

    if request.text.trim().is_empty() {
        return TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::Unsupported,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: "Dictation produced no text to insert.".into(),
            timing: TextInsertionTiming::default(),
        };
    }

    match request.mode {
        TextInsertionMode::CopyOnly => copy_only(&request.text, target_context),
        TextInsertionMode::BestEffortVerified => best_effort_verified(request, target_context),
    }
}

pub fn capture_active_target_context() -> Option<ActiveTargetContext> {
    capture_target_context()
}

pub fn restore_target_focus(context: &ActiveTargetContext) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        restore_macos_target_focus(context)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = context;
        Ok(())
    }
}

pub fn can_insert_into_apps() -> bool {
    current_insertion_capability().supports_active_app_insertion()
}

pub fn current_insertion_capability() -> TextInsertionCapability {
    #[cfg(target_os = "macos")]
    {
        // The current insertion strategy is System Events paste automation.
        // AXIsProcessTrusted() describes Minutes' direct AX access, not whether
        // the child osascript/System Events command can paste. Treating it as a
        // hard preflight produced false copy-only states even after users had
        // granted the visible macOS permissions. The attempted operation is the
        // authoritative capability check; failure still preserves the text.
        insertion_capability_for("macos", false, false, false)
    }

    #[cfg(target_os = "linux")]
    {
        insertion_capability_for(
            "linux",
            linux_wayland_session(),
            linux_x11_session(),
            linux_command_available("xdotool"),
        )
    }

    #[cfg(target_os = "windows")]
    {
        insertion_capability_for("windows", false, false, false)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        insertion_capability_for(std::env::consts::OS, false, false, false)
    }
}

fn insertion_capability_for(
    platform: &str,
    wayland_session: bool,
    x11_session: bool,
    xdotool_available: bool,
) -> TextInsertionCapability {
    match platform {
        "macos" => TextInsertionCapability::MacosNativeOrPaste,
        "windows" => TextInsertionCapability::ClipboardOnly,
        "linux" if x11_session && !wayland_session && xdotool_available => {
            TextInsertionCapability::LinuxX11Paste
        }
        "linux" if wayland_session || x11_session => TextInsertionCapability::ClipboardOnly,
        _ => TextInsertionCapability::Unavailable,
    }
}

pub fn insertion_unavailable_message() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macOS paste automation is unavailable; dictation will be copied."
    }

    #[cfg(target_os = "linux")]
    {
        "Paste at cursor is unavailable in this Linux session; dictation will be copied."
    }

    #[cfg(target_os = "windows")]
    {
        "Type at cursor is not supported on Windows yet; dictation will be copied."
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "Type at cursor is not supported on this platform; dictation will be copied."
    }
}

fn copy_only(text: &str, target_context: Option<ActiveTargetContext>) -> TextInsertionResult {
    match write_clipboard(text) {
        Ok(()) => TextInsertionResult {
            outcome: InsertOutcome::Copied,
            method: InsertMethod::ClipboardOnly,
            verified: true,
            clipboard_restored: false,
            target_context,
            message: "Copied dictation to the clipboard.".into(),
            timing: TextInsertionTiming::default(),
        },
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
            timing: TextInsertionTiming::default(),
        },
    }
}

#[cfg(any(target_os = "macos", test))]
fn target_prefers_clipboard_paste(target: Option<&ActiveTargetContext>) -> bool {
    let bundle_id = target
        .and_then(|context| context.bundle_id.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let app_name = target
        .and_then(|context| context.app_name.as_deref())
        .unwrap_or_default()
        .to_ascii_lowercase();

    let is_minutes_webview = matches!(
        bundle_id.as_str(),
        "com.useminutes.desktop" | "com.useminutes.desktop.dev"
    );

    is_minutes_webview
        || [
            "ghostty",
            "terminal",
            "iterm",
            "wezterm",
            "alacritty",
            "kitty",
            "warp",
        ]
        .iter()
        .any(|marker| bundle_id.contains(marker) || app_name.contains(marker))
}

#[cfg(any(target_os = "macos", test))]
fn native_ax_delivery_verified(
    before: Option<&str>,
    after: Option<&str>,
    inserted_text: &str,
) -> bool {
    matches!((before, after), (Some(before), Some(after)) if before != after && after.contains(inserted_text))
}

#[cfg(target_os = "macos")]
fn copy_after_unverified_native_insert(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
    operation_started: std::time::Instant,
    visible_ms: u64,
    verification_ms: u64,
) -> TextInsertionResult {
    let diagnostic = "native_ax: macOS accepted the write but delivery was not verified";
    tracing::warn!(diagnostic, "dictation native insertion was not verified");

    match write_clipboard(&request.text) {
        Ok(()) => TextInsertionResult {
            outcome: InsertOutcome::Copied,
            method: InsertMethod::ClipboardOnly,
            verified: true,
            clipboard_restored: false,
            target_context,
            message: "Could not verify typing into the active app. Copied dictation instead."
                .into(),
            timing: TextInsertionTiming {
                visible_ms: Some(visible_ms),
                clipboard_restore_ms: None,
                verification_ms: Some(verification_ms),
                total_ms: operation_started.elapsed().as_millis() as u64,
                diagnostic: Some(diagnostic.into()),
            },
        },
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error.clone(),
            timing: TextInsertionTiming {
                visible_ms: Some(visible_ms),
                clipboard_restore_ms: None,
                verification_ms: Some(verification_ms),
                total_ms: operation_started.elapsed().as_millis() as u64,
                diagnostic: Some(format!("{diagnostic}; clipboard_copy: {error}")),
            },
        },
    }
}

#[cfg(target_os = "macos")]
fn best_effort_verified(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
) -> TextInsertionResult {
    let operation_started = std::time::Instant::now();
    if let Err(message) =
        verify_paste_target(request.expected_target.as_ref(), target_context.as_ref())
    {
        return copy_after_block(request, target_context, &message);
    }

    let before_value = focused_ax_value().ok();

    // Terminal accessibility trees can report a successful AXSelectedText
    // write without delivering anything to the live prompt. Route known
    // terminals through the proven clipboard-paste path instead. The embedded
    // xterm surface is reported as the Minutes bundle rather than as a terminal
    // app, so the owned production and development bundles use the same path.
    // Other native controls may use the faster direct path, but only a verified
    // change is allowed to claim `Typed`.
    let native_ax_error = if target_prefers_clipboard_paste(target_context.as_ref()) {
        "skipped for terminal target".to_string()
    } else {
        match macos_ax::insert_selected_text(&request.text) {
            Ok(()) => {
                let visible_ms = operation_started.elapsed().as_millis() as u64;
                let verification_started = std::time::Instant::now();
                let after_value = focused_ax_value().ok();
                let verified = native_ax_delivery_verified(
                    before_value.as_deref(),
                    after_value.as_deref(),
                    &request.text,
                );
                let verification_ms = verification_started.elapsed().as_millis() as u64;
                if !verified {
                    return copy_after_unverified_native_insert(
                        request,
                        target_context,
                        operation_started,
                        visible_ms,
                        verification_ms,
                    );
                }

                let clipboard_error = if request.restore_clipboard {
                    None
                } else {
                    write_clipboard(&request.text).err()
                };
                if let Some(error) = clipboard_error.as_deref() {
                    tracing::warn!(error, "dictation typed but clipboard copy failed");
                    minutes_core::logging::log_error("dictation_clipboard", "", error);
                }

                return TextInsertionResult {
                    outcome: InsertOutcome::Typed,
                    method: InsertMethod::NativeAx,
                    verified: true,
                    clipboard_restored: false,
                    target_context,
                    message: if clipboard_error.is_some() {
                        "Typed dictation into the active app, but could not copy it to the clipboard."
                            .into()
                    } else {
                        "Typed dictation into the active app and copied it to the clipboard.".into()
                    },
                    timing: TextInsertionTiming {
                        visible_ms: Some(visible_ms),
                        clipboard_restore_ms: None,
                        verification_ms: Some(verification_ms),
                        total_ms: operation_started.elapsed().as_millis() as u64,
                        diagnostic: clipboard_error.map(|error| format!("clipboard_copy: {error}")),
                    },
                };
            }
            Err(error) => error,
        }
    };

    let paste_started_ms = operation_started.elapsed().as_millis() as u64;
    match paste_via_clipboard_restoring(
        &request.text,
        request.restore_clipboard,
        request.clipboard_snapshot.as_deref(),
    ) {
        Ok(paste) => {
            let verification_started = std::time::Instant::now();
            let verified = focused_ax_value().ok().is_some_and(|after| {
                before_value.as_ref() != Some(&after) && after.contains(&request.text)
            });
            let verification_ms = verification_started.elapsed().as_millis() as u64;
            TextInsertionResult {
                outcome: if verified {
                    InsertOutcome::Typed
                } else {
                    InsertOutcome::Pasted
                },
                method: InsertMethod::ClipboardPaste,
                verified,
                clipboard_restored: paste.restored,
                target_context,
                message: if verified {
                    "Typed dictation into the active app.".into()
                } else {
                    "Pasted dictation into the active app.".into()
                },
                timing: TextInsertionTiming {
                    visible_ms: Some(paste_started_ms + paste.visible_ms),
                    clipboard_restore_ms: paste.clipboard_restore_ms,
                    verification_ms: Some(verification_ms),
                    total_ms: operation_started.elapsed().as_millis() as u64,
                    diagnostic: Some(format!("native_ax: {native_ax_error}")),
                },
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "dictation paste automation failed");
            minutes_core::logging::log_error("dictation_paste", "", &error);
            TextInsertionResult {
                outcome: InsertOutcome::Copied,
                method: InsertMethod::ClipboardOnly,
                verified: true,
                clipboard_restored: false,
                target_context,
                message: "Could not type into the active app. Copied dictation instead.".into(),
                timing: TextInsertionTiming {
                    total_ms: operation_started.elapsed().as_millis() as u64,
                    diagnostic: Some(format!(
                        "native_ax: {native_ax_error}; clipboard_paste: {error}"
                    )),
                    ..TextInsertionTiming::default()
                },
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn best_effort_verified(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
) -> TextInsertionResult {
    match write_clipboard(&request.text) {
        Ok(()) => {
            if linux_x11_paste_available() {
                match paste_via_xdotool() {
                    Ok(()) => {
                        let restored = restore_clipboard_if_requested(
                            request.restore_clipboard,
                            request.clipboard_snapshot.as_deref(),
                        );
                        return TextInsertionResult {
                            outcome: InsertOutcome::Pasted,
                            method: InsertMethod::ClipboardPaste,
                            verified: false,
                            clipboard_restored: restored,
                            target_context,
                            message: "Pasted dictation into the active X11 app.".into(),
                            timing: TextInsertionTiming::default(),
                        };
                    }
                    Err(error) => {
                        tracing::warn!(error = %error, "linux dictation paste automation failed");
                        return TextInsertionResult {
                            outcome: InsertOutcome::Copied,
                            method: InsertMethod::ClipboardOnly,
                            verified: true,
                            clipboard_restored: false,
                            target_context,
                            message:
                                "Could not paste into the focused X11 app. Copied dictation instead."
                                    .into(),
                            timing: TextInsertionTiming::default(),
                        };
                    }
                }
            }

            TextInsertionResult {
                outcome: InsertOutcome::Copied,
                method: InsertMethod::ClipboardOnly,
                verified: true,
                clipboard_restored: false,
                target_context,
                message: linux_copy_fallback_message(),
                timing: TextInsertionTiming::default(),
            }
        }
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
            timing: TextInsertionTiming::default(),
        },
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn best_effort_verified(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
) -> TextInsertionResult {
    copy_after_block(request, target_context, insertion_unavailable_message())
}

fn copy_after_block(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
    message: &str,
) -> TextInsertionResult {
    match write_clipboard(&request.text) {
        Ok(()) => TextInsertionResult {
            outcome: InsertOutcome::Blocked,
            method: InsertMethod::ClipboardOnly,
            verified: true,
            clipboard_restored: false,
            target_context,
            message: message.into(),
            timing: TextInsertionTiming::default(),
        },
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
            timing: TextInsertionTiming::default(),
        },
    }
}

#[cfg(not(target_os = "macos"))]
fn restore_clipboard_if_requested(restore: bool, snapshot: Option<&str>) -> bool {
    if !restore {
        return false;
    }
    let Some(snapshot) = snapshot else {
        return false;
    };
    std::thread::sleep(Duration::from_millis(150));
    write_clipboard(snapshot).is_ok()
}

#[cfg(test)]
fn clipboard_paste_restore_with(
    text: &str,
    restore: bool,
    snapshot: Option<&str>,
    mut write: impl FnMut(&str) -> Result<(), String>,
    mut paste: impl FnMut() -> Result<(), String>,
    mut wait_before_restore: impl FnMut(),
) -> Result<bool, String> {
    write(text)?;
    paste()?;
    if restore {
        if let Some(snapshot) = snapshot {
            wait_before_restore();
            write(snapshot)?;
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct ClipboardSnapshot {
    change_count: i64,
    text: Option<String>,
    has_plain_text: bool,
    types: Vec<String>,
    items: Option<Vec<std::collections::BTreeMap<String, Vec<u8>>>>,
}

pub fn write_clipboard(text: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        let mut child = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start pbcopy: {error}"))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .map_err(|error| format!("Could not write to clipboard: {error}"))?;
        }
        let status = child
            .wait()
            .map_err(|error| format!("Could not finish clipboard write: {error}"))?;
        if status.success() {
            Ok(())
        } else {
            Err("pbcopy failed to update the clipboard.".into())
        }
    }

    #[cfg(target_os = "linux")]
    {
        linux_write_clipboard(text)
    }

    #[cfg(target_os = "windows")]
    {
        arboard::Clipboard::new()
            .map_err(|error| format!("Could not open clipboard: {error}"))?
            .set_text(text)
            .map_err(|error| format!("Could not write to clipboard: {error}"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = text;
        Err("Clipboard insertion is not implemented on this platform.".into())
    }
}

#[cfg(target_os = "macos")]
struct MacosPasteTiming {
    restored: bool,
    visible_ms: u64,
    clipboard_restore_ms: Option<u64>,
}

pub fn focus_captured_process(process_id: i32) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        macos_ax::focus_process(process_id)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = process_id;
        Err("Use Copy and paste into your destination app.".into())
    }
}

#[cfg(target_os = "macos")]
fn paste_via_clipboard_restoring(
    text: &str,
    restore: bool,
    snapshot: Option<&str>,
) -> Result<MacosPasteTiming, String> {
    let operation_started = std::time::Instant::now();
    let snapshot_before_write = if restore {
        match macos_clipboard_snapshot() {
            Ok(snapshot) => Some(snapshot),
            Err(error) => {
                tracing::warn!(error = %error, "could not snapshot clipboard before dictation paste");
                None
            }
        }
    } else {
        None
    };

    let _ = snapshot;
    write_clipboard(text)?;
    let after_write_change_count = macos_clipboard_change_count().ok();
    simulate_macos_paste()?;
    let visible_ms = operation_started.elapsed().as_millis() as u64;

    let Some(snapshot) = snapshot_before_write else {
        return Ok(MacosPasteTiming {
            restored: false,
            visible_ms,
            clipboard_restore_ms: None,
        });
    };
    let restore_started = std::time::Instant::now();
    match restore_macos_clipboard_after_paste(snapshot, after_write_change_count) {
        Ok(restored) => Ok(MacosPasteTiming {
            restored,
            visible_ms,
            clipboard_restore_ms: Some(restore_started.elapsed().as_millis() as u64),
        }),
        Err(error) => {
            // The paste event has already been accepted at this point. Clipboard
            // cleanup is deliberately secondary: a restore failure must not turn
            // a delivered paste into a false copy-only result.
            tracing::warn!(error = %error, "dictation pasted but clipboard restore failed");
            Ok(MacosPasteTiming {
                restored: false,
                visible_ms,
                clipboard_restore_ms: Some(restore_started.elapsed().as_millis() as u64),
            })
        }
    }
}

#[cfg(target_os = "macos")]
fn verify_paste_target(
    expected: Option<&ActiveTargetContext>,
    actual: Option<&ActiveTargetContext>,
) -> Result<(), String> {
    if let Some(expected_process_id) = expected.and_then(|context| context.process_id) {
        let actual_process_id = actual
            .and_then(|context| context.process_id)
            .ok_or_else(|| "app changed, text is on the clipboard".to_string())?;
        if actual_process_id != expected_process_id {
            return Err("app changed, text is on the clipboard".into());
        }
    }

    let Some(expected_bundle_id) = expected
        .and_then(|context| context.bundle_id.as_deref())
        .filter(|value| !value.is_empty())
    else {
        return Ok(());
    };

    let actual_bundle_id = actual
        .and_then(|context| context.bundle_id.as_deref())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "app changed, text is on the clipboard".to_string())?;

    if actual_bundle_id == expected_bundle_id {
        Ok(())
    } else {
        Err("app changed, text is on the clipboard".into())
    }
}

#[cfg(target_os = "macos")]
fn restore_macos_clipboard_after_paste(
    snapshot: ClipboardSnapshot,
    after_write_change_count: Option<i64>,
) -> Result<bool, String> {
    let Some(after_write_change_count) = after_write_change_count else {
        tracing::warn!(
            "skipping dictation clipboard restore because changeCount after write was unavailable"
        );
        return Ok(false);
    };

    wait_for_clipboard_restore_window(after_write_change_count);
    let current_change_count = macos_clipboard_change_count()?;
    if current_change_count != after_write_change_count {
        tracing::info!(
            after_write_change_count,
            current_change_count,
            "skipping dictation clipboard restore because another app changed the clipboard"
        );
        return Ok(false);
    }

    if let Some(items) = snapshot.items {
        return macos_pasteboard::restore(items, after_write_change_count);
    }
    // Never replace rich clipboard contents with a partial text-only snapshot.
    if snapshot.has_plain_text
        && snapshot.types.iter().all(|ty| {
            matches!(
                ty.as_str(),
                "public.utf8-plain-text" | "NSStringPboardType" | "public.text"
            )
        })
    {
        if let Some(text) = snapshot.text {
            write_clipboard(&text)?;
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(target_os = "macos")]
fn wait_for_clipboard_restore_window(after_write_change_count: i64) {
    let max_wait = Duration::from_millis(
        minutes_core::config::Config::load()
            .dictation
            .experience
            .clipboard_restore_delay_ms
            .clamp(250, 5000),
    );
    const STEP: Duration = Duration::from_millis(25);
    let started = std::time::Instant::now();
    while started.elapsed() < max_wait {
        std::thread::sleep(STEP);
        match macos_clipboard_change_count() {
            Ok(current) if current != after_write_change_count => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_clipboard_change_count() -> Result<i64, String> {
    macos_pasteboard::change_count()
}

#[cfg(target_os = "macos")]
fn macos_clipboard_snapshot() -> Result<ClipboardSnapshot, String> {
    macos_pasteboard::snapshot()
}

#[cfg(target_os = "macos")]
fn simulate_macos_paste() -> Result<(), String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(r#"tell application "System Events" to keystroke "v" using command down"#)
        .output()
        .map_err(|error| format!("Could not run paste automation: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.trim().is_empty() {
            "Paste automation failed.".into()
        } else {
            format!("Paste automation failed: {}", stderr.trim())
        })
    }
}

#[cfg(target_os = "linux")]
fn linux_read_clipboard() -> Result<String, String> {
    let candidates = linux_clipboard_read_candidates();
    let mut errors = Vec::new();

    for (program, args) in candidates {
        if !linux_command_available(program) {
            continue;
        }

        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("Could not start {program}: {error}"))?;
        if output.status.success() {
            return Ok(String::from_utf8_lossy(&output.stdout).to_string());
        }

        let stderr = String::from_utf8_lossy(&output.stderr);
        errors.push(format!(
            "{program} failed{}",
            if stderr.trim().is_empty() {
                String::new()
            } else {
                format!(": {}", stderr.trim())
            }
        ));
    }

    if errors.is_empty() {
        Err(linux_clipboard_tools_message("read"))
    } else {
        Err(format!("Could not read clipboard: {}", errors.join("; ")))
    }
}

#[cfg(target_os = "linux")]
fn linux_write_clipboard(text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let candidates = linux_clipboard_write_candidates();
    let mut errors = Vec::new();

    for (program, args) in candidates {
        if !linux_command_available(program) {
            continue;
        }

        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|error| format!("Could not start {program}: {error}"))?;

        if let Some(mut stdin) = child.stdin.take() {
            if let Err(error) = stdin.write_all(text.as_bytes()) {
                let _ = child.wait();
                errors.push(format!("could not write to {program}: {error}"));
                continue;
            }
        }

        match child.wait() {
            Ok(status) if status.success() => return Ok(()),
            Ok(_) => errors.push(format!("{program} failed to update the clipboard")),
            Err(error) => errors.push(format!("could not finish {program}: {error}")),
        }
    }

    if errors.is_empty() {
        Err(linux_clipboard_tools_message("update"))
    } else {
        Err(format!("Could not update clipboard: {}", errors.join("; ")))
    }
}

#[cfg(target_os = "linux")]
fn linux_clipboard_read_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut candidates = Vec::new();
    if linux_wayland_session() {
        candidates.push(("wl-paste", vec!["--no-newline"]));
    }
    if linux_x11_session() {
        candidates.push(("xclip", vec!["-selection", "clipboard", "-out"]));
        candidates.push(("xsel", vec!["--clipboard", "--output"]));
    }
    candidates
}

#[cfg(target_os = "linux")]
fn linux_clipboard_write_candidates() -> Vec<(&'static str, Vec<&'static str>)> {
    let mut candidates = Vec::new();
    if linux_wayland_session() {
        candidates.push(("wl-copy", Vec::new()));
    }
    if linux_x11_session() {
        candidates.push(("xclip", vec!["-selection", "clipboard"]));
        candidates.push(("xsel", vec!["--clipboard", "--input"]));
    }
    candidates
}

#[cfg(target_os = "linux")]
fn linux_x11_paste_available() -> bool {
    linux_pure_x11_session() && linux_command_available("xdotool")
}

#[cfg(target_os = "linux")]
fn paste_via_xdotool() -> Result<(), String> {
    let output = std::process::Command::new("xdotool")
        .args(["key", "--clearmodifiers", "ctrl+v"])
        .output()
        .map_err(|error| format!("Could not start xdotool: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.trim().is_empty() {
            "xdotool paste automation failed.".into()
        } else {
            format!("xdotool paste automation failed: {}", stderr.trim())
        })
    }
}

#[cfg(target_os = "linux")]
fn linux_copy_fallback_message() -> String {
    if linux_wayland_session() {
        "Copied dictation to the clipboard. Wayland does not expose one universal paste automation path; paste manually.".into()
    } else if linux_x11_session() {
        "Copied dictation to the clipboard. Install xdotool to let Minutes paste into the focused X11 app.".into()
    } else {
        "Copied dictation to the clipboard. No supported Linux paste automation target was detected.".into()
    }
}

#[cfg(target_os = "linux")]
fn linux_clipboard_tools_message(action: &str) -> String {
    format!(
        "Could not {action} clipboard. Install wl-clipboard for Wayland or xclip/xsel for X11, then run Minutes inside that desktop session."
    )
}

#[cfg(target_os = "linux")]
fn linux_wayland_session() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some()
}

#[cfg(target_os = "linux")]
fn linux_x11_session() -> bool {
    std::env::var_os("DISPLAY").is_some()
}

#[cfg(target_os = "linux")]
fn linux_pure_x11_session() -> bool {
    linux_x11_session() && !linux_wayland_session()
}

#[cfg(target_os = "linux")]
fn linux_command_available(program: &str) -> bool {
    which::which(program).is_ok()
}

fn capture_target_context() -> Option<ActiveTargetContext> {
    #[cfg(target_os = "macos")]
    {
        if let Ok(Some(identity)) = minutes_core::desktop_context::frontmost_app_identity() {
            return Some(ActiveTargetContext {
                platform: "macos".into(),
                app_name: identity.app_name,
                bundle_id: identity.bundle_id,
                process_id: (identity.process_id > 0).then_some(identity.process_id),
            });
        }

        let app_name = frontmost_app_name().ok();
        let bundle_id = frontmost_app_bundle_id().ok();
        let process_id = frontmost_app_process_id().ok();
        Some(ActiveTargetContext {
            platform: "macos".into(),
            app_name,
            bundle_id,
            process_id,
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        Some(ActiveTargetContext {
            platform: std::env::consts::OS.into(),
            app_name: None,
            bundle_id: None,
            process_id: None,
        })
    }
}

#[cfg(target_os = "macos")]
fn frontmost_app_name() -> Result<String, String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(
            r#"tell application "System Events" to get name of first application process whose frontmost is true"#,
        )
        .output()
        .map_err(|error| format!("Could not query frontmost app: {error}"))?;
    if !output.status.success() {
        return Err("Could not query frontmost app.".into());
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        Err("Frontmost app query returned no app.".into())
    } else {
        Ok(name)
    }
}

#[cfg(target_os = "macos")]
fn frontmost_app_bundle_id() -> Result<String, String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(
            r#"tell application "System Events" to get bundle identifier of first application process whose frontmost is true"#,
        )
        .output()
        .map_err(|error| format!("Could not query frontmost app bundle id: {error}"))?;
    if !output.status.success() {
        return Err("Could not query frontmost app bundle id.".into());
    }
    let bundle_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if bundle_id.is_empty() {
        Err("Frontmost app query returned no bundle id.".into())
    } else {
        Ok(bundle_id)
    }
}

#[cfg(target_os = "macos")]
fn frontmost_app_process_id() -> Result<i32, String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(
            r#"tell application "System Events" to get unix id of first application process whose frontmost is true"#,
        )
        .output()
        .map_err(|error| format!("Could not query frontmost app process id: {error}"))?;
    if !output.status.success() {
        return Err("Could not query frontmost app process id.".into());
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<i32>()
        .ok()
        .filter(|pid| *pid > 0)
        .ok_or_else(|| "Frontmost app query returned no process id.".into())
}

#[cfg(target_os = "macos")]
fn restore_macos_target_focus(context: &ActiveTargetContext) -> Result<(), String> {
    if let Some(process_id) = context.process_id.filter(|pid| *pid > 0) {
        return macos_ax::focus_process(process_id);
    }

    if let Some(bundle_id) = context
        .bundle_id
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return run_macos_focus_script(
            r#"on run argv
  set targetId to item 1 of argv
  tell application "System Events"
    set frontmost of first application process whose bundle identifier is targetId to true
  end tell
end run"#,
            bundle_id,
        );
    }

    if let Some(app_name) = context
        .app_name
        .as_deref()
        .filter(|value| !value.is_empty())
    {
        return run_macos_focus_script(
            r#"on run argv
  set targetName to item 1 of argv
  tell application "System Events"
    set frontmost of first application process whose name is targetName to true
  end tell
end run"#,
            app_name,
        );
    }

    Err("No target app was captured before dictation.".into())
}

#[cfg(target_os = "macos")]
fn run_macos_focus_script(script: &str, target: &str) -> Result<(), String> {
    let output = std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .arg(target)
        .output()
        .map_err(|error| format!("Could not restore focus to dictation target: {error}"))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(if stderr.trim().is_empty() {
            "Could not restore focus to dictation target.".into()
        } else {
            format!(
                "Could not restore focus to dictation target: {}",
                stderr.trim()
            )
        })
    }
}

#[cfg(target_os = "macos")]
fn focused_ax_value() -> Result<String, String> {
    macos_ax::focused_value()
}

#[cfg(target_os = "macos")]
mod macos_pasteboard {
    use super::ClipboardSnapshot;
    use std::ffi::{c_char, c_void, CStr, CString};

    type Id = *mut c_void;
    type Sel = *mut c_void;
    type Class = *mut c_void;
    type NSInteger = isize;
    type NSUInteger = usize;

    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> Class;
        fn sel_registerName(name: *const c_char) -> Sel;
        fn objc_msgSend();
    }

    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}

    const NS_UTF8_STRING_ENCODING: NSUInteger = 4;

    pub fn change_count() -> Result<i64, String> {
        unsafe { Ok(msg_send_id_isize(general_pasteboard()?, sel("changeCount")?) as i64) }
    }
    struct Pool(Id);
    impl Pool {
        unsafe fn new() -> Result<Self, String> {
            Ok(Self(msg_send_class_id(
                class("NSAutoreleasePool")?,
                sel("new")?,
            )))
        }
    }
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe {
                if let Ok(selector) = sel("drain") {
                    let call: unsafe extern "C" fn(Id, Sel) =
                        std::mem::transmute(objc_msgSend as *const ());
                    call(self.0, selector);
                }
            }
        }
    }
    pub fn snapshot() -> Result<ClipboardSnapshot, String> {
        unsafe {
            let _pool = Pool::new()?;
            snapshot_from(general_pasteboard()?)
        }
    }
    unsafe fn snapshot_from(pasteboard: Id) -> Result<ClipboardSnapshot, String> {
        let change_count = msg_send_id_isize(pasteboard, sel("changeCount")?) as i64;
        let types = read_types(pasteboard)?;
        let has_plain_text = types.iter().any(|ty| {
            ty == "public.utf8-plain-text" || ty == "NSStringPboardType" || ty == "public.text"
        });
        let text = if has_plain_text {
            read_string_for_type(pasteboard, "public.utf8-plain-text")?
                .or(read_string_for_type(pasteboard, "NSStringPboardType")?)
                .or(read_string_for_type(pasteboard, "public.text")?)
        } else {
            None
        };
        let items = read_items(pasteboard)?;
        if msg_send_id_isize(pasteboard, sel("changeCount")?) as i64 != change_count {
            return Err("Clipboard changed while its formats were being read.".into());
        }
        Ok(ClipboardSnapshot {
            change_count,
            text,
            has_plain_text,
            types,
            items,
        })
    }

    type Items = Vec<std::collections::BTreeMap<String, Vec<u8>>>;
    unsafe fn read_items(pasteboard: Id) -> Result<Option<Items>, String> {
        let items = msg_send_id_id(pasteboard, sel("pasteboardItems")?);
        if items.is_null() {
            return Ok(Some(Vec::new()));
        }
        let count = msg_send_id_usize(items, sel("count")?);
        if count > 64 {
            return Ok(None);
        }
        let mut out = Vec::new();
        let mut total = 0usize;
        for index in 0..count {
            let item = msg_send_id_usize_id(items, sel("objectAtIndex:")?, index);
            let types = read_types(item)?;
            if types.len() > 32 {
                return Ok(None);
            }
            let mut captured = std::collections::BTreeMap::new();
            for ty in types {
                let data = msg_send_id_id_id(item, sel("dataForType:")?, nsstring(&ty)?);
                if data.is_null() {
                    return Ok(None);
                }
                let len = msg_send_id_usize(data, sel("length")?);
                total = total.saturating_add(len);
                if total > 16 * 1024 * 1024 {
                    return Ok(None);
                }
                let bytes = msg_send_id_ptr(data, sel("bytes")?);
                if len > 0 && bytes.is_null() {
                    return Ok(None);
                }
                let bytes = if len == 0 {
                    Vec::new()
                } else {
                    std::slice::from_raw_parts(bytes.cast::<u8>(), len).to_vec()
                };
                captured.insert(ty, bytes);
            }
            out.push(captured);
        }
        Ok(Some(out))
    }
    pub fn restore(items: Items, expected_count: i64) -> Result<bool, String> {
        unsafe {
            let _pool = Pool::new()?;
            restore_to(general_pasteboard()?, items, expected_count)
        }
    }
    unsafe fn restore_to(
        pasteboard: Id,
        items: Items,
        expected_count: i64,
    ) -> Result<bool, String> {
        let array = msg_send_class_id(class("NSMutableArray")?, sel("array")?);
        for item in items {
            let object = msg_send_class_id(class("NSPasteboardItem")?, sel("new")?);
            if object.is_null() {
                return Ok(false);
            }
            for (ty, bytes) in item {
                let data_fn: unsafe extern "C" fn(Class, Sel, *const u8, usize) -> Id =
                    std::mem::transmute(objc_msgSend as *const ());
                let data = data_fn(
                    class("NSData")?,
                    sel("dataWithBytes:length:")?,
                    bytes.as_ptr(),
                    bytes.len(),
                );
                let set_fn: unsafe extern "C" fn(Id, Sel, Id, Id) -> bool =
                    std::mem::transmute(objc_msgSend as *const ());
                if data.is_null() || !set_fn(object, sel("setData:forType:")?, data, nsstring(&ty)?)
                {
                    let release: unsafe extern "C" fn(Id, Sel) =
                        std::mem::transmute(objc_msgSend as *const ());
                    release(object, sel("release")?);
                    return Ok(false);
                }
            }
            let add: unsafe extern "C" fn(Id, Sel, Id) =
                std::mem::transmute(objc_msgSend as *const ());
            add(array, sel("addObject:")?, object);
            let release: unsafe extern "C" fn(Id, Sel) =
                std::mem::transmute(objc_msgSend as *const ());
            release(object, sel("release")?);
        }
        // Recheck immediately before mutation, after materializing all items.
        if msg_send_id_isize(pasteboard, sel("changeCount")?) as i64 != expected_count {
            return Ok(false);
        }
        let _ = msg_send_id_isize(pasteboard, sel("clearContents")?);
        let write_fn: unsafe extern "C" fn(Id, Sel, Id) -> bool =
            std::mem::transmute(objc_msgSend as *const ());
        if msg_send_id_usize(array, sel("count")?) == 0 {
            return Ok(true);
        }
        Ok(write_fn(pasteboard, sel("writeObjects:")?, array))
    }

    #[cfg(test)]
    mod clipboard_tests {
        use super::*;
        #[test]
        fn native_rich_roundtrip_preserves_formats_and_yields_to_new_copies() {
            unsafe {
                let _pool = Pool::new().unwrap();
                // Unique named board: never read or mutate the user's clipboard.
                let board = msg_send_class_id(
                    class("NSPasteboard").unwrap(),
                    sel("pasteboardWithUniqueName").unwrap(),
                );
                assert!(!board.is_null());
                let fixture = vec![std::collections::BTreeMap::from([
                    ("public.utf8-plain-text".into(), b"fixture words".to_vec()),
                    ("public.rtf".into(), b"{\\rtf1 fixture words}".to_vec()),
                    ("public.png".into(), vec![0, 1, 2, 255]),
                ])];
                let original_count = msg_send_id_isize(board, sel("changeCount").unwrap()) as i64;
                assert!(restore_to(board, fixture.clone(), original_count).unwrap());
                let saved = snapshot_from(board).unwrap();
                let captured = saved.items.clone().unwrap();
                for (format, bytes) in &fixture[0] {
                    assert_eq!(captured[0].get(format), Some(bytes));
                }
                // macOS can add equivalent UTF-16 representations. Preserve
                // every captured format, including those added by the OS.
                let inserted = vec![std::collections::BTreeMap::from([(
                    "public.utf8-plain-text".into(),
                    b"dictated words".to_vec(),
                )])];
                assert!(restore_to(board, inserted, saved.change_count).unwrap());
                let after_insert = snapshot_from(board).unwrap();
                assert!(restore_to(board, captured.clone(), after_insert.change_count).unwrap());
                let restored = snapshot_from(board).unwrap();
                assert_eq!(restored.items, Some(captured.clone()));
                let newer = vec![std::collections::BTreeMap::from([(
                    "public.utf8-plain-text".into(),
                    b"newer copy".to_vec(),
                )])];
                assert!(restore_to(board, newer.clone(), restored.change_count).unwrap());
                assert!(!restore_to(board, captured, restored.change_count).unwrap());
                let final_items = snapshot_from(board).unwrap().items.unwrap();
                assert_eq!(
                    final_items[0].get("public.utf8-plain-text"),
                    newer[0].get("public.utf8-plain-text")
                );
                assert!(!final_items[0].contains_key("public.rtf"));
                assert!(!final_items[0].contains_key("public.png"));
                let release: unsafe extern "C" fn(Id, Sel) =
                    std::mem::transmute(objc_msgSend as *const ());
                release(board, sel("releaseGlobally").unwrap());
            }
        }
    }

    unsafe fn general_pasteboard() -> Result<Id, String> {
        let class = class("NSPasteboard")?;
        let pasteboard = msg_send_class_id(class, sel("generalPasteboard")?);
        if pasteboard.is_null() {
            Err("NSPasteboard generalPasteboard returned nil".into())
        } else {
            Ok(pasteboard)
        }
    }

    unsafe fn read_types(pasteboard: Id) -> Result<Vec<String>, String> {
        let array = msg_send_id_id(pasteboard, sel("types")?);
        if array.is_null() {
            return Ok(Vec::new());
        }
        let count = msg_send_id_usize(array, sel("count")?);
        let object_at_index = sel("objectAtIndex:")?;
        let mut out = Vec::new();
        for index in 0..count {
            let value = msg_send_id_usize_id(array, object_at_index, index);
            if let Some(s) = nsstring_to_string(value) {
                out.push(s);
            }
        }
        Ok(out)
    }

    unsafe fn read_string_for_type(pasteboard: Id, ty: &str) -> Result<Option<String>, String> {
        let ns_type = nsstring(ty)?;
        let value = msg_send_id_id_id(pasteboard, sel("stringForType:")?, ns_type);
        Ok(nsstring_to_string(value))
    }

    unsafe fn nsstring(value: &str) -> Result<Id, String> {
        let class = class("NSString")?;
        let alloc = msg_send_class_id(class, sel("alloc")?);
        let c_string = CString::new(value)
            .map_err(|_| "NSString value contained an interior NUL byte".to_string())?;
        let ns = msg_send_id_ptr_usize_id(
            alloc,
            sel("initWithBytes:length:encoding:")?,
            c_string.as_ptr().cast(),
            value.len(),
            NS_UTF8_STRING_ENCODING,
        );
        if ns.is_null() {
            Err("Could not create NSString".into())
        } else {
            Ok(msg_send_id_id(ns, sel("autorelease")?))
        }
    }

    unsafe fn nsstring_to_string(value: Id) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let c_string = msg_send_id_ptr(value, sel("UTF8String").ok()?);
        if c_string.is_null() {
            return None;
        }
        CStr::from_ptr(c_string.cast())
            .to_str()
            .ok()
            .map(|s| s.to_string())
    }

    fn class(name: &str) -> Result<Class, String> {
        let c_name = CString::new(name)
            .map_err(|_| "Objective-C class name contained an interior NUL byte".to_string())?;
        let class = unsafe { objc_getClass(c_name.as_ptr()) };
        if class.is_null() {
            Err(format!("Objective-C class {name} was not found"))
        } else {
            Ok(class)
        }
    }

    fn sel(name: &str) -> Result<Sel, String> {
        let c_name = CString::new(name)
            .map_err(|_| "Objective-C selector contained an interior NUL byte".to_string())?;
        let selector = unsafe { sel_registerName(c_name.as_ptr()) };
        if selector.is_null() {
            Err(format!("Objective-C selector {name} was not found"))
        } else {
            Ok(selector)
        }
    }

    unsafe fn msg_send_class_id(receiver: Class, selector: Sel) -> Id {
        let f: unsafe extern "C" fn(Class, Sel) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector)
    }

    unsafe fn msg_send_id_id(receiver: Id, selector: Sel) -> Id {
        let f: unsafe extern "C" fn(Id, Sel) -> Id = std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector)
    }

    unsafe fn msg_send_id_isize(receiver: Id, selector: Sel) -> NSInteger {
        let f: unsafe extern "C" fn(Id, Sel) -> NSInteger =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector)
    }

    unsafe fn msg_send_id_usize(receiver: Id, selector: Sel) -> NSUInteger {
        let f: unsafe extern "C" fn(Id, Sel) -> NSUInteger =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector)
    }

    unsafe fn msg_send_id_usize_id(receiver: Id, selector: Sel, index: NSUInteger) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, NSUInteger) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector, index)
    }

    unsafe fn msg_send_id_id_id(receiver: Id, selector: Sel, arg: Id) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, Id) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector, arg)
    }

    unsafe fn msg_send_id_ptr(receiver: Id, selector: Sel) -> *const c_char {
        let f: unsafe extern "C" fn(Id, Sel) -> *const c_char =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector)
    }

    unsafe fn msg_send_id_ptr_usize_id(
        receiver: Id,
        selector: Sel,
        bytes: *const c_void,
        len: NSUInteger,
        encoding: NSUInteger,
    ) -> Id {
        let f: unsafe extern "C" fn(Id, Sel, *const c_void, NSUInteger, NSUInteger) -> Id =
            std::mem::transmute(objc_msgSend as *const ());
        f(receiver, selector, bytes, len, encoding)
    }
}

#[cfg(target_os = "macos")]
mod macos_ax {
    use std::ffi::{c_char, c_void, CString};
    use std::ptr;

    type AXError = i32;
    type AXUIElementRef = *const c_void;
    type CFStringRef = *const c_void;
    type CFTypeRef = *const c_void;
    type CFAllocatorRef = *const c_void;
    type Boolean = u8;

    const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
        fn AXUIElementSetAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: CFTypeRef,
        ) -> AXError;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        #[link_name = "kCFBooleanTrue"]
        static K_CF_BOOLEAN_TRUE: CFTypeRef;
        fn CFStringCreateWithCString(
            alloc: CFAllocatorRef,
            c_str: *const c_char,
            encoding: u32,
        ) -> CFStringRef;
        fn CFStringGetCString(
            the_string: CFStringRef,
            buffer: *mut c_char,
            buffer_size: isize,
            encoding: u32,
        ) -> Boolean;
        fn CFRelease(cf: CFTypeRef);
    }

    pub fn focused_value() -> Result<String, String> {
        let focused = focused_element()?;
        let value = copy_string_attribute(focused.cast(), "AXValue");
        unsafe { CFRelease(focused) };
        value
    }

    pub fn focus_process(process_id: i32) -> Result<(), String> {
        let application = unsafe { AXUIElementCreateApplication(process_id) };
        if application.is_null() {
            return Err("Could not create target app accessibility element.".into());
        }
        let frontmost_attr = match cfstring("AXFrontmost") {
            Ok(attribute) => attribute,
            Err(error) => {
                unsafe { CFRelease(application) };
                return Err(error);
            }
        };
        let set_err =
            unsafe { AXUIElementSetAttributeValue(application, frontmost_attr, K_CF_BOOLEAN_TRUE) };
        unsafe {
            CFRelease(frontmost_attr);
            CFRelease(application);
        }
        if set_err == 0 {
            Ok(())
        } else {
            Err(format!(
                "Could not focus the captured app process (AX error {set_err})."
            ))
        }
    }

    pub fn insert_selected_text(text: &str) -> Result<(), String> {
        let focused = focused_element()?;
        let selected_text_attr = match cfstring("AXSelectedText") {
            Ok(attribute) => attribute,
            Err(error) => {
                unsafe { CFRelease(focused) };
                return Err(error);
            }
        };
        let value = match cfstring(text) {
            Ok(value) => value,
            Err(error) => {
                unsafe {
                    CFRelease(selected_text_attr);
                    CFRelease(focused);
                }
                return Err(error);
            }
        };
        let set_err = unsafe {
            AXUIElementSetAttributeValue(focused.cast(), selected_text_attr, value.cast())
        };
        unsafe {
            CFRelease(value);
            CFRelease(selected_text_attr);
            CFRelease(focused);
        }
        if set_err == 0 {
            Ok(())
        } else {
            Err(format!(
                "Focused control does not accept selected text (AX error {set_err})."
            ))
        }
    }

    fn focused_element() -> Result<AXUIElementRef, String> {
        let system = unsafe { AXUIElementCreateSystemWide() };
        if system.is_null() {
            return Err("Could not create system accessibility element.".into());
        }

        let focused_attr = match cfstring("AXFocusedUIElement") {
            Ok(attribute) => attribute,
            Err(error) => {
                unsafe { CFRelease(system) };
                return Err(error);
            }
        };
        let mut focused: CFTypeRef = ptr::null();
        let focused_err =
            unsafe { AXUIElementCopyAttributeValue(system, focused_attr, &mut focused) };
        unsafe {
            CFRelease(focused_attr);
            CFRelease(system);
        }
        if focused_err != 0 || focused.is_null() {
            if !focused.is_null() {
                unsafe { CFRelease(focused) };
            }
            return Err(format!(
                "Could not read focused accessibility element (AX error {focused_err})."
            ));
        }
        Ok(focused.cast())
    }

    fn copy_string_attribute(element: AXUIElementRef, name: &str) -> Result<String, String> {
        let attr = cfstring(name)?;
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe { AXUIElementCopyAttributeValue(element, attr, &mut value) };
        unsafe { CFRelease(attr) };
        if err != 0 || value.is_null() {
            return Err(format!(
                "Could not read AX attribute {name} (AX error {err})."
            ));
        }
        let string = cfstring_to_string(value.cast());
        unsafe { CFRelease(value) };
        string
    }

    fn cfstring(value: &str) -> Result<CFStringRef, String> {
        let c_string = CString::new(value)
            .map_err(|_| "Accessibility string contained an interior NUL byte.".to_string())?;
        let cf = unsafe {
            CFStringCreateWithCString(ptr::null(), c_string.as_ptr(), K_CF_STRING_ENCODING_UTF8)
        };
        if cf.is_null() {
            Err("Could not create CoreFoundation string.".into())
        } else {
            Ok(cf)
        }
    }

    fn cfstring_to_string(value: CFStringRef) -> Result<String, String> {
        let mut buffer = vec![0i8; 8192];
        let ok = unsafe {
            CFStringGetCString(
                value,
                buffer.as_mut_ptr(),
                buffer.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
            )
        };
        if ok == 0 {
            return Err("AX string value was not readable as UTF-8.".into());
        }
        let nul = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        let bytes = buffer[..nul]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        String::from_utf8(bytes).map_err(|error| format!("AX value was not UTF-8: {error}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_result_maps_to_overlay_state() {
        let result = TextInsertionResult {
            outcome: InsertOutcome::Typed,
            method: InsertMethod::ClipboardPaste,
            verified: true,
            clipboard_restored: true,
            target_context: None,
            message: String::new(),
            timing: TextInsertionTiming::default(),
        };
        assert_eq!(result.overlay_state(), "typed");
    }

    #[test]
    fn native_accessibility_method_has_an_honest_stable_label() {
        assert_eq!(InsertMethod::NativeAx.as_str(), "native_ax");
    }

    #[test]
    fn insertion_capability_matrix_is_honest_on_every_build_host() {
        let cases = [
            (
                ("macos", false, false, false),
                TextInsertionCapability::MacosNativeOrPaste,
            ),
            (
                ("windows", false, false, false),
                TextInsertionCapability::ClipboardOnly,
            ),
            (
                ("linux", false, true, true),
                TextInsertionCapability::LinuxX11Paste,
            ),
            (
                ("linux", false, true, false),
                TextInsertionCapability::ClipboardOnly,
            ),
            (
                ("linux", true, true, true),
                TextInsertionCapability::ClipboardOnly,
            ),
            (
                ("linux", true, false, false),
                TextInsertionCapability::ClipboardOnly,
            ),
            (
                ("linux", false, false, true),
                TextInsertionCapability::Unavailable,
            ),
            (
                ("freebsd", false, false, false),
                TextInsertionCapability::Unavailable,
            ),
        ];

        for ((platform, wayland, x11, xdotool), expected) in cases {
            let actual = insertion_capability_for(platform, wayland, x11, xdotool);
            assert_eq!(actual, expected, "platform={platform}");
        }
    }

    #[test]
    fn only_proven_capabilities_claim_active_app_insertion() {
        assert!(TextInsertionCapability::MacosNativeOrPaste.supports_active_app_insertion());
        assert!(TextInsertionCapability::LinuxX11Paste.supports_active_app_insertion());
        assert!(!TextInsertionCapability::ClipboardOnly.supports_active_app_insertion());
        assert!(!TextInsertionCapability::Unavailable.supports_active_app_insertion());
        assert_eq!(
            TextInsertionCapability::ClipboardOnly.as_str(),
            "clipboard_only"
        );
    }

    #[test]
    fn failed_insertion_maps_to_error_state() {
        let result = TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::Unsupported,
            verified: false,
            clipboard_restored: false,
            target_context: None,
            message: String::new(),
            timing: TextInsertionTiming::default(),
        };
        assert_eq!(result.overlay_state(), "error");
    }

    #[test]
    fn clipboard_paste_restore_orders_operations() {
        let operations = std::cell::RefCell::new(Vec::new());
        let restored = clipboard_paste_restore_with(
            "dictated text",
            true,
            Some("previous clipboard"),
            |text| {
                operations.borrow_mut().push(format!("write:{text}"));
                Ok(())
            },
            || {
                operations.borrow_mut().push("paste".into());
                Ok(())
            },
            || operations.borrow_mut().push("wait".into()),
        )
        .expect("clipboard flow should succeed");

        assert!(restored);
        assert_eq!(
            operations.into_inner(),
            vec![
                "write:dictated text",
                "paste",
                "wait",
                "write:previous clipboard"
            ]
        );
    }

    #[test]
    fn clipboard_paste_restore_skips_restore_without_snapshot() {
        let operations = std::cell::RefCell::new(Vec::new());
        let restored = clipboard_paste_restore_with(
            "dictated text",
            true,
            None,
            |text| {
                operations.borrow_mut().push(format!("write:{text}"));
                Ok(())
            },
            || {
                operations.borrow_mut().push("paste".into());
                Ok(())
            },
            || operations.borrow_mut().push("wait".into()),
        )
        .expect("clipboard flow should succeed");

        assert!(!restored);
        assert_eq!(
            operations.into_inner(),
            vec!["write:dictated text", "paste"]
        );
    }

    #[test]
    fn clipboard_paste_without_restore_leaves_dictation_on_clipboard() {
        let operations = std::cell::RefCell::new(Vec::new());
        let restored = clipboard_paste_restore_with(
            "dictated text",
            false,
            Some("previous clipboard"),
            |text| {
                operations.borrow_mut().push(format!("write:{text}"));
                Ok(())
            },
            || {
                operations.borrow_mut().push("paste".into());
                Ok(())
            },
            || operations.borrow_mut().push("wait".into()),
        )
        .expect("clipboard flow should succeed");

        assert!(!restored);
        assert_eq!(
            operations.into_inner(),
            vec!["write:dictated text", "paste"]
        );
    }

    #[test]
    fn terminal_targets_use_clipboard_paste_instead_of_native_ax() {
        for (app_name, bundle_id) in [
            ("Ghostty", "com.mitchellh.ghostty"),
            ("Terminal", "com.apple.Terminal"),
            ("iTerm2", "com.googlecode.iterm2"),
            ("Warp", "dev.warp.Warp-Stable"),
            ("Minutes", "com.useminutes.desktop"),
            ("Minutes Dev", "com.useminutes.desktop.dev"),
        ] {
            let target = ActiveTargetContext {
                platform: "macos".into(),
                app_name: Some(app_name.into()),
                bundle_id: Some(bundle_id.into()),
                process_id: Some(42),
            };
            assert!(target_prefers_clipboard_paste(Some(&target)), "{app_name}");
        }

        let notes = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("Notes".into()),
            bundle_id: Some("com.apple.Notes".into()),
            process_id: Some(42),
        };
        assert!(!target_prefers_clipboard_paste(Some(&notes)));
    }

    #[test]
    fn native_ax_delivery_requires_observed_text_change() {
        assert!(native_ax_delivery_verified(
            Some("before"),
            Some("before dictated text"),
            "dictated text"
        ));
        assert!(!native_ax_delivery_verified(
            Some("before"),
            Some("before"),
            "dictated text"
        ));
        assert!(!native_ax_delivery_verified(
            None,
            Some("dictated text"),
            "dictated text"
        ));
        assert!(!native_ax_delivery_verified(
            Some("before"),
            None,
            "dictated text"
        ));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_paste_restore_failure_is_secondary_to_delivery() {
        let source = include_str!("text_insertion.rs");
        assert!(source.contains("dictation pasted but clipboard restore failed"));
        assert!(source
            .contains("Err(error) => {\n            // The paste event has already been accepted"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn paste_target_verification_blocks_bundle_mismatch() {
        let expected = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("Notes".into()),
            bundle_id: Some("com.apple.Notes".into()),
            process_id: None,
        };
        let actual = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("Slack".into()),
            bundle_id: Some("com.tinyspeck.slackmacgap".into()),
            process_id: None,
        };

        let error = verify_paste_target(Some(&expected), Some(&actual)).unwrap_err();

        assert_eq!(error, "app changed, text is on the clipboard");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn paste_target_verification_allows_matching_bundle() {
        let expected = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("Notes".into()),
            bundle_id: Some("com.apple.Notes".into()),
            process_id: Some(42),
        };
        let actual = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("Notes".into()),
            bundle_id: Some("com.apple.Notes".into()),
            process_id: Some(42),
        };

        verify_paste_target(Some(&expected), Some(&actual)).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn paste_target_verification_blocks_matching_bundle_with_different_process() {
        let expected = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("TextEdit".into()),
            bundle_id: Some("com.apple.TextEdit".into()),
            process_id: Some(42),
        };
        let actual = ActiveTargetContext {
            platform: "macos".into(),
            app_name: Some("TextEdit".into()),
            bundle_id: Some("com.apple.TextEdit".into()),
            process_id: Some(43),
        };

        let error = verify_paste_target(Some(&expected), Some(&actual)).unwrap_err();

        assert_eq!(error, "app changed, text is on the clipboard");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_attempts_system_events_paste_without_ax_trust_preflight() {
        assert!(can_insert_into_apps());
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn windows_clipboard_round_trip() {
        let text = "minutes-clipboard-test-windows";
        write_clipboard(text).expect("write_clipboard should succeed on Windows");
        let got = read_clipboard().expect("read_clipboard should succeed on Windows");
        assert_eq!(got, text);
    }
}
