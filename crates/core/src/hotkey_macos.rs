use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ──────────────────────────────────────────────────────────────
// Native macOS hotkey via CGEventTap.
//
// Captures low-level key events (Caps Lock, fn, etc.) that
// Tauri's global shortcut system cannot intercept. Runs on a
// dedicated background thread with a CFRunLoop.
//
// Architecture:
//   CGEventTapCreate(kCGHIDEventTap)
//        │
//        ▼
//   event_callback(type, keycode)
//        │
//        ├─ target keycode? → consume event, call handler
//        └─ other key → pass through
//
// Permission: requires Input Monitoring.
// Accessibility trust is not a reliable proxy for this permission.
// ──────────────────────────────────────────────────────────────

/// Well-known key codes for dictation hotkeys.
pub const KEYCODE_CAPS_LOCK: i64 = 57;
pub const KEYCODE_FN: i64 = 63;

// ── Core Foundation / Core Graphics FFI ──────────────────────

#[allow(non_upper_case_globals)]
mod ffi {
    use std::ffi::c_void;

    pub type CFMachPortRef = *mut c_void;
    pub type CFRunLoopSourceRef = *mut c_void;
    pub type CFRunLoopRef = *mut c_void;
    pub type CFAllocatorRef = *const c_void;
    pub type CFDictionaryRef = *const c_void;
    pub type CFStringRef = *const c_void;
    pub type CFRunLoopMode = CFStringRef;
    pub type CGEventRef = *mut c_void;
    pub type CGEventTapProxy = *mut c_void;
    pub type CGEventType = u32;

    // CGEventTap constants
    pub const kCGHIDEventTap: u32 = 0;
    pub const kCGHeadInsertEventTap: u32 = 0;
    pub const kCGEventTapOptionDefault: u32 = 0;
    pub const kCGEventKeyDown: u32 = 10;
    pub const kCGEventKeyUp: u32 = 11;
    pub const kCGEventFlagsChanged: u32 = 12;
    pub const kCGKeyboardEventKeycode: u32 = 9;

    // CFRunLoop result codes
    pub const kCFRunLoopRunFinished: i32 = 1;

    pub type CGEventTapCallBack = unsafe extern "C" fn(
        proxy: CGEventTapProxy,
        event_type: CGEventType,
        event: CGEventRef,
        user_info: *mut c_void,
    ) -> CGEventRef;

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {}

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {}

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {}

    extern "C" {
        pub static kCFAllocatorDefault: CFAllocatorRef;
        pub static kCFRunLoopCommonModes: CFRunLoopMode;
        pub static kCFRunLoopDefaultMode: CFRunLoopMode;

        pub fn CGEventTapCreate(
            tap: u32,
            place: u32,
            options: u32,
            events_of_interest: u64,
            callback: CGEventTapCallBack,
            user_info: *mut c_void,
        ) -> CFMachPortRef;

        pub fn CGEventTapEnable(tap: CFMachPortRef, enable: bool);
        pub fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;

        pub fn CFMachPortCreateRunLoopSource(
            allocator: CFAllocatorRef,
            port: CFMachPortRef,
            order: i64,
        ) -> CFRunLoopSourceRef;

        pub fn CGEventGetIntegerValueField(event: CGEventRef, field: u32) -> i64;

        pub fn CFRunLoopGetCurrent() -> CFRunLoopRef;
        pub fn CFRunLoopAddSource(
            rl: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFRunLoopMode,
        );
        pub fn CFRunLoopRemoveSource(
            rl: CFRunLoopRef,
            source: CFRunLoopSourceRef,
            mode: CFRunLoopMode,
        );
        pub fn CFRunLoopRunInMode(mode: CFRunLoopMode, seconds: f64, return_after: bool) -> i32;

        pub fn CFRelease(cf: *const c_void);

        // Input Monitoring permission (correct API for CGEventTap)
        pub fn CGPreflightListenEventAccess() -> bool;
        pub fn CGRequestListenEventAccess() -> bool;

        // Accessibility permission (for reference, NOT what CGEventTap needs)
        pub fn AXIsProcessTrustedWithOptions(options: CFDictionaryRef) -> bool;
    }
}

