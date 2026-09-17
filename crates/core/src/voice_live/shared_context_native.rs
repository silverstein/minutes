// Included in selection::native to reuse its bounded AX readers and ownership.
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool as ContextFlag, Ordering as ContextOrdering};
use std::sync::Arc as ContextArc;
use crossbeam_channel::{Receiver as ContextReceiver, Sender as ContextSender};
use super::super::shared_context::{Observations, Request as ContextRequest};

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXObserverCreate(pid: i32, callback: unsafe extern "C" fn(Ref, Ref, Ref, *mut c_void), observer: *mut Ref) -> i32;
    fn AXObserverAddNotification(observer: Ref, element: Ref, notification: Ref, context: *mut c_void) -> i32;
    fn AXObserverGetRunLoopSource(observer: Ref) -> *mut c_void;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRetain(value: Ref) -> Ref;
    fn CFArrayGetTypeID() -> usize;
    fn CFArrayGetCount(array: Ref) -> isize;
    fn CFArrayGetValueAtIndex(array: Ref, index: isize) -> Ref;
    fn CFRunLoopGetCurrent() -> *mut c_void;
    fn CFRunLoopAddSource(run_loop: *mut c_void, source: *mut c_void, mode: Ref);
    fn CFRunLoopRemoveSource(run_loop: *mut c_void, source: *mut c_void, mode: Ref);
    fn CFNumberGetTypeID() -> usize;
    fn CFNumberGetValue(number: Ref, kind: isize, value: *mut c_void) -> bool;
    fn CFRunLoopRunInMode(mode: Ref, seconds: f64, return_after_source: bool) -> i32;
    static kCFRunLoopDefaultMode: Ref;
    fn CFDictionaryGetTypeID() -> usize;
    fn CFDictionaryGetValue(dictionary: Ref, key: Ref) -> Ref;
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(options: u32, relative_to: u32) -> Ref;
    static kCGWindowOwnerPID: Ref;
    static kCGWindowName: Ref;
    static kCGWindowNumber: Ref;
}

unsafe fn window_number(pid: i32, title: &str) -> Result<u32, String> {
    let title = std::ffi::CString::new(title).map_err(|_| "Invalid window title")?;
    let title = owned(CFStringCreateWithCString(ptr::null(), title.as_ptr(), UTF8))?;
    let windows = owned(CGWindowListCopyWindowInfo(1 | 16, 0))?;
    if CFGetTypeID(windows.0) != CFArrayGetTypeID() || CFArrayGetCount(windows.0) > 2048 {
        return Err("Window identity budget exceeded; no screenshot taken.".into());
    }
    let number = |dictionary: Ref, key: Ref| -> Option<i64> {
        let value = CFDictionaryGetValue(dictionary, key);
        let mut result = 0i64;
        if !value.is_null() && CFGetTypeID(value) == CFNumberGetTypeID()
            && CFNumberGetValue(value, 4, std::ptr::from_mut(&mut result).cast()) { Some(result) } else { None }
    };
    let mut ids = Vec::new();
    for index in 0..CFArrayGetCount(windows.0) {
        let window = CFArrayGetValueAtIndex(windows.0, index);
        if window.is_null() || CFGetTypeID(window) != CFDictionaryGetTypeID()
            || number(window, kCGWindowOwnerPID) != Some(i64::from(pid)) { continue; }
        let name = CFDictionaryGetValue(window, kCGWindowName);
        if !name.is_null() && CFEqual(name, title.0) != 0 {
            if let Some(id) = number(window, kCGWindowNumber).and_then(|n|u32::try_from(n).ok()).filter(|n|*n>0) { ids.push(id); }
        }
    }
    if ids.len() == 1 { Ok(ids[0]) } else { Err("Cannot bind one exact visible window; no screenshot taken and no full-screen fallback.".into()) }
}

