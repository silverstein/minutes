use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TextInsertionRequest {
    pub text: String,
    pub mode: TextInsertionMode,
    pub restore_clipboard: bool,
    pub clipboard_snapshot: Option<String>,
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
    ClipboardOnly,
    ClipboardPaste,
    Unsupported,
}

impl InsertMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            InsertMethod::ClipboardOnly => "clipboard_only",
            InsertMethod::ClipboardPaste => "clipboard_paste",
            InsertMethod::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveTargetContext {
    pub platform: String,
    pub app_name: Option<String>,
    pub bundle_id: Option<String>,
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

fn copy_only(text: &str, target_context: Option<ActiveTargetContext>) -> TextInsertionResult {
    match write_clipboard(text) {
        Ok(()) => TextInsertionResult {
            outcome: InsertOutcome::Copied,
            method: InsertMethod::ClipboardOnly,
            verified: true,
            clipboard_restored: false,
            target_context,
            message: "Copied dictation to the clipboard.".into(),
        },
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
        },
    }
}

#[cfg(target_os = "macos")]
fn best_effort_verified(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
) -> TextInsertionResult {
    if !minutes_core::hotkey_macos::is_accessibility_trusted() {
        return copy_after_block(request, target_context, "Accessibility permission is required to type into the active app. Copied dictation instead.");
    }

    let before_value = focused_ax_value().ok();

    match paste_via_clipboard(&request.text) {
        Ok(()) => {
            let verified = focused_ax_value().ok().is_some_and(|after| {
                before_value.as_ref() != Some(&after) && after.contains(&request.text)
            });
            let restored = restore_clipboard_if_requested(
                request.restore_clipboard,
                request.clipboard_snapshot.as_deref(),
            );
            TextInsertionResult {
                outcome: if verified {
                    InsertOutcome::Typed
                } else {
                    InsertOutcome::Pasted
                },
                method: InsertMethod::ClipboardPaste,
                verified,
                clipboard_restored: restored,
                target_context,
                message: if verified {
                    "Typed dictation into the active app.".into()
                } else {
                    "Pasted dictation into the active app.".into()
                },
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "dictation paste automation failed");
            TextInsertionResult {
                outcome: InsertOutcome::Copied,
                method: InsertMethod::ClipboardOnly,
                verified: true,
                clipboard_restored: false,
                target_context,
                message: "Could not type into the active app. Copied dictation instead.".into(),
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
            }
        }
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
        },
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn best_effort_verified(
    request: TextInsertionRequest,
    target_context: Option<ActiveTargetContext>,
) -> TextInsertionResult {
    copy_after_block(
        request,
        target_context,
        "Typing into apps is not implemented on this platform. Copied dictation instead.",
    )
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
        },
        Err(error) => TextInsertionResult {
            outcome: InsertOutcome::Failed,
            method: InsertMethod::ClipboardOnly,
            verified: false,
            clipboard_restored: false,
            target_context,
            message: error,
        },
    }
}

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

fn write_clipboard(text: &str) -> Result<(), String> {
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
fn paste_via_clipboard(text: &str) -> Result<(), String> {
    write_clipboard(text)?;
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
            });
        }

        let app_name = frontmost_app_name().ok();
        let bundle_id = frontmost_app_bundle_id().ok();
        Some(ActiveTargetContext {
            platform: "macos".into(),
            app_name,
            bundle_id,
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        Some(ActiveTargetContext {
            platform: std::env::consts::OS.into(),
            app_name: None,
            bundle_id: None,
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
fn restore_macos_target_focus(context: &ActiveTargetContext) -> Result<(), String> {
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
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(
            element: AXUIElementRef,
            attribute: CFStringRef,
            value: *mut CFTypeRef,
        ) -> AXError;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
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
        let system = unsafe { AXUIElementCreateSystemWide() };
        if system.is_null() {
            return Err("Could not create system accessibility element.".into());
        }

        let focused_attr = cfstring("AXFocusedUIElement")?;
        let mut focused: CFTypeRef = ptr::null();
        let focused_err =
            unsafe { AXUIElementCopyAttributeValue(system, focused_attr, &mut focused) };
        unsafe { CFRelease(focused_attr) };
        if focused_err != 0 || focused.is_null() {
            return Err(format!(
                "Could not read focused accessibility element (AX error {focused_err})."
            ));
        }

        let value = copy_string_attribute(focused.cast(), "AXValue");
        unsafe { CFRelease(focused) };
        value
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
        };
        assert_eq!(result.overlay_state(), "typed");
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
        };
        assert_eq!(result.overlay_state(), "error");
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