/// Check if Input Monitoring permission is granted (what CGEventTap actually needs).
pub fn is_input_monitoring_granted() -> bool {
    unsafe { ffi::CGPreflightListenEventAccess() }
}

/// Request Input Monitoring permission. Shows the system prompt if not yet decided.
pub fn request_input_monitoring() -> bool {
    unsafe { ffi::CGRequestListenEventAccess() }
}

/// Open System Settings to the Input Monitoring pane.
pub fn open_input_monitoring_settings() {
    let _ = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
        .spawn();
}

/// Check if Accessibility permission is granted (NOT needed for CGEventTap,
/// but kept for other uses like AppleScript automation).
pub fn is_accessibility_trusted() -> bool {
    unsafe { ffi::AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

/// Prompt the user for Accessibility permission (legacy, prefer open_input_monitoring_settings
/// for hotkey-related flows).
pub fn prompt_accessibility_permission() {
    open_input_monitoring_settings();
}

/// Events emitted by the native hotkey monitor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyEvent {
    Press,
    Release,
}

/// Lifecycle updates emitted by the native hotkey monitor thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotkeyMonitorStatus {
    Starting,
    Active,
    Failed(String),
    Stopped,
}

/// Handle to a running hotkey monitor. Drop to stop monitoring.
pub struct HotkeyMonitor {
    stop: Arc<AtomicBool>,
    _thread: Option<std::thread::JoinHandle<()>>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct HotkeyProbeResult {
    pub keycode: i64,
    pub accessibility_trusted: bool,
    pub status: String,
    pub message: String,
    pub elapsed_ms: u128,
}

impl HotkeyMonitor {
    /// Start monitoring a specific keycode for press/release events.
    ///
    /// `keycode`: the macOS virtual key code to monitor (e.g., 57 for Caps Lock).
    /// `callback`: called on the monitoring thread when the key is pressed or released.
    ///
    /// Returns an error only when the monitor thread cannot be spawned.
    ///
    /// Startup success or permission failures are reported asynchronously through
    /// `status_callback`, which keeps the caller off the UI thread.
    pub fn start<F, S>(keycode: i64, callback: F, status_callback: S) -> Result<Self, String>
    where
        F: Fn(HotkeyEvent) + Send + 'static,
        S: Fn(HotkeyMonitorStatus) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);

        let boxed_callback: Box<dyn Fn(HotkeyEvent) + Send> = Box::new(callback);
        let boxed_status_callback: Box<dyn Fn(HotkeyMonitorStatus) + Send> =
            Box::new(status_callback);

        let thread = std::thread::Builder::new()
            .name("hotkey-monitor".into())
            .spawn(move || {
                run_event_tap(keycode, boxed_callback, boxed_status_callback, stop_clone);
            })
            .map_err(|err| format!("Could not spawn hotkey monitor: {}", err))?;

        Ok(HotkeyMonitor {
            stop,
            _thread: Some(thread),
        })
    }

    /// Stop the hotkey monitor.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Drop for HotkeyMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Attempt to start the native macOS hotkey monitor and report whether the