fn bounded_capture_command(command: &mut std::process::Command) -> Result<(), String> {
    let mut child = command.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|e|e.to_string())?;
    let began = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return if status.success() { Ok(()) } else { Err(format!("Window capture helper failed ({status})")) },
            Ok(None) if began.elapsed() < Duration::from_secs(1) => std::thread::sleep(Duration::from_millis(20)),
            result => {
                let _ = child.kill(); let _ = child.wait();
                return Err(format!("Window capture helper timed out or failed: {result:?}"));
            }
        }
    }
}

unsafe fn capture_context_window(app: &Owned, window: &Owned, pid: i32, title: &str, document: &str) -> Result<Vec<u8>, String> {
    if !crate::screen::check_screen_permission() { return Err("Screen Recording permission is unavailable; accessibility context remains available.".into()); }
    same_context(app, window, pid, title, document)?;
    let id = window_number(pid, title)?;
    let temp = tempfile::tempdir().map_err(|e|e.to_string())?;
    let path = temp.path().join("window.png");
    bounded_capture_command(crate::engine_process::command("/usr/sbin/screencapture").args(["-x","-o","-l",&id.to_string(),"-t","png"]).arg(&path))?;
    same_context(app, window, pid, title, document)?;
    if window_number(pid, title)? != id { return Err("Window identity changed during capture; image discarded.".into()); }
    bounded_capture_command(crate::engine_process::command("/usr/bin/sips").args(["--resampleWidth","1440"]).arg(&path))?;
    if std::fs::metadata(&path).map_err(|e|e.to_string())?.len() > 5_000_000 { return Err("Window image exceeds the sharing budget.".into()); }
    std::fs::read(path).map_err(|e|e.to_string())
}

unsafe extern "C" fn context_changed(_observer: Ref, _element: Ref, _notification: Ref, context: *mut c_void) {
    if !context.is_null() {
        unsafe { &*context.cast::<ContextFlag>() }.store(true, ContextOrdering::Release);
    }
}

struct ContextObserver {
    observer: Owned,
    source: *mut c_void,
    run_loop: *mut c_void,
}
impl Drop for ContextObserver {
    fn drop(&mut self) {
        unsafe { CFRunLoopRemoveSource(self.run_loop, self.source, kCFRunLoopDefaultMode); }
    }
}

unsafe fn subscribe_context(observer: &Owned, element: &Owned, name: &CStr, dirty: &mut ContextFlag) -> bool {
    let Ok(notification) = owned(CFStringCreateWithCString(ptr::null(), name.as_ptr(), UTF8)) else { return false; };
    matches!(AXObserverAddNotification(observer.0, element.0, notification.0, std::ptr::from_mut(dirty).cast()), 0 | -25209)
}

unsafe fn same_context(app: &Owned, window: &Owned, pid: i32, title: &str, document: &str) -> Result<(), String> {
    if NSWorkspace::sharedWorkspace().frontmostApplication().is_none_or(|a| a.processIdentifier() != pid) {
        return Err("Shared app lost focus. Sharing ended; explicitly share the desired window again.".into());
    }
    let current = element(app, c"AXFocusedWindow")?;
    if CFEqual(window.0, current.0) == 0
        || string(window, c"AXTitle")? != title
        || string(window, c"AXDocument").unwrap_or_default() != document
    {
        return Err("Shared window or document changed. Sharing ended; old references are invalid.".into());
    }
    Ok(())
}

