use crate::config::DesktopContextConfig;
use crate::context_store::{
    self, ContextEventSource, ContextPrivacyScope, ContextStoreError, NewContextEvent,
};
use crate::pid::CaptureMode;
use chrono::{DateTime, Local};
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

pub fn capture_supported() -> bool {
    platform::capture_supported()
}

pub fn session_tracking_enabled(settings: &DesktopContextConfig) -> bool {
    settings.enabled && capture_supported()
}

pub fn maybe_start_capture_session(
    settings: &DesktopContextConfig,
    mode: CaptureMode,
    title: Option<String>,
    started_at: DateTime<Local>,
) -> Option<String> {
    if !session_tracking_enabled(settings) {
        return None;
    }

    match context_store::start_capture_session(mode, title, started_at) {
        Ok(session) => Some(session.id),
        Err(error) => {
            tracing::warn!(error = %error, mode = ?mode, "failed to create desktop context session for capture");
            None
        }
    }
}

pub fn maybe_start_live_transcript_session(
    settings: &DesktopContextConfig,
    started_at: DateTime<Local>,
) -> Option<String> {
    if !session_tracking_enabled(settings) {
        return None;
    }

    match context_store::start_live_transcript_session(started_at) {
        Ok(session) => Some(session.id),
        Err(error) => {
            tracing::warn!(error = %error, "failed to create desktop context session for live transcript");
            None
        }
    }
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
        if !session_tracking_enabled(&settings) {
            return Err(if settings.enabled {
                "desktop context unsupported on this platform".into()
            } else {
                "desktop context disabled in config".into()
            });
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

fn load_runtime_settings() -> DesktopContextConfig {
    crate::config::Config::load().desktop_context
}

fn run_collector_loop(
    stop: Arc<AtomicBool>,
    session_id: String,
    session_kind: DesktopContextSessionKind,
    initial_settings: DesktopContextConfig,
) {
    let mut previous: Option<PlatformSnapshot> = None;
    let mut settings = initial_settings;
    let mut previous_settings = settings.clone();

    while !stop.load(Ordering::Relaxed) {
        settings = load_runtime_settings();
        if settings != previous_settings {
            previous = None;
            previous_settings = settings.clone();
        }
        if !settings.enabled {
            previous = None;
            sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
            continue;
        }

        match platform::snapshot_frontmost_context() {
            Ok(Some(current)) => {
                if !app_allowed(&settings, current.bundle_id.as_deref(), &current.app_name) {
                    previous = None;
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
                            previous = None;
                            sleep_with_stop(&stop, Duration::from_millis(DEFAULT_POLL_INTERVAL_MS));
                            continue;
                        }
                        if !browser_candidate && !settings.capture_window_titles {
                            previous = None;
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
        extern "C" {
            pub fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
            pub fn AXUIElementCopyAttributeValue(
                element: AXUIElementRef,
                attribute: CFStringRef,
                value: *mut CFTypeRef,
            ) -> AXError;
        }

        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            pub fn CFRelease(value: *const c_void);
            pub fn CFStringGetLength(the_string: CFStringRef) -> CFIndex;
            pub fn CFStringGetMaximumSizeForEncoding(
                length: CFIndex,
                encoding: CFStringEncoding,
            ) -> CFIndex;
            pub fn CFStringCreateWithCString(
                alloc: *const c_void,
                c_str: *const i8,
                encoding: CFStringEncoding,
            ) -> CFStringRef;
            pub fn CFStringGetCString(
                the_string: CFStringRef,
                buffer: *mut i8,
                buffer_size: CFIndex,
                encoding: CFStringEncoding,
            ) -> Boolean;
        }
    }

    pub fn capture_supported() -> bool {
        true
    }

    pub fn accessibility_trusted() -> bool {
        crate::hotkey_macos::is_accessibility_trusted()
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

        // AX attribute "constants" are CFSTR() macros in Apple's headers, not exported symbols.
        let focused_window_attribute = ffi::CFStringCreateWithCString(
            std::ptr::null(),
            c"AXFocusedWindow".as_ptr(),
            ffi::kCFStringEncodingUTF8,
        );
        if focused_window_attribute.is_null() {
            ffi::CFRelease(app.cast());
            return None;
        }

        let mut focused_window: ffi::CFTypeRef = std::ptr::null();
        let focused_window_result =
            ffi::AXUIElementCopyAttributeValue(app, focused_window_attribute, &mut focused_window);
        ffi::CFRelease(focused_window_attribute.cast());
        ffi::CFRelease(app.cast());
        if focused_window_result != ffi::kAXErrorSuccess || focused_window.is_null() {
            return None;
        }

        let title_attribute = ffi::CFStringCreateWithCString(
            std::ptr::null(),
            c"AXTitle".as_ptr(),
            ffi::kCFStringEncodingUTF8,
        );
        if title_attribute.is_null() {
            ffi::CFRelease(focused_window.cast());
            return None;
        }

        let mut title: ffi::CFTypeRef = std::ptr::null();
        let title_result =
            ffi::AXUIElementCopyAttributeValue(focused_window.cast(), title_attribute, &mut title);
        ffi::CFRelease(title_attribute.cast());
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

#[cfg(target_os = "windows")]
mod platform {
    use super::PlatformSnapshot;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::path::Path;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
    };

    pub fn capture_supported() -> bool {
        true
    }

    pub fn snapshot_frontmost_context() -> Result<Option<PlatformSnapshot>, String> {
        unsafe {
            let hwnd = GetForegroundWindow();
            if hwnd.is_null() {
                return Ok(None);
            }

            let mut process_id = 0u32;
            let _thread_id = GetWindowThreadProcessId(hwnd, &mut process_id);
            if process_id == 0 {
                return Ok(None);
            }

            let process_path = process_image_path(process_id);
            let bundle_id = process_path
                .as_deref()
                .and_then(|path| Path::new(path).file_name())
                .map(|name| name.to_string_lossy().to_string());
            let app_name = process_path
                .as_deref()
                .and_then(|path| Path::new(path).file_stem())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("pid-{}", process_id));
            let window_title = window_title(hwnd);

            Ok(Some(PlatformSnapshot {
                app_name,
                bundle_id,
                process_id: process_id as i32,
                window_title,
                accessibility_trusted: true,
            }))
        }
    }

    unsafe fn process_image_path(process_id: u32) -> Option<String> {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, process_id);
        if process.is_null() {
            return None;
        }

        let mut buffer = vec![0u16; 1024];
        let mut len = buffer.len() as u32;
        let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len);
        CloseHandle(process);
        if ok == 0 || len == 0 {
            return None;
        }

        Some(
            OsString::from_wide(&buffer[..len as usize])
                .to_string_lossy()
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty())
    }

    unsafe fn window_title(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<String> {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }

        let mut buffer = vec![0u16; (len + 1) as usize];
        let written = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if written <= 0 {
            return None;
        }

        Some(
            OsString::from_wide(&buffer[..written as usize])
                .to_string_lossy()
                .trim()
                .to_string(),
        )
        .filter(|value| !value.is_empty())
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::PlatformSnapshot;
    use std::cell::RefCell;
    use zbus::blocking::{connection, Connection, Proxy};
    use zbus::zvariant::OwnedObjectPath;

    const ATSPI_BUS_SERVICE: &str = "org.a11y.Bus";
    const ATSPI_BUS_PATH: &str = "/org/a11y/bus";
    const ATSPI_BUS_INTERFACE: &str = "org.a11y.Bus";
    const ATSPI_REGISTRY_SERVICE: &str = "org.a11y.atspi.Registry";
    const ATSPI_ROOT_PATH: &str = "/org/a11y/atspi/accessible/root";
    const ATSPI_ACCESSIBLE_INTERFACE: &str = "org.a11y.atspi.Accessible";
    const ATSPI_APPLICATION_INTERFACE: &str = "org.a11y.atspi.Application";
    const ATSPI_STATE_ACTIVE: u32 = 1;
    const ATSPI_ROLE_DIALOG: u32 = 16;
    const ATSPI_ROLE_FRAME: u32 = 23;
    const ATSPI_ROLE_WINDOW: u32 = 69;
    const ATSPI_ROLE_DOCUMENT_FRAME: u32 = 82;
    const MAX_VISITED_OBJECTS: usize = 256;

    thread_local! {
        static CLIENT: RefCell<Option<AtspiClient>> = const { RefCell::new(None) };
    }

    pub fn capture_supported() -> bool {
        with_client(|client| client.bus_available()).unwrap_or(false)
    }

    pub fn snapshot_frontmost_context() -> Result<Option<PlatformSnapshot>, String> {
        with_client(|client| client.snapshot_frontmost_context())
    }

    fn with_client<T>(f: impl FnOnce(&AtspiClient) -> Result<T, String>) -> Result<T, String> {
        CLIENT.with(|slot| {
            if slot.borrow().is_none() {
                *slot.borrow_mut() = Some(AtspiClient::connect()?);
            }

            let result = {
                let guard = slot.borrow();
                let client = guard
                    .as_ref()
                    .ok_or_else(|| "AT-SPI client not initialized".to_string())?;
                f(client)
            };

            if result.is_err() {
                *slot.borrow_mut() = None;
            }

            result
        })
    }

    struct AtspiClient {
        connection: Connection,
    }

    impl AtspiClient {
        fn connect() -> Result<Self, String> {
            let session = Connection::session().map_err(|error| error.to_string())?;
            let bus_proxy = Proxy::new(
                &session,
                ATSPI_BUS_SERVICE,
                ATSPI_BUS_PATH,
                ATSPI_BUS_INTERFACE,
            )
            .map_err(|error| error.to_string())?;
            let address: String = bus_proxy
                .call("GetAddress", &())
                .map_err(|error| error.to_string())?;
            if address.trim().is_empty() {
                return Err("AT-SPI bus address was empty".into());
            }

            let connection = connection::Builder::address(address.as_str())
                .map_err(|error| error.to_string())?
                .build()
                .map_err(|error| error.to_string())?;

            Ok(Self { connection })
        }

        fn bus_available(&self) -> Result<bool, String> {
            let root = self.accessible_proxy(ATSPI_REGISTRY_SERVICE, ATSPI_ROOT_PATH)?;
            let _: Vec<(String, OwnedObjectPath)> = root
                .call("GetChildren", &())
                .map_err(|error| error.to_string())?;
            Ok(true)
        }

        fn snapshot_frontmost_context(&self) -> Result<Option<PlatformSnapshot>, String> {
            let root = self.accessible_proxy(ATSPI_REGISTRY_SERVICE, ATSPI_ROOT_PATH)?;
            let applications: Vec<(String, OwnedObjectPath)> = root
                .call("GetChildren", &())
                .map_err(|error| error.to_string())?;
            if applications.is_empty() {
                return Ok(None);
            }

            for (service, app_path) in applications {
                let app_proxy = self.accessible_proxy(&service, app_path.as_str())?;
                let app_name = app_proxy
                    .get_property::<String>("Name")
                    .unwrap_or_else(|_| service.clone())
                    .trim()
                    .to_string();
                let process_id = self
                    .application_pid(&service, app_path.as_str())
                    .unwrap_or(0);
                let children: Vec<(String, OwnedObjectPath)> =
                    app_proxy
                        .call("GetChildren", &())
                        .map_err(|error| error.to_string())?;

                for (child_service, child_path) in children {
                    let mut visited = 0usize;
                    if let Some(snapshot) = self.find_active_window(
                        &child_service,
                        child_path.as_str(),
                        &app_name,
                        process_id,
                        &mut visited,
                    )? {
                        return Ok(Some(snapshot));
                    }
                }
            }

            Ok(None)
        }

        fn find_active_window(
            &self,
            service: &str,
            path: &str,
            app_name: &str,
            process_id: i32,
            visited: &mut usize,
        ) -> Result<Option<PlatformSnapshot>, String> {
            if *visited >= MAX_VISITED_OBJECTS {
                return Ok(None);
            }
            *visited += 1;

            let proxy = self.accessible_proxy(service, path)?;
            let role: u32 = proxy
                .call("GetRole", &())
                .map_err(|error| error.to_string())?;
            let states: Vec<u32> = proxy
                .call("GetState", &())
                .map_err(|error| error.to_string())?;
            let is_top_level_window = matches!(
                role,
                ATSPI_ROLE_DIALOG
                    | ATSPI_ROLE_FRAME
                    | ATSPI_ROLE_WINDOW
                    | ATSPI_ROLE_DOCUMENT_FRAME
            );
            let is_active = states.iter().any(|state| *state == ATSPI_STATE_ACTIVE);

            if is_top_level_window && is_active {
                let window_title = proxy
                    .get_property::<String>("Name")
                    .ok()
                    .map(|value| value.trim().to_string())
                    .filter(|value| !value.is_empty());
                let bundle_id = Some(service.to_string()).filter(|value| !value.trim().is_empty());

                return Ok(Some(PlatformSnapshot {
                    app_name: app_name.to_string(),
                    bundle_id,
                    process_id,
                    window_title,
                    accessibility_trusted: true,
                }));
            }

            let children: Vec<(String, OwnedObjectPath)> = proxy
                .call("GetChildren", &())
                .map_err(|error| error.to_string())?;
            for (child_service, child_path) in children {
                if let Some(snapshot) = self.find_active_window(
                    &child_service,
                    child_path.as_str(),
                    app_name,
                    process_id,
                    visited,
                )? {
                    return Ok(Some(snapshot));
                }
            }

            Ok(None)
        }

        fn application_pid(&self, service: &str, path: &str) -> Option<i32> {
            let proxy =
                Proxy::new(&self.connection, service, path, ATSPI_APPLICATION_INTERFACE).ok()?;
            proxy.get_property::<i32>("Id").ok()
        }

        fn accessible_proxy<'a>(
            &self,
            service: &'a str,
            path: &'a str,
        ) -> Result<Proxy<'a>, String> {
            Proxy::new(&self.connection, service, path, ATSPI_ACCESSIBLE_INTERFACE)
                .map_err(move |error| error.to_string())
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
mod platform {
    use super::PlatformSnapshot;

    pub fn capture_supported() -> bool {
        false
    }

    pub fn snapshot_frontmost_context() -> Result<Option<PlatformSnapshot>, String> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce(&TempDir) -> T) -> T {
        let _lock = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        #[cfg(windows)]
        let original_userprofile = std::env::var_os("USERPROFILE");

        std::env::set_var("HOME", dir.path());
        #[cfg(windows)]
        std::env::set_var("USERPROFILE", dir.path());

        let result = f(&dir);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        #[cfg(windows)]
        if let Some(userprofile) = original_userprofile {
            std::env::set_var("USERPROFILE", userprofile);
        } else {
            std::env::remove_var("USERPROFILE");
        }

        result
    }

    #[test]
    fn desktop_context_session_tracking_requires_opt_in() {
        let mut settings = DesktopContextConfig::default();
        assert!(!session_tracking_enabled(&settings));

        settings.enabled = true;
        assert_eq!(session_tracking_enabled(&settings), capture_supported());
    }

    #[test]
    fn desktop_context_app_filters_deny_before_allow() {
        let settings = DesktopContextConfig {
            enabled: true,
            capture_window_titles: true,
            capture_browser_context: false,
            allowed_apps: vec!["arc".into()],
            denied_apps: vec!["1password".into()],
            allowed_domains: vec![],
            denied_domains: vec![],
        };

        assert!(app_allowed(
            &settings,
            Some("company.thebrowser.arc"),
            "Arc"
        ));
        assert!(!app_allowed(
            &settings,
            Some("com.1password.1password"),
            "1Password"
        ));
        assert!(!app_allowed(&settings, Some("com.apple.Safari"), "Safari"));
    }

    #[test]
    fn desktop_context_disabled_capture_session_does_not_touch_store() {
        let session = maybe_start_capture_session(
            &DesktopContextConfig::default(),
            CaptureMode::Meeting,
            Some("test".into()),
            Local::now(),
        );
        assert!(session.is_none());
    }

    #[test]
    fn desktop_context_disabled_live_session_does_not_touch_store() {
        let session =
            maybe_start_live_transcript_session(&DesktopContextConfig::default(), Local::now());
        assert!(session.is_none());
    }

    #[test]
    fn desktop_context_runtime_settings_reload_from_saved_config() {
        with_temp_home(|_| {
            let path = crate::config::Config::config_path();
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                &path,
                r#"[desktop_context]
enabled = true
capture_window_titles = false
capture_browser_context = true
allowed_apps = ["Arc"]
denied_apps = ["Messages"]
"#,
            )
            .unwrap();

            let settings = load_runtime_settings();
            assert!(settings.enabled);
            assert!(!settings.capture_window_titles);
            assert!(settings.capture_browser_context);
            assert_eq!(settings.allowed_apps, vec!["Arc"]);
            assert_eq!(settings.denied_apps, vec!["Messages"]);
        });
    }
}