/// current process identity can create the CGEventTap successfully.
pub fn probe_hotkey_monitor(keycode: i64, timeout: Duration) -> HotkeyProbeResult {
    let accessibility_trusted = is_input_monitoring_granted();
    let started_at = Instant::now();
    let (tx, rx) = std::sync::mpsc::channel::<HotkeyMonitorStatus>();

    let monitor = match HotkeyMonitor::start(
        keycode,
        |_| {},
        move |status| {
            let _ = tx.send(status);
        },
    ) {
        Ok(monitor) => monitor,
        Err(error) => {
            return HotkeyProbeResult {
                keycode,
                accessibility_trusted,
                status: "spawn-failed".into(),
                message: error,
                elapsed_ms: started_at.elapsed().as_millis(),
            };
        }
    };

    let deadline = started_at + timeout;
    let mut result = HotkeyProbeResult {
        keycode,
        accessibility_trusted,
        status: "timeout".into(),
        message: format!(
            "Timed out after {}ms waiting for the native hotkey monitor to report status.",
            timeout.as_millis()
        ),
        elapsed_ms: timeout.as_millis(),
    };

    loop {
        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now);
        match rx.recv_timeout(remaining) {
            Ok(HotkeyMonitorStatus::Starting) => {
                result = HotkeyProbeResult {
                    keycode,
                    accessibility_trusted,
                    status: "starting".into(),
                    message: "Native hotkey monitor thread started and is waiting for CGEventTap activation.".into(),
                    elapsed_ms: started_at.elapsed().as_millis(),
                };
            }
            Ok(HotkeyMonitorStatus::Active) => {
                result = HotkeyProbeResult {
                    keycode,
                    accessibility_trusted,
                    status: "active".into(),
                    message: "CGEventTap started successfully for this process identity.".into(),
                    elapsed_ms: started_at.elapsed().as_millis(),
                };
                break;
            }
            Ok(HotkeyMonitorStatus::Failed(message)) => {
                result = HotkeyProbeResult {
                    keycode,
                    accessibility_trusted,
                    status: "failed".into(),
                    message,
                    elapsed_ms: started_at.elapsed().as_millis(),
                };
                break;
            }
            Ok(HotkeyMonitorStatus::Stopped) => {
                result = HotkeyProbeResult {
                    keycode,
                    accessibility_trusted,
                    status: "stopped".into(),
                    message: "Native hotkey monitor stopped before it reported active status."
                        .into(),
                    elapsed_ms: started_at.elapsed().as_millis(),
                };
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                break;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                result = HotkeyProbeResult {
                    keycode,
                    accessibility_trusted,
                    status: "disconnected".into(),
                    message: "Native hotkey monitor status channel disconnected unexpectedly."
                        .into(),
                    elapsed_ms: started_at.elapsed().as_millis(),
                };
                break;
            }
        }
    }

    monitor.stop();
    result.elapsed_ms = started_at.elapsed().as_millis();
    result
}

/// Context passed through the CGEventTap C callback via void* user_info.
struct TapContext {
    target_keycode: i64,
    callback: Box<dyn Fn(HotkeyEvent) + Send>,
    stop: Arc<AtomicBool>,
    key_is_down: AtomicBool,
}