unsafe fn context_snapshot(app: &Owned, window: &Owned, bundle: &str) -> Result<(Value, Vec<Owned>), String> {
    let began = Instant::now();
    let focused = element(app, c"AXFocusedUIElement")?;
    let role = string(&focused, c"AXRole")?;
    let subrole = string(&focused, c"AXSubrole").unwrap_or_default();
    if role == "AXSecureTextField" || subrole == "AXSecureTextField"
        || (role == "AXTextField" && subrole.is_empty()) {
        return Err("Secure or unverifiable focused field; no shared context captured.".into());
    }
    let selected: String = string(&focused, c"AXSelectedText").unwrap_or_default().chars().take(512).collect();
    let title = string(window, c"AXTitle")?;
    let document = string(window, c"AXDocument").unwrap_or_default();
    let mut queue = VecDeque::from([(owned(CFRetain(window.0))?, 0)]);
    let mut nodes = Vec::new();
    let mut elements = Vec::new();
    let mut remaining = 4096usize;
    while let Some((node, depth)) = queue.pop_front() {
        if nodes.len() >= 24 || began.elapsed() >= Duration::from_millis(800) || remaining < 512 { break; }
        let role = string(&node, c"AXRole").unwrap_or_default();
        let subrole = string(&node, c"AXSubrole").unwrap_or_default();
        if role == "AXSecureTextField" || subrole == "AXSecureTextField" || (role == "AXTextField" && subrole.is_empty()) { continue; }
        let label = string(&node, c"AXTitle").or_else(|_| string(&node, c"AXDescription")).unwrap_or_default();
        // Editable documents are represented by their selection, never their
        // entire AXValue. Static labels and scalar controls remain bounded.
        let value = if matches!(role.as_str(), "AXStaticText" | "AXSlider" | "AXCheckBox" | "AXPopUpButton") {
            string(&node, c"AXValue").unwrap_or_else(|_| {
                let Ok(value) = attribute(&node, c"AXValue") else { return String::new(); };
                if CFGetTypeID(value.0) == CFBooleanGetTypeID() {
                    return (CFBooleanGetValue(value.0) != 0).to_string();
                }
                let mut number = 0.0f64;
                if CFGetTypeID(value.0) == CFNumberGetTypeID()
                    && CFNumberGetValue(value.0, 13, std::ptr::from_mut(&mut number).cast())
                    && number.is_finite() { number.to_string() } else { String::new() }
            })
        } else { String::new() };
        let label: String = label.chars().take(80).collect();
        let value: String = value.chars().take(120).collect();
        remaining = remaining.saturating_sub(label.len() + value.len() + role.len());
        elements.push(json!({"role":role,"label":label,"value":value}));
        if depth < 7 {
            if let Ok(children) = attribute(&node, c"AXChildren") {
                if CFGetTypeID(children.0) == CFArrayGetTypeID() {
                    for index in 0..CFArrayGetCount(children.0).clamp(0, 32) {
                        if queue.len() + nodes.len() >= 64 { break; }
                        let child = CFArrayGetValueAtIndex(children.0, index);
                        if !child.is_null() && CFGetTypeID(child) == AXUIElementGetTypeID() {
                            queue.push_back((owned(CFRetain(child))?, depth + 1));
                        }
                    }
                }
            }
        }
        nodes.push(node);
    }
    nodes.push(focused);
    Ok((json!({"bundle_id":bundle,"window_title":title.chars().take(160).collect::<String>(),"document":document.chars().take(256).collect::<String>(),
        "focused_role":role,"selected_text":selected,"elements":elements,"bounded":true}), nodes))
}

