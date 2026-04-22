use chrono::Local;
use minutes_core::config::DesktopContextConfig;
use minutes_core::context_store::{
    self, ContextEventSource, ContextPrivacyScope, ContextStoreError, NewContextEvent,
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

const DEFAULT_POLL_INTERVAL_MS: u64 = 1500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopContextSessionKind {
    Recording,
    LiveTranscript,
}

impl DesktopContextSessionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::LiveTranscript => "live_transcript",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlatformSnapshot {
    app_name: String,
    bundle_id: Option<String>,
    process_id: i32,
    window_title: Option<String>,
    accessibility_trusted: bool,
}

pub struct DesktopContextCollector {
    stop: Arc<AtomicBool>,
    join_handle: Option<JoinHandle<()>>,
}

impl DesktopContextCollector {
    pub fn start(
        session_id: String,
        session_kind: DesktopContextSessionKind,
        settings: DesktopContextConfig,
    ) -> Result<Self, String> {
        if !settings.enabled {
            return Err("desktop context disabled in config".into());
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let join_handle = thread::Builder::new()
            .name(format!("desktop-context-{}", session_kind.as_str()))
            .spawn(move || run_collector_loop(stop_for_thread, session_id, session_kind, settings))
            .map_err(|e| format!("failed to spawn desktop-context collector: {e}"))?;

        Ok(Self {
            stop,
            join_handle: Some(join_handle),
        })
    }
}

impl Drop for DesktopContextCollector {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

fn run_collector_loop(
    stop: Arc<AtomicBool>,
    session_id: String,
    session_kind: DesktopContextSessionKind,
    settings: DesktopContextConfig,
) {
    let mut previous: Option<PlatformSnapshot> = None;

    while !stop.load(Ordering::Relaxed) {
        match platform::snapshot_frontmost_context() {
            Ok(Some(current)) => {
                if !app_allowed(&settings, current.bundle_id.as_deref(), &current.app_name) {
                    previous = Some(current);
                    sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
                    continue;
                }
                let app_focus_changed = previous
                    .as_ref()
                    .map(|prev| {
                        prev.process_id != current.process_id
                            || prev.app_name != current.app_name
                            || prev.bundle_id != current.bundle_id
                    })
                    .unwrap_or(true);
                if app_focus_changed {
                    append_event(
                        &session_id,
                        NewContextEvent {
                            observed_at: Local::now(),
                            source: ContextEventSource::AppFocus,
                            app_name: Some(current.app_name.clone()),
                            bundle_id: current.bundle_id.clone(),
                            window_title: None,
                            url: None,
                            domain: None,
                            artifact_path: None,
                            privacy_scope: ContextPrivacyScope::Normal,
                            metadata: serde_json::json!({
                                "session_kind": session_kind.as_str(),
                                "process_id": current.process_id,
                            }),
                        },
                    );
                }

                let window_title_changed = previous
                    .as_ref()
                    .map(|prev| app_focus_changed || prev.window_title != current.window_title)
                    .unwrap_or(current.window_title.is_some());
                if window_title_changed {
                    if let Some(window_title) = current.window_title.clone() {
                        let browser_candidate =
                            is_browser_candidate(current.bundle_id.as_deref(), &current.app_name);
                        if browser_candidate && !settings.capture_browser_context {
                            previous = Some(current);
                            sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
                            continue;
                        }
                        if !browser_candidate && !settings.capture_window_titles {
                            previous = Some(current);
                            sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
                            continue;
                        }
                        let source = if browser_candidate {
                            ContextEventSource::BrowserPage
                        } else {
                            ContextEventSource::WindowFocus
                        };
                        append_event(
                            &session_id,
                            NewContextEvent {
                                observed_at: Local::now(),
                                source,
                                app_name: Some(current.app_name.clone()),
                                bundle_id: current.bundle_id.clone(),
                                window_title: Some(window_title),
                                url: None,
                                domain: None,
                                artifact_path: None,
                                privacy_scope: ContextPrivacyScope::Normal,
                                metadata: serde_json::json!({
                                    "session_kind": session_kind.as_str(),
                                    "process_id": current.process_id,
                                    "title_source": "accessibility",
                                    "accessibility_trusted": current.accessibility_trusted,
                                    "browser_candidate": browser_candidate,
                                    // Browser enrichment is intentionally deferred; if the
                                    // frontmost app is a browser, the focused-window title
                                    // is the only metadata we capture in this first wave.
                                    "browser_enrichment": if browser_candidate {
                                        "deferred_window_title_only"
                                    } else {
                                        "not_browser"
                                    },
                                }),
                            },
                        );
                    }
                }

                previous = Some(current);
            }
            Ok(None) => {}
            Err(error) => {
                tracing::debug!(
                    error = %error,
                    session_id,
                    session_kind = session_kind.as_str(),
                    "desktop context snapshot failed"
                );
            }
        }

        sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
    }
}

fn append_event(session_id: &str, event: NewContextEvent) {
    if let Err(error) = context_store::append_event(session_id, event) {
        log_append_error(session_id, &error);
    }
}

fn log_append_error(session_id: &str, error: &ContextStoreError) {
    tracing::warn!(session_id, error = %error, "failed to append desktop context event");
}

fn app_allowed(settings: &DesktopContextConfig, bundle_id: Option<&str>, app_name: &str) -> bool {
    let app = app_name.trim().to_ascii_lowercase();
    let bundle = bundle_id.unwrap_or_default().trim().to_ascii_lowercase();
    let candidate = if !bundle.is_empty() { &bundle } else { &app };

    let denied = settings
        .denied_apps
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .any(|value| {
            !value.is_empty()
                && (candidate.contains(&value) || app.contains(&value) || bundle.contains(&value))
        });
    if denied {
        return false;
    }

    let allow_list: Vec<String> = settings
        .allowed_apps
        .iter()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .collect();
    if allow_list.is_empty() {
        return true;
    }

    allow_list
        .iter()
        .any(|value| candidate.contains(value) || app.contains(value) || bundle.contains(value))
}

fn is_browser_candidate(bundle_id: Option<&str>, app_name: &str) -> bool {
    let bundle = bundle_id.unwrap_or_default().to_ascii_lowercase();
    let app = app_name.to_ascii_lowercase();
    bundle.contains("safari")
        || bundle.contains("chrome")
        || bundle.contains("chromium")
        || bundle.contains("arc")
        || bundle.contains("firefox")
        || bundle.contains("edge")
        || app.contains("safari")
        || app.contains("chrome")
        || app.contains("chromium")
        || app.contains("arc")
        || app.contains("firefox")
        || app.contains("edge")
}

fn sleep_with_stop(stop: &AtomicBool, duration: Duration) {
    let started_at = std::time::Instant::now();
    while started_at.elapsed() < duration {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::PlatformSnapshot;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::NSWorkspace;

    #[allow(non_upper_case_globals)]
    mod ffi {
        use std::ffi::c_void;

        pub type AXUIElementRef = *const c_void;
        pub type CFStringRef = *const c_void;
        pub type CFTypeRef = *const c_void;
        pub type CFIndex = isize;
        pub type Boolean = u8;
        pub type CFStringEncoding = u32;
        pub type AXError = i32;
        pub const kCFStringEncodingUTF8: CFStringEncoding = 0x0800_0100;
        pub const kAXErrorSuccess: AXError = 0;

        #[link(name = "ApplicationServices", kind = "framework")]
        extern "C" {}

        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {}

        extern "C" {
            pub static kAXFocusedWindowAttribute: CFStringRef;
            pub static kAXTitleAttribute: CFStringRef;

            pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
            pub fn AXUIElementCopyAttributeValue(
                element: AXUIElementRef,
                attribute: CFStringRef,
                value: *mut CFTypeRef,
            ) -> AXError;
            pub fn CFRelease(value: *const c_void);
            pub fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
            pub fn CFStringGetMaximumSizeForEncoding(
                length: CFIndex,
                encoding: CFStringEncoding,
            ) -> CFIndex;
            pub fn CFStringGetCString(
                the_string: CFStringRef,
                buffer: *mut i8,
                buffer_size: CFIndex,
                encoding: CFStringEncoding,
            ) -> Boolean;
        }
    }

    pub fn accessibility_trusted() -> bool {
        minutes_core::hotkey_macos::is_accessibility_trusted()
    }

    pub fn snapshot_frontmost_context() -> Result<Option<PlatformSnapshot>, String> {
        autoreleasepool(|_| unsafe {
            let workspace = NSWorkspace::sharedWorkspace();
            let Some(frontmost) = workspace.frontmostApplication() else {
                return Ok(None);
            };

            let process_id = frontmost.processIdentifier();
            if process_id <= 0 {
                return Ok(None);
            }

            let app_name = frontmost
                .localizedName()
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "Unknown App".into());
            let bundle_id = frontmost
                .bundleIdentifier()
                .map(|value| value.to_string())
                .filter(|value| !value.trim().is_empty());
            let accessibility_trusted = accessibility_trusted();
            let window_title = if accessibility_trusted {
                focused_window_title(process_id)
            } else {
                None
            };

            Ok(Some(PlatformSnapshot {
                app_name,
                bundle_id,
                process_id,
                window_title,
                accessibility_trusted,
            }))
        })
    }

    unsafe fn focused_window_title(process_id: i32) -> Option<String> {
        let app = ffi::AXUIElementCreateApplication(process_id);
        if app.is_null() {
            return None;
        }

        let mut focused_window: ffi::CFTypeRef = std::ptr::null();
        let focused_window_result = ffi::AXUIElementCopyAttributeValue(
            app,
            ffi::kAXFocusedWindowAttribute,
            &mut focused_window,
        );
        ffi::CFRelease(app.cast());
        if focused_window_result != ffi::kAXErrorSuccess || focused_window.is_null() {
            return None;
        }

        let mut title: ffi::CFTypeRef = std::ptr::null();
        let title_result = ffi::AXUIElementCopyAttributeValue(
            focused_window.cast(),
            ffi::kAXTitleAttribute,
            &mut title,
        );
        ffi::CFRelease(focused_window.cast());
        if title_result != ffi::kAXErrorSuccess || title.is_null() {
            return None;
        }

        let string = cf_string_to_string(title.cast());
        ffi::CFRelease(title.cast());
        string.and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }

    unsafe fn cf_string_to_string(value: ffi::CFStringRef) -> Option<String> {
        let length = ffi::CFStringGetLength(value);
        if length <= 0 {
            return Some(String::new());
        }

        let max_size = ffi::CFStringGetMaximumSizeForEncoding(length, ffi::kCFStringEncodingUTF8);
        if max_size <= 0 {
            return None;
        }

        let mut buffer = vec![0i8; (max_size + 1) as usize];
        let ok = ffi::CFStringGetCString(
            value,
            buffer.as_mut_ptr(),
            buffer.len() as ffi::CFIndex,
            ffi::kCFStringEncodingUTF8,
        );
        if ok == 0 {
            return None;
        }

        let bytes = buffer
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        String::from_utf8(bytes).ok()
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::PlatformSnapshot;

    pub fn snapshot_frontmost_context() -> Result<Option<PlatformSnapshot>, String> {
        Ok(None)
    }
}