fn run_event_tap(
    target_keycode: i64,
    callback: Box<dyn Fn(HotkeyEvent) + Send>,
    status_callback: Box<dyn Fn(HotkeyMonitorStatus) + Send>,
    stop: Arc<AtomicBool>,
) {
    // Event mask: keyDown + keyUp + flagsChanged (for modifier keys)
    let event_mask: u64 =
        (1 << ffi::kCGEventKeyDown) | (1 << ffi::kCGEventKeyUp) | (1 << ffi::kCGEventFlagsChanged);

    let context = Box::new(TapContext {
        target_keycode,
        callback,
        stop: Arc::clone(&stop),
        key_is_down: AtomicBool::new(false),
    });
    let context_ptr = Box::into_raw(context) as *mut std::ffi::c_void;

    status_callback(HotkeyMonitorStatus::Starting);

    unsafe {
        let tap = ffi::CGEventTapCreate(
            ffi::kCGHIDEventTap,
            ffi::kCGHeadInsertEventTap,
            ffi::kCGEventTapOptionDefault,
            event_mask,
            event_tap_callback,
            context_ptr,
        );

        if tap.is_null() {
            let message =
                "Could not start native hotkey. Enable Minutes in System Settings > Privacy & Security > Input Monitoring, then try again.";
            tracing::error!("{}", message);
            let _ = Box::from_raw(context_ptr as *mut TapContext);
            status_callback(HotkeyMonitorStatus::Failed(message.to_string()));
            return;
        }

        tracing::info!(keycode = target_keycode, "native hotkey monitor started");

        let source = ffi::CFMachPortCreateRunLoopSource(ffi::kCFAllocatorDefault, tap, 0);

        if source.is_null() {
            let message = "Could not start native hotkey run loop.";
            tracing::error!("{}", message);
            ffi::CFRelease(tap as *const std::ffi::c_void);
            let _ = Box::from_raw(context_ptr as *mut TapContext);
            status_callback(HotkeyMonitorStatus::Failed(message.to_string()));
            return;
        }

        let run_loop = ffi::CFRunLoopGetCurrent();
        ffi::CFRunLoopAddSource(run_loop, source, ffi::kCFRunLoopCommonModes);
        ffi::CGEventTapEnable(tap, true);
        status_callback(HotkeyMonitorStatus::Active);

        // Run in 0.5s intervals so we can check the stop flag and tap health
        while !stop.load(Ordering::Relaxed) {
            // Health check: macOS can silently disable the tap after code re-signing
            // or secure input activation. Re-enable if needed.
            if !ffi::CGEventTapIsEnabled(tap) {
                tracing::warn!(
                    keycode = target_keycode,
                    "CGEventTap was silently disabled, re-enabling"
                );
                ffi::CGEventTapEnable(tap, true);
            }

            let result = ffi::CFRunLoopRunInMode(ffi::kCFRunLoopDefaultMode, 0.5, false);
            if result == ffi::kCFRunLoopRunFinished {
                break;
            }
        }

        // Clean up
        ffi::CGEventTapEnable(tap, false);
        ffi::CFRunLoopRemoveSource(run_loop, source, ffi::kCFRunLoopCommonModes);
        ffi::CFRelease(source as *const std::ffi::c_void);
        ffi::CFRelease(tap as *const std::ffi::c_void);
        let _ = Box::from_raw(context_ptr as *mut TapContext);
    }

    tracing::info!("native hotkey monitor stopped");
    status_callback(HotkeyMonitorStatus::Stopped);
}

/// C callback for CGEventTap.
unsafe extern "C" fn event_tap_callback(
    _proxy: ffi::CGEventTapProxy,
    event_type: ffi::CGEventType,
    event: ffi::CGEventRef,
    user_info: *mut std::ffi::c_void,
) -> ffi::CGEventRef {
    let context = &*(user_info as *const TapContext);

    if context.stop.load(Ordering::Relaxed) {
        return event;
    }

    let keycode = ffi::CGEventGetIntegerValueField(event, ffi::kCGKeyboardEventKeycode);

    if keycode != context.target_keycode {
        return event; // Not our key — pass through
    }

    match event_type {
        ffi::kCGEventKeyDown => {
            if !context.key_is_down.swap(true, Ordering::Relaxed) {
                (context.callback)(HotkeyEvent::Press);
            }
            std::ptr::null_mut() // Consume
        }
        ffi::kCGEventKeyUp => {
            context.key_is_down.store(false, Ordering::Relaxed);
            (context.callback)(HotkeyEvent::Release);
            std::ptr::null_mut() // Consume
        }
        ffi::kCGEventFlagsChanged => {
            // Modifier keys (Caps Lock, fn) use FlagsChanged instead of keyDown/keyUp.
            // We track press state ourselves since FlagsChanged toggles.
            let was_down = context.key_is_down.load(Ordering::Relaxed);
            if was_down {
                context.key_is_down.store(false, Ordering::Relaxed);
                (context.callback)(HotkeyEvent::Release);
            } else {
                context.key_is_down.store(true, Ordering::Relaxed);
                (context.callback)(HotkeyEvent::Press);
            }
            std::ptr::null_mut() // Consume — prevent Caps Lock toggle
        }
        _ => event, // Unknown — pass through
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_monitoring_check_returns_bool() {
        let _ = is_input_monitoring_granted();
    }

    #[test]
    fn accessibility_check_returns_bool() {
        let _ = is_accessibility_trusted();
    }

    #[test]
    fn constants_are_correct() {
        assert_eq!(KEYCODE_CAPS_LOCK, 57);
        assert_eq!(KEYCODE_FN, 63);
    }
}