pub fn observe_window(target: &str, duration: Duration, grant: u64, stop: ContextArc<ContextFlag>, requests: ContextReceiver<ContextRequest>, ready: ContextSender<Result<Value, String>>) {
    let started = Instant::now();
    let revoked = || stop.load(ContextOrdering::Acquire) || super::super::shared_context::generation() != grant;
    let result: Result<(), String> = autoreleasepool(|_| unsafe {
        if !AXIsProcessTrusted() { return Err("Accessibility permission is unavailable; no shared window started.".to_string()); }
        let application = NSWorkspace::sharedWorkspace().frontmostApplication().ok_or("No frontmost app")?;
        let bundle = application.bundleIdentifier().ok_or("App has no stable identity")?.to_string();
        let name = application.localizedName().map(|v| v.to_string()).unwrap_or_default();
        if bundle != target && !name.eq_ignore_ascii_case(target) { return Err("The named app must be frontmost before sharing its window.".into()); }
        let pid = application.processIdentifier();
        let app = owned(AXUIElementCreateApplication(pid))?;
        AXUIElementSetMessagingTimeout(app.0, 0.1);
        let window = element(&app, c"AXFocusedWindow")?;
        let title = string(&window, c"AXTitle")?;
        let document = string(&window, c"AXDocument").unwrap_or_default();
        let (initial, nodes) = context_snapshot(&app, &window, &bundle)?;
        same_context(&app, &window, pid, &title, &document)?;
        let mut dirty = Box::new(ContextFlag::new(false));
        let mut observer = ptr::null();
        let status = AXObserverCreate(pid, context_changed, &mut observer);
        if status != 0 { return Err(format!("Accessibility event observer unavailable ({status}); no ongoing sharing.")); }
        let observer = owned(observer)?;
        let source = AXObserverGetRunLoopSource(observer.0);
        if source.is_null() { return Err("Accessibility observer has no event source".into()); }
        let run_loop = CFRunLoopGetCurrent();
        CFRunLoopAddSource(run_loop, source, kCFRunLoopDefaultMode);
        let observer = ContextObserver { observer, source, run_loop };
        let mut subscribed = 0;
        for node in std::iter::once(&app).chain(std::iter::once(&window)).chain(nodes.iter()) {
            for notification in [c"AXFocusedWindowChanged", c"AXFocusedUIElementChanged", c"AXSelectedTextChanged", c"AXValueChanged", c"AXTitleChanged", c"AXUIElementDestroyed"] {
                if subscribe_context(&observer.observer, node, notification, &mut dirty) { subscribed += 1; }
            }
        }
        if subscribed == 0 { return Err("This app does not expose accessibility change notifications; no ongoing sharing.".into()); }
        same_context(&app, &window, pid, &title, &document)?;
        if revoked() || started.elapsed() >= duration {
            return Err("Sharing expired or was cancelled during setup; no context released.".into());
        }
        let mut observations = Observations::new(initial);
        ready.send(Ok(observations.receipt(duration.saturating_sub(started.elapsed()).as_secs()))).map_err(|_| "Shared-window request was cancelled")?;
        while !revoked() && started.elapsed() < duration {
            CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.1, true);
            if revoked() { break; }
            // Metadata-only target checks do not read other apps or their text.
            same_context(&app, &window, pid, &title, &document)?;
            if dirty.swap(false, ContextOrdering::AcqRel) {
                let (next, _) = context_snapshot(&app, &window, &bundle)?;
                same_context(&app, &window, pid, &title, &document)?;
                observations.update(next, "accessibility_event");
            }
            for request in requests.try_iter() {
                if revoked() || started.elapsed() >= duration { break; }
                match request {
                    ContextRequest::Inspect(reply) => {
                        let (next, _) = context_snapshot(&app, &window, &bundle)?;
                        same_context(&app, &window, pid, &title, &document)?;
                        if revoked() || started.elapsed() >= duration {
                            let _ = reply.send(Err("Sharing ended during inspection; context withheld.".into()));
                            break;
                        }
                        observations.update(next, "explicit_inspection");
                        let _ = reply.send(Ok(observations.receipt(duration.saturating_sub(started.elapsed()).as_secs())));
                    }
                    ContextRequest::Capture(reply) => {
                        let image = capture_context_window(&app, &window, pid, &title, &document);
                        let result = image.and_then(|bytes| {
                            if revoked() || started.elapsed() >= duration { return Err("Sharing ended during capture; image discarded.".into()); }
                            same_context(&app, &window, pid, &title, &document)?;
                            let (next, _) = context_snapshot(&app, &window, &bundle)?;
                            same_context(&app, &window, pid, &title, &document)?;
                            if revoked() || started.elapsed() >= duration { return Err("Sharing ended during capture inspection; image discarded.".into()); }
                            observations.update(next,"explicit_targeted_vision");
                            Ok((observations.receipt(duration.saturating_sub(started.elapsed()).as_secs()), bytes))
                        });
                        let _ = reply.send(result);
                    }
                }
            }
        }
        Err("Shared-window grant stopped or expired; share the window again if needed.".into())
    });
    stop.store(true, ContextOrdering::Release);
    if let Err(reason) = result {
        super::super::shared_context::invalidate_if(grant);
        let _ = ready.try_send(Err(reason.clone()));
        super::super::shared_context::reject_pending(&requests, &reason);
    }
}
