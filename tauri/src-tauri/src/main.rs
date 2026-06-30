#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use minutes_core::Config;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
#[cfg(target_os = "macos")]
use std::ffi::{c_char, c_void};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tauri::{
    menu::{Menu, MenuItem, SubmenuBuilder},
    tray::TrayIconBuilder,
    Emitter, LogicalPosition, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

#[cfg(feature = "parakeet")]
#[used]
static PARAKEET_FEATURE_SENTINEL: &[u8] = b"transcribe_parakeet parakeet_helper\0";

mod call_capture;
mod call_detect;
#[cfg(target_os = "macos")]
mod cli_setup;
mod commands;
mod context;
mod palette_dispatch;
mod pty;
mod secret_store;
mod shortcut_manager;
mod text_insertion;

const MINUTES_WEBSITE_URL: &str = "https://useminutes.app";
const MINUTES_CHANGELOG_URL: &str = "https://github.com/silverstein/minutes/releases";
const MINUTES_DISCUSSIONS_URL: &str = "https://github.com/silverstein/minutes/discussions";
const MAIN_WINDOW_TRANSPARENT: bool = false;
#[cfg(target_os = "macos")]
const MAIN_WINDOW_APPLY_VIBRANCY: bool = false;

static CLEAN_EXIT_STARTED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
static MACOS_TERMINATE_APP_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

fn cleanup_before_process_exit(app: &tauri::AppHandle) {
    let sessions = app
        .try_state::<commands::AppState>()
        .and_then(|state| {
            state
                .pty_manager
                .lock()
                .ok()
                .map(|mut mgr| mgr.take_all_sessions())
        })
        .unwrap_or_default();
    crate::pty::kill_all(sessions);
    minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
}

fn exit_process_without_destructors(code: i32) -> ! {
    #[cfg(target_os = "macos")]
    unsafe {
        libc::_exit(code);
    }

    #[cfg(not(target_os = "macos"))]
    {
        std::process::exit(code);
    }
}

fn finish_clean_exit(app: tauri::AppHandle, code: i32) -> ! {
    cleanup_before_process_exit(&app);
    exit_process_without_destructors(code);
}

fn request_clean_exit(app: &tauri::AppHandle, code: i32) {
    if CLEAN_EXIT_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }

    let state = app.state::<commands::AppState>();
    if commands::recording_active(&state.recording) {
        if commands::request_stop(&state.recording, &state.stop_flag).is_err() {
            CLEAN_EXIT_STARTED.store(false, Ordering::SeqCst);
            return;
        }

        let app_handle = app.clone();
        std::thread::spawn(move || {
            commands::wait_for_recording_shutdown_forever();
            finish_clean_exit(app_handle, code);
        });
    } else {
        finish_clean_exit(app.clone(), code);
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> *mut c_void;
    fn sel_registerName(name: *const c_char) -> *mut c_void;
    fn class_addMethod(
        cls: *mut c_void,
        name: *mut c_void,
        imp: *const c_void,
        types: *const c_char,
    ) -> libc::c_schar;
}

#[cfg(target_os = "macos")]
unsafe extern "C" fn application_should_terminate(
    _this: *mut c_void,
    _cmd: *mut c_void,
    _sender: *mut c_void,
) -> isize {
    const NS_TERMINATE_CANCEL: isize = 0;
    const NS_TERMINATE_NOW: isize = 1;

    if let Some(app) = MACOS_TERMINATE_APP_HANDLE.get() {
        request_clean_exit(app, 0);
        NS_TERMINATE_CANCEL
    } else {
        NS_TERMINATE_NOW
    }
}

#[cfg(target_os = "macos")]
fn install_macos_terminate_hook(app: &tauri::AppHandle) {
    let _ = MACOS_TERMINATE_APP_HANDLE.set(app.clone());

    unsafe {
        let cls = objc_getClass(c"TaoAppDelegateParent".as_ptr());
        if cls.is_null() {
            eprintln!("[macos] unable to install quit hook: TaoAppDelegateParent not registered");
            return;
        }

        let selector = sel_registerName(c"applicationShouldTerminate:".as_ptr());
        if selector.is_null() {
            eprintln!("[macos] unable to install quit hook: selector registration failed");
            return;
        }

        let added = class_addMethod(
            cls,
            selector,
            application_should_terminate as *const () as *const c_void,
            c"q@:@".as_ptr(),
        );

        if added == 0 {
            eprintln!("[macos] quit hook already installed or unavailable");
        }
    }
}

#[cfg(target_os = "macos")]
fn maybe_run_hotkey_diagnostic() -> Option<i32> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !args.iter().any(|arg| arg == "--diagnose-hotkey") {
        return None;
    }

    let mut keycode = minutes_core::hotkey_macos::KEYCODE_CAPS_LOCK;
    let mut output_path: Option<std::path::PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "--diagnose-hotkey-keycode" {
            if let Some(value) = iter.next() {
                if let Ok(parsed) = value.parse::<i64>() {
                    keycode = parsed;
                }
            }
        } else if arg == "--diagnose-hotkey-output" {
            if let Some(value) = iter.next() {
                output_path = Some(std::path::PathBuf::from(value));
            }
        } else if let Some(value) = arg.strip_prefix("--diagnose-hotkey-keycode=") {
            if let Ok(parsed) = value.parse::<i64>() {
                keycode = parsed;
            }
        } else if let Some(value) = arg.strip_prefix("--diagnose-hotkey-output=") {
            output_path = Some(std::path::PathBuf::from(value));
        }
    }

    let probe = minutes_core::hotkey_macos::probe_hotkey_monitor(
        keycode,
        std::time::Duration::from_millis(1200),
    );
    let current_exe = std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string());
    let bundle_root = current_exe.as_ref().and_then(|path| {
        path.strip_suffix("/Contents/MacOS/minutes-app")
            .map(|root| root.to_string())
    });

    let payload = serde_json::json!({
        "mode": "diagnose-hotkey",
        "current_exe": current_exe,
        "bundle_root": bundle_root,
        "probe": probe,
    });

    match serde_json::to_string_pretty(&payload) {
        Ok(json) => {
            if let Some(path) = output_path {
                if let Some(parent) = path.parent() {
                    if let Err(error) = std::fs::create_dir_all(parent) {
                        eprintln!("failed to create diagnostic output directory: {}", error);
                        return Some(1);
                    }
                }
                if let Err(error) = std::fs::write(&path, &json) {
                    eprintln!("failed to write hotkey diagnostic: {}", error);
                    return Some(1);
                }
            }
            println!("{}", json);
        }
        Err(error) => {
            eprintln!("failed to encode hotkey diagnostic: {}", error);
            return Some(1);
        }
    }

    Some(if probe.status == "active" { 0 } else { 2 })
}

fn maybe_run_process_queue_worker() -> Option<i32> {
    if !std::env::args()
        .skip(1)
        .any(|arg| arg == "--process-queue-worker")
    {
        return None;
    }

    let config = minutes_core::config::Config::load();
    match minutes_core::jobs::process_pending_jobs(&config, |_| {}) {
        Ok(()) => Some(0),
        Err(error) => {
            eprintln!("[minutes] processing worker failed: {}", error);
            Some(1)
        }
    }
}

pub(crate) fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        if !win.is_visible().ok().unwrap_or(false) {
            win.show().ok();
        }
        if win.is_minimized().ok().unwrap_or(false) {
            win.unminimize().ok();
        }
        if !win.is_focused().ok().unwrap_or(false) {
            win.set_focus().ok();
        }
        return;
    }
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        // Empty title hides the centered "Minutes" text in any native chrome.
        // The in-app brand mark (italic m + recording dot) carries the identity.
        .title("")
        .inner_size(560.0, 700.0)
        .min_inner_size(460.0, 520.0)
        // The main window is hidden, shown, and resized while the app lives in
        // the tray. On macOS 26, keeping that long-lived WebView transparent
        // and backed by vibrancy can trip a WebKit frame-update PAC trap when
        // the hidden window is re-framed. The UI already paints solid app
        // surfaces, so keep this WebView opaque and reserve transparency for
        // short-lived overlays that need it.
        .transparent(MAIN_WINDOW_TRANSPARENT)
        .content_protected(Config::load().privacy.hide_from_screen_share)
        .focused(true);

    #[cfg(target_os = "macos")]
    {
        builder = builder
            // Keep this as a normal app window, but let the in-app header own the
            // visual chrome instead of stacking below a separate gray title bar.
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(16.0, 16.0));
    }

    if let Ok(win) = builder.build() {
        #[cfg(target_os = "macos")]
        if MAIN_WINDOW_APPLY_VIBRANCY {
            use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
            apply_vibrancy(&win, NSVisualEffectMaterial::Sidebar, None, None).ok();
        }
        // Re-seed the tray appearance from the fresh window's theme. In
        // normal flow CloseRequested hides instead of destroys (see
        // on_window_event for "main"), so this branch is rare — but if
        // the main window was ever destroyed and the system appearance
        // flipped while it was absent, `ThemeChanged` never fired and the
        // cached state is stale. Reseed + repaint here is idempotent
        // when the cache is already correct (codex diff-review P2).
        if let Some(state) = app.try_state::<TrayAppearanceState>() {
            if let Ok(theme) = win.theme() {
                state.set(TrayAppearance::from_theme(theme));
                sync_tray_appearance(app);
            }
        }
    }
}

#[tauri::command]
fn cmd_show_main_window(app: tauri::AppHandle) {
    show_main_window(&app);
}

#[tauri::command]
fn cmd_apply_recall_window_layout(
    app: tauri::AppHandle,
    width: f64,
    height: f64,
    avoid_right_edge: bool,
) -> Result<(), String> {
    const MIN_WIDTH: f64 = 460.0;
    const MAX_WIDTH: f64 = 1400.0;
    const MIN_HEIGHT: f64 = 520.0;
    const MAX_HEIGHT: f64 = 1000.0;
    const FRAME_EPSILON: f64 = 1.0;

    if !width.is_finite() || !height.is_finite() {
        return Err("invalid recall window layout size".into());
    }

    let Some(win) = app.get_webview_window("main") else {
        return Ok(());
    };
    if !win.is_visible().ok().unwrap_or(false) {
        return Ok(());
    }
    if win.is_fullscreen().ok().unwrap_or(false) {
        return Ok(());
    }

    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    let monitor = win.current_monitor().ok().flatten();
    let scale = monitor.as_ref().map(|monitor| monitor.scale_factor());

    if avoid_right_edge {
        if let (Some(monitor), Some(scale)) = (monitor.as_ref(), scale) {
            let position = win.outer_position().map_err(|e| e.to_string())?;
            let logical_x = position.x as f64 / scale;
            let logical_y = position.y as f64 / scale;
            let work_area = monitor.work_area();
            let work_x = work_area.position.x as f64 / scale;
            let work_width = work_area.size.width as f64 / scale;
            let work_right = work_x + work_width;
            let new_right = logical_x + width;
            if new_right - work_right > FRAME_EPSILON {
                let shift_x = (logical_x - (new_right - work_right)).max(work_x);
                if (shift_x - logical_x).abs() > FRAME_EPSILON {
                    win.set_position(LogicalPosition::new(shift_x, logical_y))
                        .map_err(|e| e.to_string())?;
                }
            }
        }
    }

    let size_needs_update = match (win.outer_size(), scale) {
        (Ok(size), Some(scale)) => {
            let logical_width = size.width as f64 / scale;
            let logical_height = size.height as f64 / scale;
            (logical_width - width).abs() > FRAME_EPSILON
                || (logical_height - height).abs() > FRAME_EPSILON
        }
        _ => true,
    };

    if size_needs_update {
        win.set_size(LogicalSize::new(width, height))
            .map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Unscaled (zoom=16) logical size each secondary window is built at. Scaling
/// always derives from this constant, never from the window's current size, so
/// repeated opens can't compound the dimensions. Keep in sync with each
/// window's `.inner_size(..)` in its builder.
fn window_base_size(label: &str) -> Option<(f64, f64)> {
    match label {
        "palette" => Some((640.0, 420.0)),
        "note" => Some((420.0, 260.0)),
        "dictation-overlay" => Some((320.0, 88.0)),
        "meeting-prompt" => Some((380.0, 240.0)),
        _ => None,
    }
}

#[tauri::command]
fn cmd_scale_window(app: tauri::AppHandle, label: String, zoom: f64) -> Result<(), String> {
    const BASE: f64 = 16.0;
    let Some(win) = app.get_webview_window(&label) else {
        return Ok(());
    };
    let Some((base_w, base_h)) = window_base_size(&label) else {
        return Ok(());
    };
    let ratio = zoom / BASE;
    let width = (base_w * ratio).round();
    let height = (base_h * ratio).round();

    // setFrame from Rust works on non-resizable windows; no resizable toggle is
    // needed
    win.set_size(LogicalSize::new(width, height))
        .map_err(|e| e.to_string())?;

    // Clamp position so bottom-right anchored windows (e.g. dictation overlay)
    // don't extend off-screen after scaling up.
    let monitor = win
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| win.primary_monitor().ok().flatten())
        .or_else(|| {
            app.get_webview_window("main")
                .and_then(|m| m.current_monitor().ok().flatten())
        });
    if let (Some(monitor), Ok(pos)) = (monitor, win.outer_position()) {
        let scale = monitor.scale_factor();
        let work_area = monitor.work_area();
        let work_x = work_area.position.x as f64 / scale;
        let work_y = work_area.position.y as f64 / scale;
        let work_right = work_x + work_area.size.width as f64 / scale;
        let work_bottom = work_y + work_area.size.height as f64 / scale;
        let logical_x = pos.x as f64 / scale;
        let logical_y = pos.y as f64 / scale;
        let clamped_x = logical_x.min(work_right - width).max(work_x);
        let clamped_y = logical_y.min(work_bottom - height).max(work_y);
        if (clamped_x - logical_x).abs() > 0.5 || (clamped_y - logical_y).abs() > 0.5 {
            win.set_position(LogicalPosition::new(clamped_x, clamped_y))
                .ok();
        }
    }

    Ok(())
}

fn show_note_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("note") {
        win.show().ok();
        win.set_focus().ok();
        return;
    }
    let _win = WebviewWindowBuilder::new(app, "note", WebviewUrl::App("note.html".into()))
        .title("Add Note")
        .inner_size(420.0, 260.0)
        .resizable(false)
        .content_protected(Config::load().privacy.hide_from_screen_share)
        .always_on_top(true)
        .center()
        .focused(true)
        .build();
}

pub fn show_terminal_window(app: &tauri::AppHandle, session_id: &str, title: &str) {
    // Use session_id as the window label (must be unique)
    let label = session_id.replace(':', "-");
    if let Some(win) = app.get_webview_window(&label) {
        win.set_title(title).ok();
        win.show().ok();
        win.set_focus().ok();
        app.emit_to(
            &label,
            &format!("terminal:title:{}", session_id),
            title.to_string(),
        )
        .ok();
        return;
    }
    // Pass session_id via a fragment so terminal.html can read it
    let url = format!("terminal.html#{}", session_id);
    let url_log = url.clone();
    match WebviewWindowBuilder::new(app, &label, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(900.0, 600.0)
        .min_inner_size(600.0, 400.0)
        .content_protected(Config::load().privacy.hide_from_screen_share)
        .center()
        .focused(true)
        .build()
    {
        Ok(_) => eprintln!("[terminal] window created: label={} url={}", label, url_log),
        Err(e) => eprintln!(
            "[terminal] window creation FAILED: {} (label={}, url={})",
            e, label, url_log
        ),
    }
}

/// Cloned references to the tray's recording-related menu items, registered
/// via `app.manage()` after menu construction so any recording-state change
/// (regardless of source — tray click, main window, hotkey, CLI, call-detect,
/// palette) can flip the menu items' enabled state through the central
/// `sync_tray_state` function.
///
/// Without this, items built with `MenuItem::with_id(..., enabled, ...)` are
/// frozen in their startup state because `enabled` is only consulted at
/// construction time. Issue #223 surfaced this: stop was always grayed out
/// when recordings started from outside the tray.
///
/// Tauri 2's `MenuItem<R>` is `Send + Sync` (it's `Arc<MenuItemInner<R>>`
/// internally with explicit unsafe impls; all setter operations marshal back
/// to the main thread), so no extra wrapping is needed.
pub struct TrayMenuHandles {
    pub record: tauri::menu::MenuItem<tauri::Wry>,
    pub quick_thought: tauri::menu::MenuItem<tauri::Wry>,
    pub stop: tauri::menu::MenuItem<tauri::Wry>,
}

/// The active "recording-class" activity that the tray reflects. Each
/// acquisition path (`launch_recording`, `try_acquire_live`,
/// `try_acquire_dictation`) gates against the other two — but those gates
/// are check-then-CAS across separate atomics, so under concurrent starts
/// the cross-mode race window can briefly leave two flags set. The core
/// layer still serializes the actual capture session via PID + flock
/// (see `minutes_core::pid` and the run-start checks in `dictation::run` /
/// `live_transcript::run`), so the underlying audio stream is exclusive
/// even when the in-app atomics drift. The losing session's flag clears
/// when its `run` fails and its RAII guard drops, re-syncing the tray.
///
/// `derive_tray_activity` defines a deterministic priority order
/// (Recording > Live > Dictation > Idle) so the tray renders coherently
/// during the brief drift window. Properly closing the cross-mode race
/// needs a single serialized lifecycle primitive (e.g. an `AtomicU8` mode
/// CAS or a mutex around mode reservation) — out of scope here, tracked
/// for follow-up.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrayActivity {
    Idle,
    Recording,
    Live,
    Dictation,
}

/// Inferred macOS menu-bar appearance. Honestly a proxy: we read the app's
/// `NSApplication.effectiveAppearance` via Tauri's `Window::theme()` API,
/// which is the system Aqua/DarkAqua choice — NOT the status item's actual
/// rendering background (which can drift in translucent / wallpaper-tinted
/// menu bar configurations). It's a good-enough heuristic for the common
/// case (status items track system appearance on stock macOS), which beats
/// the current state of a low-contrast icon on every dark menu bar.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TrayAppearance {
    Light,
    Dark,
}

impl TrayAppearance {
    fn from_theme(theme: tauri::Theme) -> Self {
        // Tauri's Theme has Light/Dark/_NonExhaustive. Default to Light for
        // any future variant — matches current asset's design target.
        match theme {
            tauri::Theme::Dark => Self::Dark,
            _ => Self::Light,
        }
    }
}

/// Last-known menu-bar appearance, seeded from `Window::theme()` and
/// updated by the `WindowEvent::ThemeChanged` listener. Stored as a
/// managed `AtomicU8` so tray syncs from any thread read a current value
/// without locking. Light = 0, Dark = 1.
pub struct TrayAppearanceState(pub Arc<AtomicU8>);

impl TrayAppearanceState {
    pub fn new(initial: TrayAppearance) -> Self {
        Self(Arc::new(AtomicU8::new(match initial {
            TrayAppearance::Light => 0,
            TrayAppearance::Dark => 1,
        })))
    }

    pub fn get(&self) -> TrayAppearance {
        match self.0.load(Ordering::Relaxed) {
            1 => TrayAppearance::Dark,
            _ => TrayAppearance::Light,
        }
    }

    pub fn set(&self, appearance: TrayAppearance) {
        self.0.store(
            match appearance {
                TrayAppearance::Light => 0,
                TrayAppearance::Dark => 1,
            },
            Ordering::Relaxed,
        );
    }
}

fn current_tray_appearance(app: &tauri::AppHandle) -> TrayAppearance {
    app.try_state::<TrayAppearanceState>()
        .map(|s| s.get())
        .unwrap_or(TrayAppearance::Light)
}

impl TrayActivity {
    fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }

    fn icon_bytes(self, appearance: TrayAppearance) -> &'static [u8] {
        match (self, appearance) {
            (Self::Idle, _) => include_bytes!("../icons/icon-tray.png"),
            (Self::Recording | Self::Dictation, TrayAppearance::Light) => {
                include_bytes!("../icons/icon-recording.png")
            }
            (Self::Recording | Self::Dictation, TrayAppearance::Dark) => {
                include_bytes!("../icons/icon-recording-dark.png")
            }
            (Self::Live, TrayAppearance::Light) => include_bytes!("../icons/icon-live.png"),
            (Self::Live, TrayAppearance::Dark) => include_bytes!("../icons/icon-live-dark.png"),
        }
    }

    fn tooltip(self) -> &'static str {
        match self {
            Self::Idle => "Minutes",
            Self::Recording => "Minutes — Recording...",
            Self::Live => "Minutes — Live Transcribing...",
            Self::Dictation => "Minutes — Dictating...",
        }
    }

    fn stop_label(self) -> &'static str {
        match self {
            // Idle reuses the construction-time label at main.rs ~1535 so
            // the menu reads "Stop Recording" by default before any session.
            Self::Idle | Self::Recording => "Stop Recording",
            Self::Live => "Stop Live Transcript",
            Self::Dictation => "Stop Dictation",
        }
    }

    fn palette_source(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Recording => "recording",
            Self::Live => "live-transcript",
            Self::Dictation => "dictation",
        }
    }
}

/// Snapshot of the lifecycle flags used to derive `TrayActivity`. Capturing
/// once per sync keeps derivation deterministic if a flag flips between
/// reads, and makes the derivation testable as a pure function.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct TrayStateSnapshot {
    pub recording: bool,
    pub live: bool,
    pub dictation: bool,
}

/// Derive the tray activity from a state snapshot. Pure function; tested in
/// the module's `#[cfg(test)]` block. Priority is Recording > Live >
/// Dictation > Idle so an external CLI recording (surfaced through
/// `recording_active` PID check) keeps the tray rendering as recording even
/// if an in-app dictation flag is somehow also set.
pub fn derive_tray_activity(snapshot: TrayStateSnapshot) -> TrayActivity {
    if snapshot.recording {
        TrayActivity::Recording
    } else if snapshot.live {
        TrayActivity::Live
    } else if snapshot.dictation {
        TrayActivity::Dictation
    } else {
        TrayActivity::Idle
    }
}

fn snapshot_tray_state(app: &tauri::AppHandle) -> TrayStateSnapshot {
    match app.try_state::<commands::AppState>() {
        Some(state) => TrayStateSnapshot {
            // Use the PID-aware helper so external/CLI recordings keep the
            // tray showing Recording at app launch (codex plan-review catch).
            recording: commands::recording_active(&state.recording),
            live: state.live_transcript_active.load(Ordering::Relaxed),
            dictation: state.dictation_active.load(Ordering::Relaxed),
        },
        None => TrayStateSnapshot {
            recording: false,
            live: false,
            dictation: false,
        },
    }
}

/// Apply the tray rendering for `activity` against the last-known menu-bar
/// appearance. `emit_palette_refresh` separates lifecycle transitions
/// (which must wake the palette so its visible command list re-fetches)
/// from appearance-only repaints (which would otherwise spam the palette
/// with no-op refreshes — codex plan-review #4).
fn apply_tray_activity(app: &tauri::AppHandle, activity: TrayActivity, emit_palette_refresh: bool) {
    let appearance = current_tray_appearance(app);
    if let Some(tray) = app.tray_by_id("minutes-tray") {
        if let Ok(icon) = tauri::image::Image::from_bytes(activity.icon_bytes(appearance)) {
            tray.set_icon(Some(icon)).ok();
            // Active-state icons (recording/live/dictation) render with
            // template tinting OFF so their colored dot is visible; idle
            // uses the template so the M adopts the menu-bar tint.
            tray.set_icon_as_template(!activity.is_active()).ok();
        }
        tray.set_tooltip(Some(activity.tooltip())).ok();
    }

    // Sync tray menu item enabled state with the lifecycle. Tray callback
    // handlers used to do this on their own immediate paths, but any
    // non-tray-initiated recording (main window, hotkey, CLI, call-detect,
    // palette) bypassed those callbacks and left the menu stuck in its
    // construction-time state. Centralizing here fixes #223 (and the
    // dictation follow-on) and removes a class of race between tray
    // callback post-async-work cleanup and external state updates.
    //
    // No-op + warn if the handles aren't registered (programmer error caught
    // at the first recording state transition — quieter than panic, louder
    // than silent regression).
    match app.try_state::<TrayMenuHandles>() {
        Some(handles) => {
            handles.record.set_enabled(!activity.is_active()).ok();
            handles
                .quick_thought
                .set_enabled(!activity.is_active())
                .ok();
            handles.stop.set_enabled(activity.is_active()).ok();
            handles.stop.set_text(activity.stop_label()).ok();
        }
        None => {
            tracing::warn!(
                "TrayMenuHandles not registered; tray record/stop menu items will not \
                 reflect recording state changes from non-tray sources"
            );
        }
    }

    if emit_palette_refresh {
        // Notify the palette overlay that lifecycle state changed so it
        // can re-fetch its visible command list. The source string lets
        // the palette distinguish recording / live / dictation transitions.
        let _ = app.emit(
            "palette:refresh",
            serde_json::json!({
                "source": activity.palette_source(),
                "active": activity.is_active(),
            }),
        );
    }
}

/// Re-sync the tray (icon, tooltip, menu enabled/labels, palette refresh)
/// from the current AppState lifecycle flags. Callers must mutate
/// recording / live / dictation flags BEFORE invoking this — the function
/// reads, it does not write.
pub fn sync_tray_state(app: &tauri::AppHandle) {
    let snapshot = snapshot_tray_state(app);
    let activity = derive_tray_activity(snapshot);
    apply_tray_activity(app, activity, true);
}

/// Re-paint the tray for an appearance change (system Light/Dark toggle)
/// without re-emitting `palette:refresh`. The lifecycle activity is
/// unchanged; only the icon variant differs. Called from the
/// `WindowEvent::ThemeChanged` listener.
pub fn sync_tray_appearance(app: &tauri::AppHandle) {
    let snapshot = snapshot_tray_state(app);
    let activity = derive_tray_activity(snapshot);
    apply_tray_activity(app, activity, false);
}

// ── Auto-updater ────────────────────────────────────────────

async fn check_for_update(app: &tauri::AppHandle, manual: bool) {
    use tauri_plugin_updater::UpdaterExt;

    let updater = match app.updater() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("[updater] init failed (non-fatal): {}", e);
            if manual {
                commands::show_user_notification(
                    app,
                    "Updates",
                    &format!("Could not initialize the updater: {}", e),
                );
            }
            return;
        }
    };

    let update = match updater.check().await {
        Ok(Some(u)) => u,
        Ok(None) => {
            if manual {
                commands::show_user_notification(app, "Updates", "Minutes is up to date.");
            }
            return;
        }
        Err(e) => {
            eprintln!("[updater] check failed (non-fatal): {}", e);
            if manual {
                commands::show_user_notification(
                    app,
                    "Updates",
                    &format!("Could not check for updates: {}", e),
                );
            }
            return;
        }
    };

    let version = update.version.clone();
    let body = update.body.clone().unwrap_or_default();
    let download_bytes = commands::fetch_update_download_size(&update.download_url).await;
    eprintln!(
        "[updater] v{} available (check only, no download yet)",
        version
    );

    // Store pending update info in AppState
    if let Some(state) = app.try_state::<commands::AppState>() {
        if let Ok(mut pending) = state.pending_update.lock() {
            *pending = Some(commands::PendingUpdate {
                version: version.clone(),
                body: body.clone(),
                download_bytes,
            });
        }

        // Defer notification if any session activity is in progress.
        // The pending_update is stored either way, so it will be surfaced
        // by the 30s deferred poll once the session ends.
        if state.recording.load(Ordering::Relaxed)
            || state.starting.load(Ordering::Relaxed)
            || state.processing.load(Ordering::Relaxed)
            || state.live_transcript_active.load(Ordering::Relaxed)
            || state.dictation_active.load(Ordering::Relaxed)
        {
            eprintln!("[updater] deferring notification (session active)");
            if manual {
                commands::show_user_notification(
                    app,
                    "Update available",
                    &format!(
                        "Minutes {} is ready. Finish the current session and the update banner will appear.",
                        version
                    ),
                );
            }
            return;
        }
    }

    if manual {
        show_main_window(app);
    }
    notify_update_available(app, &version, &body, download_bytes);
}

fn build_app_menu(app: &tauri::AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    #[cfg(target_os = "macos")]
    let app_menu = {
        let about_item =
            MenuItem::with_id(app, "app-show-about", "About Minutes", true, None::<&str>)?;
        let whats_new_item =
            MenuItem::with_id(app, "app-show-whats-new", "What’s New…", true, None::<&str>)?;
        let settings_item =
            MenuItem::with_id(app, "app-open-settings", "Settings…", true, Some("Cmd+,"))?;
        let check_updates_item = MenuItem::with_id(
            app,
            "app-check-for-updates",
            "Check for Updates…",
            true,
            None::<&str>,
        )?;
        let quit_item = MenuItem::with_id(app, "app-quit", "Quit Minutes", true, Some("Cmd+Q"))?;

        SubmenuBuilder::new(app, &app.package_info().name)
            .item(&about_item)
            .item(&whats_new_item)
            .item(&settings_item)
            .item(&check_updates_item)
            .separator()
            .services()
            .separator()
            .hide()
            .hide_others()
            .show_all()
            .separator()
            .item(&quit_item)
            .build()?
    };

    let file_menu = {
        let open_item =
            MenuItem::with_id(app, "app-open-main", "Open Minutes", true, Some("Cmd+O"))?;
        let note_item =
            MenuItem::with_id(app, "app-add-note", "Add Note…", true, Some("Cmd+Shift+N"))?;
        let list_item = MenuItem::with_id(
            app,
            "app-open-meetings-folder",
            "Open Meetings Folder",
            true,
            None::<&str>,
        )?;

        #[cfg(target_os = "macos")]
        {
            SubmenuBuilder::new(app, "File")
                .item(&open_item)
                .separator()
                .item(&note_item)
                .item(&list_item)
                .separator()
                .close_window()
                .build()?
        }

        #[cfg(not(target_os = "macos"))]
        {
            let quit_item = MenuItem::with_id(app, "app-quit", "Quit Minutes", true, None::<&str>)?;

            SubmenuBuilder::new(app, "File")
                .item(&open_item)
                .separator()
                .item(&note_item)
                .item(&list_item)
                .separator()
                .close_window()
                .item(&quit_item)
                .build()?
        }
    };

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;

    #[cfg(target_os = "macos")]
    let view_menu = SubmenuBuilder::new(app, "View").fullscreen().build()?;

    let window_menu = {
        let bring_all_to_front = MenuItem::with_id(
            app,
            "window-bring-all-to-front",
            "Bring All to Front",
            true,
            None::<&str>,
        )?;

        SubmenuBuilder::new(app, "Window")
            .minimize()
            .maximize()
            .separator()
            .item(&bring_all_to_front)
            .close_window()
            .build()?
    };

    let help_menu = {
        let website_item = MenuItem::with_id(
            app,
            "help-open-website",
            "Minutes Website",
            true,
            None::<&str>,
        )?;
        let changelog_item = MenuItem::with_id(
            app,
            "help-open-changelog",
            "Release Notes",
            true,
            None::<&str>,
        )?;
        let discussions_item = MenuItem::with_id(
            app,
            "help-open-discussions",
            "Get Help / Discussions",
            true,
            None::<&str>,
        )?;

        SubmenuBuilder::new(app, "Help")
            .item(&website_item)
            .item(&changelog_item)
            .item(&discussions_item)
            .build()?
    };

    Menu::with_items(
        app,
        &[
            #[cfg(target_os = "macos")]
            &app_menu,
            &file_menu,
            &edit_menu,
            #[cfg(target_os = "macos")]
            &view_menu,
            &window_menu,
            &help_menu,
        ],
    )
}

fn notify_update_available(
    app: &tauri::AppHandle,
    version: &str,
    body: &str,
    download_bytes: Option<u64>,
) {
    let _ = app.emit(
        "update-ready",
        serde_json::json!({
            "version": version,
            "body": body,
            "downloadBytes": download_bytes,
        }),
    );
}

// ── Calendar items in tray menu ──────────────────────────────

const MAX_CALENDAR_ITEMS: usize = 3;
const CALENDAR_REFRESH_SECS: u64 = 60;
const CALENDAR_LOOKAHEAD_MINUTES: u32 = 240; // 4 hours
const MEETING_NOTIFY_MINUTES: i64 = 3; // Show prompt this many minutes before

struct CalendarMenuState {
    items: Vec<MenuItem<tauri::Wry>>,
    separator: Option<MenuItem<tauri::Wry>>,
    /// Event titles we've already sent a notification for (prevents repeat alerts)
    notified: std::collections::HashSet<String>,
}

fn format_calendar_label(event: &minutes_core::calendar::CalendarEvent) -> String {
    if event.minutes_until <= 0 {
        format!("{} · now", event.title)
    } else if event.minutes_until == 1 {
        format!("{} · in 1 min", event.title)
    } else if event.minutes_until >= 60 {
        let h = event.minutes_until / 60;
        let m = event.minutes_until % 60;
        if m == 0 {
            format!("{} · in {}h", event.title, h)
        } else {
            format!("{} · in {}h {}m", event.title, h, m)
        }
    } else {
        format!("{} · in {} min", event.title, event.minutes_until)
    }
}

/// Show a floating overlay prompt for an upcoming meeting.
/// The overlay has "Join & Record" (if URL) or "Record" + "Dismiss" buttons.
fn show_meeting_prompt(app: &tauri::AppHandle, event: &minutes_core::calendar::CalendarEvent) {
    // Don't show if already recording
    if let Some(state) = app.try_state::<commands::AppState>() {
        if state.recording.load(Ordering::Relaxed) {
            return;
        }
    }

    // Close any existing prompt window
    if let Some(win) = app.get_webview_window("meeting-prompt") {
        win.destroy().ok();
    }

    // Stage the payload keyed by a monotonic token. The overlay reads its
    // token from the URL query string and calls `cmd_get_meeting_prompt` to
    // drain exactly its own entry. Keying avoids a race where back-to-back
    // `show_meeting_prompt` calls (two meetings firing in the same
    // `refresh_calendar_items` tick) would let the first overlay's still-in-
    // flight JS consume the second's payload.
    //
    // Why a query string, not a fragment: the previous fragment-based
    // approach tripped over Tauri's URL normalizer double-encoding percent
    // sequences (space → `%20` → `%2520`), so titles with spaces rendered as
    // `X1%20payout`. A bare u64 token has no characters that need encoding.
    static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(0);
    let token = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);

    let Some(state) = app.try_state::<commands::AppState>() else {
        eprintln!("[calendar] AppState missing; skipping meeting prompt");
        return;
    };
    match state.pending_meeting_prompts.lock() {
        Ok(mut map) => {
            // Cap the map to bound memory if some overlay's JS never
            // consumes (e.g. window build failed below, or webview crashed
            // before invoke). Evict lowest token IDs first — they're oldest.
            const MAX_PENDING: usize = 16;
            while map.len() >= MAX_PENDING {
                if let Some(&oldest) = map.keys().min() {
                    map.remove(&oldest);
                } else {
                    break;
                }
            }
            map.insert(
                token,
                commands::MeetingPromptData {
                    title: event.title.clone(),
                    minutes_until: event.minutes_until,
                    url: event.url.clone().filter(|u| !u.is_empty()),
                },
            );
        }
        Err(e) => {
            eprintln!(
                "[calendar] pending_meeting_prompts mutex poisoned, skipping stage: {}",
                e
            );
            return;
        }
    }

    // Position: top-right of main screen, below menu bar
    let (pos_x, pos_y) = get_top_right_position(380.0, 240.0);

    let url = format!("meeting-prompt.html?t={}", token);
    match WebviewWindowBuilder::new(app, "meeting-prompt", WebviewUrl::App(url.into()))
        .title("Upcoming Meeting")
        .inner_size(380.0, 240.0)
        .position(pos_x, pos_y)
        .resizable(false)
        .decorations(false)
        .content_protected(Config::load().privacy.hide_from_screen_share)
        .always_on_top(true)
        .focused(true)
        .skip_taskbar(true)
        .build()
    {
        Ok(_) => eprintln!("[calendar] meeting prompt shown for: {}", event.title),
        Err(e) => {
            eprintln!("[calendar] failed to show meeting prompt: {}", e);
            // Window never opened, so no JS will consume the entry. Drop it
            // now rather than waiting for the MAX_PENDING eviction.
            if let Ok(mut map) = state.pending_meeting_prompts.lock() {
                map.remove(&token);
            }
        }
    }
}

/// Calculate position for top-right placement, 16px from screen edge.
fn get_top_right_position(width: f64, height: f64) -> (f64, f64) {
    let _ = height;
    // Default to a reasonable position; Tauri doesn't expose screen size easily
    // from a non-window context, so we use a heuristic for common displays.
    // The window will be placed at x=screen_width - window_width - 16, y=38 (below menu bar).
    // For a 1440px-wide MacBook display at 2x: logical width ~1440
    // For a 1920px-wide external: logical width ~1920
    // We'll use 1440 as a safe default — the window stays visible on any Mac screen.
    let screen_width = 1440.0;
    let x = screen_width - width - 16.0;
    let y = 38.0; // Below the macOS menu bar
    (x, y)
}

fn refresh_calendar_items(
    app: &tauri::AppHandle,
    menu: &Menu<tauri::Wry>,
    state: &std::sync::Mutex<CalendarMenuState>,
) {
    let mut state = match state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Remove old items from menu
    for item in state.items.drain(..) {
        menu.remove(&item).ok();
    }
    if let Some(sep) = state.separator.take() {
        menu.remove(&sep).ok();
    }

    // Query upcoming events
    let all_events = minutes_core::calendar::upcoming_events(CALENDAR_LOOKAHEAD_MINUTES);
    // When reads are blocked (Add-Only/denied), an empty menu is a lie:
    // say so, with a clickable line that opens the Calendars privacy pane
    // (#300).
    if all_events.is_empty() && !minutes_core::calendar::calendar_access_status().can_read() {
        if let Ok(item) = MenuItem::with_id(
            app,
            "calendar-access-warning",
            "Calendar access limited — reminders off (click to fix)",
            true,
            None::<&str>,
        ) {
            if menu.append(&item).is_ok() {
                state.items.push(item);
            }
        }
    }
    eprintln!(
        "[calendar] queried {} upcoming events ({}min lookahead)",
        all_events.len(),
        CALENDAR_LOOKAHEAD_MINUTES
    );
    for e in &all_events {
        eprintln!("[calendar]   {} — in {} min", e.title, e.minutes_until);
    }
    // Show meeting prompt overlay for meetings starting in ≤ MEETING_NOTIFY_MINUTES (once per event)
    for e in &all_events {
        if e.minutes_until >= 0
            && e.minutes_until <= MEETING_NOTIFY_MINUTES
            && !state.notified.contains(&e.title)
        {
            show_meeting_prompt(app, e);
            state.notified.insert(e.title.clone());
            eprintln!(
                "[calendar] prompted: {} (in {} min)",
                e.title, e.minutes_until
            );
        }
    }

    // Clean up old notifications (events that have passed)
    state.notified.retain(|title| {
        all_events
            .iter()
            .any(|e| &e.title == title && e.minutes_until >= -5)
    });

    let events: Vec<_> = all_events
        .into_iter()
        .filter(|e| e.minutes_until >= 0)
        .take(MAX_CALENDAR_ITEMS)
        .collect();

    if events.is_empty() {
        return;
    }

    // Insert at position 2 (after "Open Minutes" + first separator)
    for (i, event) in events.iter().enumerate() {
        let label = format_calendar_label(event);
        if let Ok(item) = MenuItem::with_id(app, format!("cal-{}", i), &label, true, None::<&str>) {
            if menu.insert(&item, 2 + i).is_ok() {
                state.items.push(item);
            }
        }
    }

    // Separator after calendar items
    if !state.items.is_empty() {
        if let Ok(sep) = MenuItem::with_id(app, "cal-sep", "──────────", false, None::<&str>)
        {
            if menu.insert(&sep, 2 + state.items.len()).is_ok() {
                state.separator = Some(sep);
            }
        }
    }
}

fn should_refresh_meetings_for_paths(paths: &[std::path::PathBuf]) -> bool {
    paths.iter().any(|path| {
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("md"))
            .unwrap_or(false)
    })
}

fn bind_meetings_refresh_watcher(
    watcher: &mut RecommendedWatcher,
    output_dir: &std::path::Path,
) -> Result<(), notify::Error> {
    std::fs::create_dir_all(output_dir)
        .map_err(|error| notify::Error::generic(&error.to_string()))?;
    watcher.watch(output_dir, RecursiveMode::Recursive)
}

fn spawn_meetings_refresh_watcher(app: &tauri::AppHandle, output_dir: std::path::PathBuf) {
    let app_handle = app.clone();
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher: RecommendedWatcher = match notify::recommended_watcher(move |result| {
            let _ = tx.send(result);
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                eprintln!("[meetings-watcher] failed to create watcher: {}", error);
                return;
            }
        };

        let mut watched_output_dir = output_dir;

        if let Err(error) = bind_meetings_refresh_watcher(&mut watcher, &watched_output_dir) {
            eprintln!(
                "[meetings-watcher] failed to watch {}: {}",
                watched_output_dir.display(),
                error
            );
            return;
        }

        loop {
            let configured_output_dir = Config::load().output_dir;
            if configured_output_dir != watched_output_dir {
                if let Err(error) = watcher.unwatch(&watched_output_dir) {
                    eprintln!(
                        "[meetings-watcher] failed to unwatch {}: {}",
                        watched_output_dir.display(),
                        error
                    );
                }
                if let Err(error) =
                    bind_meetings_refresh_watcher(&mut watcher, &configured_output_dir)
                {
                    eprintln!(
                        "[meetings-watcher] failed to rebind {}: {}",
                        configured_output_dir.display(),
                        error
                    );
                } else {
                    watched_output_dir = configured_output_dir;
                    let _ = app_handle.emit("artifacts:changed", ());
                }
            }

            let event = match rx.recv_timeout(std::time::Duration::from_secs(2)) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };

            match event {
                Ok(event) if should_refresh_meetings_for_paths(&event.paths) => {
                    let _ = app_handle.emit("artifacts:changed", ());
                }
                Ok(_) => {}
                Err(error) => {
                    eprintln!("[meetings-watcher] watch error: {}", error);
                }
            }
        }
    });
}

fn main() {
    // Route whisper.cpp + ggml C-level logs through Rust `tracing` so they
    // do not leak to raw stderr. The Tauri menu-bar app records audio in
    // process and runs the same VAD path the CLI does, so the
    // `whisper_vad_detect_speech` flood happens here too without this hook
    // (issue #163). Install BEFORE the early-exit checks below: the
    // process-queue worker (`maybe_run_process_queue_worker`) reaches
    // `pipeline::transcribe_to_artifact` and spins up whisper contexts of
    // its own, so the worker subprocess needs the same routing or it
    // floods stderr while a recording is processing. The Tauri app
    // currently has no `tracing_subscriber` installed; tracing events
    // with no subscriber are dropped, which is the silencing behavior we
    // want for the chatty C INFO logs. If a future change wires a
    // subscriber into this process, set `whisper_rs=warn` and `ggml=warn`
    // in the filter or the flood returns.
    minutes_core::install_whisper_logging_hooks();

    #[cfg(target_os = "macos")]
    if let Some(code) = maybe_run_hotkey_diagnostic() {
        std::process::exit(code);
    }

    if let Some(code) = maybe_run_process_queue_worker() {
        // Worker subprocess exit: skip C++ static teardown on macOS.
        //
        // Letting `main` return (or calling `std::process::exit`) runs
        // `atexit` handlers and C++ static destructors via
        // `__cxa_finalize_ranges`. whisper.cpp / ggml / parakeet helpers
        // register global C++ state whose destructors can call `abort()`
        // on partially-initialized contexts (issue #229: a transcription
        // spawn failure left a context in a bad state, then the normal
        // exit path crashed the worker via SIGABRT inside
        // `__cxa_finalize_ranges`). `_exit` skips that teardown
        // entirely; we drain the sidecar manager explicitly above and
        // rely on the OS to reclaim the rest.
        minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
        exit_process_without_destructors(code);
    }

    // Load with first-run and upgrade migrations so palette defaults
    // stay enabled across upgrades and fresh installs.
    let mut startup_config_snapshot = minutes_core::config::Config::load_with_migrations();
    // Apply the same directory/privacy bootstrap the CLI uses before the
    // desktop app creates logs, scratch audio, or other runtime state.
    if let Err(e) = startup_config_snapshot.ensure_dirs() {
        tracing::warn!(
            "failed to ensure Minutes directories on desktop startup: {}",
            e
        );
    }
    // Auto-heal a stale `recording.device` pin: when the configured
    // input device (USB mixer, Bluetooth headset, virtual loopback) is
    // unplugged before launch, clear it and fall back to the system
    // default. Historically this caused a deterministic crash on
    // record start because the missing-device error reached call sites
    // that aborted the process.
    if minutes_core::capture::auto_heal_missing_recording_device(&mut startup_config_snapshot) {
        if let Err(e) = startup_config_snapshot.save() {
            tracing::warn!(
                "failed to persist auto-healed recording.device clear: {}",
                e
            );
        }
    }
    let _ = secret_store::hydrate_openai_compatible_api_key_env();
    let recording = Arc::new(AtomicBool::new(false));
    let starting = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let processing = Arc::new(AtomicBool::new(false));
    let processing_stage = Arc::new(Mutex::new(None));
    let latest_output = Arc::new(Mutex::new(None));
    let activation_progress = commands::load_activation_progress(&startup_config_snapshot);
    let completion_notifications_enabled = Arc::new(AtomicBool::new(
        startup_config_snapshot.notifications.completion_enabled,
    ));
    let global_hotkey_enabled = Arc::new(AtomicBool::new(
        startup_config_snapshot.global_hotkey.shortcut_enabled,
    ));
    let global_hotkey_shortcut = Arc::new(Mutex::new(
        startup_config_snapshot.global_hotkey.shortcut.clone(),
    ));
    let dictation_shortcut_enabled = Arc::new(AtomicBool::new(false));
    let dictation_shortcut = Arc::new(Mutex::new(
        startup_config_snapshot.dictation.shortcut.clone(),
    ));
    let hotkey_runtime = Arc::new(Mutex::new(commands::HotkeyRuntime::default()));
    let discard_short_hotkey_capture = Arc::new(AtomicBool::new(false));
    let dictation_active = Arc::new(AtomicBool::new(false));
    let dictation_stop_flag = Arc::new(AtomicBool::new(false));
    let live_transcript_active = Arc::new(AtomicBool::new(false));
    let live_transcript_stop_flag = Arc::new(AtomicBool::new(false));
    let screen_share_hidden = Arc::new(AtomicBool::new(
        startup_config_snapshot.privacy.hide_from_screen_share,
    ));
    let palette_shortcut_enabled = Arc::new(AtomicBool::new(false));
    let palette_shortcut = Arc::new(Mutex::new(startup_config_snapshot.palette.shortcut.clone()));
    let palette_lifecycle = Arc::new(Mutex::new(commands::PaletteLifecycle::default()));
    let palette_reopen_pending = Arc::new(AtomicBool::new(false));
    let recording_clone = recording.clone();
    let recording_for_detector = recording.clone();
    let processing_clone = processing.clone();
    let stop_clone = stop_flag.clone();
    let recording_started_by_call_detect = Arc::new(AtomicBool::new(false));
    let call_end_countdown_cancel = Arc::new(AtomicBool::new(false));
    let call_end_countdown_active = Arc::new(AtomicBool::new(false));
    let call_end_countdown_terminal_state = Arc::new(AtomicU8::new(
        commands::CallEndCountdownTerminalState::None as u8,
    ));
    let started_by_call_detect_for_detector = recording_started_by_call_detect.clone();
    let countdown_cancel_for_detector = call_end_countdown_cancel.clone();
    let countdown_active_for_detector = call_end_countdown_active.clone();
    let countdown_terminal_state_for_detector = call_end_countdown_terminal_state.clone();
    let stop_for_detector = stop_flag.clone();

    tauri::Builder::default()
        .menu(build_app_menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "app-show-about" => {
                show_main_window(app);
                let _ = app.emit("minutes://show-about", ());
            }
            "app-show-whats-new" => {
                show_main_window(app);
                let _ = app.emit("minutes://show-whats-new", ());
            }
            "calendar-access-warning" => {
                let _ = std::process::Command::new("open")
                    .arg(
                        "x-apple.systempreferences:com.apple.preference.security?Privacy_Calendars",
                    )
                    .spawn();
            }
            "app-open-settings" => {
                show_main_window(app);
                let _ = app.emit("minutes://show-settings", ());
            }
            "app-check-for-updates" => {
                let handle = app.clone();
                tauri::async_runtime::spawn(async move {
                    check_for_update(&handle, true).await;
                });
            }
            "app-open-main" => {
                show_main_window(app);
            }
            "app-add-note" => {
                show_main_window(app);
                show_note_window(app);
            }
            "app-open-meetings-folder" => {
                let meetings_dir = minutes_core::config::Config::load().output_dir;
                if let Err(err) = commands::open_target(app, &meetings_dir.display().to_string()) {
                    commands::show_user_notification(app, "Meetings", &err);
                }
            }
            "help-open-website" => {
                if let Err(err) = commands::open_target(app, MINUTES_WEBSITE_URL) {
                    commands::show_user_notification(app, "Minutes Website", &err);
                }
            }
            "help-open-changelog" => {
                if let Err(err) = commands::open_target(app, MINUTES_CHANGELOG_URL) {
                    commands::show_user_notification(app, "Release Notes", &err);
                }
            }
            "help-open-discussions" => {
                if let Err(err) = commands::open_target(app, MINUTES_DISCUSSIONS_URL) {
                    commands::show_user_notification(app, "Discussions", &err);
                }
            }
            "window-bring-all-to-front" => {
                let mut windows: Vec<_> = app.webview_windows().into_values().collect();
                windows
                    .sort_by_key(|window| (window.label() != "main", window.label().to_string()));
                for window in &windows {
                    if window.label() == "main" {
                        continue;
                    }
                    let _ = window.unminimize();
                    let _ = window.show();
                }
                show_main_window(app);
            }
            "app-quit" => {
                request_clean_exit(app, 0);
            }
            _ => {}
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    use tauri::Manager;
                    let shortcut_id = shortcut.id();

                    // Try the new unified shortcut manager first.
                    // IMPORTANT: Extract the action under the lock, then execute
                    // AFTER dropping it to avoid deadlock.
                    type UnifiedResult = Option<(
                        shortcut_manager::ShortcutSlot,
                        Option<shortcut_manager::StateMachineAction>,
                        Option<(shortcut_manager::ShortcutSlot, u64)>,
                    )>;
                    let unified_result: UnifiedResult = {
                        if let Some(mgr_state) =
                            app.try_state::<Arc<Mutex<shortcut_manager::ShortcutManager>>>()
                        {
                            if let Ok(mut mgr) = mgr_state.lock() {
                                if let Some(slot) = mgr.find_slot_for_shortcut_id(shortcut_id) {
                                    match event.state() {
                                        tauri_plugin_global_shortcut::ShortcutState::Pressed => {
                                            let hold_info = mgr.handle_press(slot);
                                            Some((slot, None, hold_info))
                                        }
                                        tauri_plugin_global_shortcut::ShortcutState::Released => {
                                            let session_active =
                                                shortcut_manager::is_slot_session_active_fast(
                                                    app, slot,
                                                );
                                            let (_s, action) =
                                                mgr.handle_release(slot, session_active);
                                            Some((slot, Some(action), None))
                                        }
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }; // lock dropped here

                    if let Some((slot, action, hold_info)) = unified_result {
                        if let Some(action) = action {
                            shortcut_manager::execute_action(app, slot, action);
                        }
                        if let Some((slot, generation)) = hold_info {
                            shortcut_manager::schedule_hold_check(app, slot, generation);
                        }
                        return;
                    }

                    // Fall through to legacy handlers for shortcuts registered by old code
                    let state = app.state::<commands::AppState>();
                    let dictation_shortcut_value = state
                        .dictation_shortcut
                        .lock()
                        .ok()
                        .map(|value| value.clone())
                        .unwrap_or_else(|| commands::default_dictation_shortcut().to_string());
                    let dictation_shortcut_id =
                        <tauri_plugin_global_shortcut::Shortcut as std::str::FromStr>::from_str(
                            dictation_shortcut_value.as_str(),
                        )
                        .ok()
                        .map(|shortcut| shortcut.id());
                    let live_shortcut_value = state
                        .live_shortcut
                        .lock()
                        .ok()
                        .map(|value| value.clone())
                        .unwrap_or_else(|| "CmdOrCtrl+Shift+L".to_string());
                    let live_shortcut_id =
                        <tauri_plugin_global_shortcut::Shortcut as std::str::FromStr>::from_str(
                            live_shortcut_value.as_str(),
                        )
                        .ok()
                        .map(|shortcut| shortcut.id());
                    let palette_shortcut_value = state
                        .palette_shortcut
                        .lock()
                        .ok()
                        .map(|value| value.clone())
                        .unwrap_or_else(|| "CmdOrCtrl+Shift+K".to_string());
                    let palette_shortcut_id =
                        <tauri_plugin_global_shortcut::Shortcut as std::str::FromStr>::from_str(
                            palette_shortcut_value.as_str(),
                        )
                        .ok()
                        .map(|shortcut| shortcut.id());

                    if Some(shortcut_id) == dictation_shortcut_id {
                        commands::handle_dictation_shortcut_event(app, event.state());
                    } else if Some(shortcut_id) == live_shortcut_id {
                        commands::handle_live_shortcut_event(app, event.state());
                    } else if Some(shortcut_id) == palette_shortcut_id {
                        commands::handle_palette_shortcut_event(app, event.state());
                    } else {
                        commands::handle_global_hotkey_event(app, event.state());
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filename("window-state.json")
                .skip_initial_state("note")
                .skip_initial_state("meeting-prompt")
                .skip_initial_state("dictation-overlay")
                .build(),
        )
        .manage(commands::AppState {
            recording: recording.clone(),
            starting: starting.clone(),
            stop_flag: stop_flag.clone(),
            processing: processing.clone(),
            processing_stage: processing_stage.clone(),
            latest_output: latest_output.clone(),
            activation_progress: activation_progress.clone(),
            call_capture_health: Arc::new(Mutex::new(None)),
            completion_notifications_enabled: completion_notifications_enabled.clone(),
            screen_share_hidden: screen_share_hidden.clone(),
            global_hotkey_enabled: global_hotkey_enabled.clone(),
            global_hotkey_shortcut: global_hotkey_shortcut.clone(),
            dictation_shortcut_enabled: dictation_shortcut_enabled.clone(),
            dictation_shortcut: dictation_shortcut.clone(),
            hotkey_runtime: hotkey_runtime.clone(),
            discard_short_hotkey_capture: discard_short_hotkey_capture.clone(),
            pty_manager: Arc::new(Mutex::new(pty::PtyManager::default())),
            dictation_active: dictation_active.clone(),
            dictation_stop_flag: dictation_stop_flag.clone(),
            dictation_focus_guard: Arc::new(Mutex::new(None)),
            pending_dictation_target: Arc::new(Mutex::new(None)),
            live_transcript_active: live_transcript_active.clone(),
            live_transcript_stop_flag: live_transcript_stop_flag.clone(),
            live_shortcut_enabled: {
                let cfg = minutes_core::config::Config::load();
                Arc::new(AtomicBool::new(cfg.live_transcript.shortcut_enabled))
            },
            live_shortcut: {
                let cfg = minutes_core::config::Config::load();
                let s = if cfg.live_transcript.shortcut.is_empty() {
                    "CmdOrCtrl+Shift+L".to_string()
                } else {
                    cfg.live_transcript.shortcut.clone()
                };
                Arc::new(Mutex::new(s))
            },
            pending_update: Arc::new(Mutex::new(None)),
            update_install_running: Arc::new(AtomicBool::new(false)),
            update_install_cancel: Arc::new(AtomicBool::new(false)),
            update_install_state: Arc::new(Mutex::new(commands::UpdateUiState::default())),
            palette_shortcut_enabled: palette_shortcut_enabled.clone(),
            palette_shortcut: palette_shortcut.clone(),
            palette_lifecycle: palette_lifecycle.clone(),
            palette_reopen_pending: palette_reopen_pending.clone(),
            pending_meeting_prompts: Arc::new(Mutex::new(HashMap::new())),
            recording_started_by_call_detect: recording_started_by_call_detect.clone(),
            call_end_countdown_cancel: call_end_countdown_cancel.clone(),
            call_end_countdown_active: call_end_countdown_active.clone(),
            call_end_countdown_terminal_state: call_end_countdown_terminal_state.clone(),
        })
        .manage(Arc::new(Mutex::new(
            shortcut_manager::ShortcutManager::new(),
        )))
        .setup(move |app| {
            let initial_recording = minutes_core::pid::status().recording;
            let startup_config = minutes_core::config::Config::load();

            #[cfg(target_os = "macos")]
            install_macos_terminate_hook(app.handle());

            spawn_meetings_refresh_watcher(app.handle(), startup_config.output_dir.clone());

            // Clean up stale terminal workspaces from previous sessions
            context::cleanup_stale_workspaces();

            let debug_update_state =
                std::env::var("MINUTES_DEBUG_UPDATE_STATE")
                    .ok()
                    .or_else(|| {
                        let path = minutes_core::config::Config::minutes_dir()
                            .join("debug-update-state.txt");
                        let value = std::fs::read_to_string(&path)
                            .ok()
                            .map(|s| s.trim().to_string());
                        if value.is_some() {
                            let _ = std::fs::remove_file(path);
                        }
                        value
                    });
            let allow_debug_update_state = app.config().identifier.contains(".dev");

            if allow_debug_update_state {
                if let Some(debug_update_state) = debug_update_state.clone() {
                    let debug_handle = app.handle().clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(2500));
                        if let Err(error) =
                            commands::debug_emit_update_state(&debug_handle, &debug_update_state)
                        {
                            eprintln!(
                                "[updater] debug startup state '{}' failed: {}",
                                debug_update_state, error
                            );
                        }
                    });
                }
            }

            if !allow_debug_update_state {
                // Auto-update: check on launch, then every 6 hours.
                // Check-only (no download). Download starts only when the user
                // accepts the update from the desktop banner.
                // Defers notification if recording/live/dictation is active.
                // Between checks, polls every 30s to surface deferred updates once sessions end.
                //
                // Dev builds (bundle id ending in .dev) skip the auto-update thread entirely:
                // a dev app shouldn't replace itself with a release-channel build, and the
                // auto-check was surfacing release banners (and hardcoded debug versions
                // resurrected from localStorage) that obscured the real UI. Manual "Check
                // for Updates" from the tray/app menu still works for end-to-end testing.
                let update_handle = app.handle().clone();
                std::thread::spawn(move || {
                    const CHECK_INTERVAL_SECS: u64 = 6 * 60 * 60;
                    const DEFERRED_POLL_SECS: u64 = 30;

                    loop {
                        tauri::async_runtime::block_on(check_for_update(&update_handle, false));

                        let polls = CHECK_INTERVAL_SECS / DEFERRED_POLL_SECS;
                        for _ in 0..polls {
                            std::thread::sleep(std::time::Duration::from_secs(DEFERRED_POLL_SECS));
                            commands::surface_deferred_update(&update_handle);
                        }
                    }
                });
            }

            // Preload whisper model for dictation in background thread.
            // Only if dictation shortcuts are enabled — avoids 150MB RAM for
            // users who never use dictation.
            if startup_config.dictation.shortcut_enabled || startup_config.dictation.hotkey_enabled
            {
                let preload_config = startup_config.clone();
                std::thread::spawn(move || {
                    if let Err(e) = minutes_core::dictation::preload_model(&preload_config) {
                        eprintln!("[dictation] model preload failed (non-fatal): {}", e);
                    }
                });
            }

            // Create main window on launch
            commands::seed_latest_retryable_output(&latest_output);
            show_main_window(app.handle());
            commands::spawn_permission_monitor(app.handle().clone());

            if minutes_core::jobs::active_job_count() > 0 {
                commands::spawn_processing_worker(
                    app.handle().clone(),
                    processing.clone(),
                    processing_stage.clone(),
                    latest_output.clone(),
                    activation_progress.clone(),
                    completion_notifications_enabled.clone(),
                );
            }

            // Restore dictation shortcut via the unified ShortcutManager.
            // This replaces the old dual-path (legacy hotkey + legacy standard shortcut).
            {
                let cfg = &startup_config;
                let app_handle = app.handle().clone();
                if cfg.dictation.hotkey_enabled || cfg.dictation.shortcut_enabled {
                    let (shortcut, keycode) = if cfg.dictation.hotkey_enabled {
                        let kc = cfg.dictation.hotkey_keycode;
                        let label = if kc == 57 {
                            "CapsLock"
                        } else if kc == 63 {
                            "fn"
                        } else {
                            "CapsLock"
                        };
                        (label.to_string(), kc)
                    } else {
                        (cfg.dictation.shortcut.clone(), -1i64)
                    };
                    let register_result = {
                        let mgr_state =
                            app_handle.state::<Arc<Mutex<shortcut_manager::ShortcutManager>>>();
                        let mut mgr = match mgr_state.lock() {
                            Ok(mgr) => mgr,
                            Err(_) => {
                                eprintln!("[shortcut_manager] mutex poisoned at startup");
                                return Ok(());
                            }
                        };
                        mgr.register(
                            shortcut_manager::ShortcutSlot::Dictation,
                            shortcut.clone(),
                            keycode,
                            &app_handle,
                        )
                    };
                    match register_result {
                        Ok(_) => {
                            dictation_shortcut_enabled.store(true, Ordering::Relaxed);
                            if let Ok(mut current) = dictation_shortcut.lock() {
                                *current = shortcut;
                            }
                        }
                        Err(e) => {
                            eprintln!("[shortcut_manager] startup restore dictation failed: {}", e);
                        }
                    }
                }
            }

            // Restore live transcript shortcut from config
            if startup_config.live_transcript.shortcut_enabled {
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                let shortcut = if startup_config.live_transcript.shortcut.is_empty() {
                    "CmdOrCtrl+Shift+L".to_string()
                } else {
                    startup_config.live_transcript.shortcut.clone()
                };
                if let Err(e) = app.global_shortcut().register(shortcut.as_str()) {
                    eprintln!("[live-shortcut] startup restore failed: {}", e);
                } else {
                    let state = app.state::<commands::AppState>();
                    state.live_shortcut_enabled.store(true, Ordering::Relaxed);
                    if let Ok(mut current) = state.live_shortcut.lock() {
                        *current = shortcut;
                    };
                }
            }

            // Register the palette shortcut if the config opts into it.
            if startup_config.palette.shortcut_enabled {
                use tauri_plugin_global_shortcut::GlobalShortcutExt;
                let shortcut = if startup_config.palette.shortcut.is_empty() {
                    "CmdOrCtrl+Shift+K".to_string()
                } else {
                    startup_config.palette.shortcut.clone()
                };
                if let Err(e) = app.global_shortcut().register(shortcut.as_str()) {
                    eprintln!(
                        "[palette-shortcut] startup register failed ({}): {}",
                        shortcut, e
                    );
                } else {
                    let state = app.state::<commands::AppState>();
                    state
                        .palette_shortcut_enabled
                        .store(true, Ordering::Relaxed);
                    if let Ok(mut current) = state.palette_shortcut.lock() {
                        *current = shortcut;
                    };
                }
            }

            commands::maybe_show_palette_first_run_notice(app.handle());

            // Calendar state for dynamic tray menu items
            let cal_state = Arc::new(std::sync::Mutex::new(CalendarMenuState {
                items: Vec::new(),
                separator: None,
                notified: std::collections::HashSet::new(),
            }));

            // Tray menu
            let open_item = MenuItem::with_id(app, "open", "Open Minutes", true, None::<&str>)?;
            let sep0 = MenuItem::with_id(app, "sep0", "──────────", false, None::<&str>)?;
            let record_item = MenuItem::with_id(
                app,
                "record",
                "Start Recording",
                !initial_recording,
                None::<&str>,
            )?;
            let record_item_ref = record_item.clone();
            let quick_thought_item = MenuItem::with_id(
                app,
                "quick-thought",
                "Quick Thought",
                !initial_recording,
                None::<&str>,
            )?;
            let quick_thought_item_ref = quick_thought_item.clone();
            let sensitive_item =
                MenuItem::with_id(app, "sensitive", "Sensitive Meeting", true, None::<&str>)?;
            let sensitive_item_ref = sensitive_item.clone();
            let stop_item = MenuItem::with_id(
                app,
                "stop",
                "Stop Recording",
                initial_recording,
                None::<&str>,
            )?;
            let stop_item_ref = stop_item.clone();
            // Enabled regardless of recording state: toggling when no recording
            // is active primes the sentinel so the NEXT dual-source recording
            // starts muted. During a recording, the toggle takes effect on the
            // next loop iteration of record_to_wav_dual_source.
            let initial_mic_muted = minutes_core::streaming::is_mic_muted();
            let mic_mute_item = MenuItem::with_id(
                app,
                "mic-mute-toggle",
                if initial_mic_muted {
                    "Mute My Mic (Recording Only) ✓"
                } else {
                    "Mute My Mic (Recording Only)"
                },
                true,
                None::<&str>,
            )?;
            let mic_mute_item_ref = mic_mute_item.clone();
            let sep = MenuItem::with_id(app, "sep1", "──────────", false, None::<&str>)?;
            let note_item = MenuItem::with_id(app, "note", "Add Note...", true, None::<&str>)?;
            let list_item =
                MenuItem::with_id(app, "list", "Open Meetings Folder", true, None::<&str>)?;
            let paste_summary_item = MenuItem::with_id(
                app,
                "paste-summary",
                "Copy Latest Summary",
                true,
                None::<&str>,
            )?;
            let paste_transcript_item = MenuItem::with_id(
                app,
                "paste-transcript",
                "Copy Latest Transcript",
                true,
                None::<&str>,
            )?;
            let assistant_item = MenuItem::with_id(app, "assistant", "Recall", true, None::<&str>)?;
            let screen_share_item = MenuItem::with_id(
                app,
                "screen-share-toggle",
                if startup_config.privacy.hide_from_screen_share {
                    "Hide from Screen Share ✓"
                } else {
                    "Hide from Screen Share"
                },
                true,
                None::<&str>,
            )?;
            let screen_share_item_ref = screen_share_item.clone();
            let check_update_item = MenuItem::with_id(
                app,
                "check-for-updates",
                "Check for Updates",
                true,
                None::<&str>,
            )?;
            let sep2 = MenuItem::with_id(app, "sep2", "──────────", false, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Minutes", true, None::<&str>)?;

            let menu = Menu::new(app)?;
            menu.append_items(&[
                &open_item,
                &sep0,
                &record_item,
                &quick_thought_item,
                &sensitive_item,
                &stop_item,
                &mic_mute_item,
                &sep,
                &note_item,
                &assistant_item,
                &list_item,
            ])?;
            if commands::supports_tray_artifact_copy() {
                menu.append_items(&[&paste_summary_item, &paste_transcript_item])?;
            }
            menu.append_items(&[&sep2, &screen_share_item, &check_update_item, &quit_item])?;

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon-tray.png"))
                .expect("load tray icon");

            let _tray = TrayIconBuilder::with_id("minutes-tray")
                .icon(icon)
                .icon_as_template(true)
                .menu(&menu)
                .tooltip("Minutes")
                .on_menu_event(move |app, event| {
                    let recording = recording_clone.clone();
                    let stop = stop_clone.clone();
                    let rec_item = record_item_ref.clone();
                    let quick_item = quick_thought_item_ref.clone();
                    let sensitive_item_ref = sensitive_item_ref.clone();
                    let stp_item = stop_item_ref.clone();
                    let screen_share_hidden = screen_share_hidden.clone();
                    let screen_share_item_ref = screen_share_item_ref.clone();
                    let mic_mute_item_ref = mic_mute_item_ref.clone();
                    match event.id.as_ref() {
                        "open" => {
                            show_main_window(app);
                        }
                        "record" => {
                            let app_state = app.state::<commands::AppState>();
                            // Guard double-click: a fast second click before
                            // recording_active flips spawns a redundant
                            // wrapper that resets transitional text. The
                            // `state.starting` flag covers the window
                            // between launch_recording acquiring the slot
                            // (commands.rs:4576-4583) and the recording flag
                            // actually flipping (codex diff-review attack G).
                            if commands::recording_active(&recording)
                                || app_state.starting.load(Ordering::Relaxed)
                            {
                                return;
                            }
                            show_main_window(app);
                            app.emit("recording:start-requested", serde_json::json!({}))
                                .ok();
                        }
                        "quick-thought" => {
                            let app_state = app.state::<commands::AppState>();
                            if commands::recording_active(&recording)
                                || app_state.starting.load(Ordering::Relaxed)
                            {
                                return;
                            }
                            // Transitional text only; see "record" arm for full rationale.
                            quick_item.set_text("Starting Quick Thought…").ok();
                            let app_handle = app.clone();
                            let ri = rec_item.clone();
                            let qi = quick_item.clone();
                            std::thread::spawn(move || {
                                let app_for_launch = app_handle.clone();
                                let state = app_handle.state::<commands::AppState>();
                                let _ = commands::launch_recording(
                                    app_for_launch,
                                    &state,
                                    minutes_core::CaptureMode::QuickThought,
                                    Some(minutes_core::capture::RecordingIntent::Memo),
                                    false,
                                    None,
                                    None,
                                    None,
                                    None,
                                );
                                ri.set_text("Start Recording").ok();
                                qi.set_text("Quick Thought").ok();
                            });
                        }
                        "stop" => {
                            // The Stop menu item is enabled whenever ANY
                            // recording-class state is active: recording
                            // proper, live transcript, OR dictation. Route to
                            // the active flow. Each has a distinct stop path:
                            //   - recording → `commands::request_stop`
                            //   - live      → `commands::cmd_stop_live_transcript`
                            //   - dictation → `commands::cmd_stop_dictation`
                            // Priority order matches `derive_tray_activity`
                            // (Recording > Live > Dictation) so behavior is
                            // deterministic if two flags are somehow both
                            // true.
                            let state = app.state::<commands::AppState>();
                            let recording_was_active = commands::recording_active(&recording);
                            let live_active = state.live_transcript_active.load(Ordering::Relaxed);
                            let dictation_was_active =
                                state.dictation_active.load(Ordering::Relaxed);

                            let stop_ok = if recording_was_active {
                                commands::request_stop(&recording, &stop).is_ok()
                            } else if live_active {
                                commands::cmd_stop_live_transcript(state).is_ok()
                            } else if dictation_was_active {
                                commands::cmd_stop_dictation(state).is_ok()
                            } else {
                                false
                            };
                            if !stop_ok {
                                // Nothing active to stop, or stop failed; bail.
                                return;
                            }

                            // Transitional text only; see comment in "record"
                            // arm. Disabling stop immediately to prevent
                            // double-click is intentional and stays here
                            // (transient UX affordance, separate from
                            // steady-state sync). The steady-state sync that
                            // re-renders the idle label fires when the
                            // active session's RAII guard drops and triggers
                            // `sync_tray_state` (Live/DictationActiveGuard).
                            rec_item.set_text("Stopping...").ok();
                            quick_item.set_text("Quick Thought").ok();
                            stp_item.set_enabled(false).ok();
                            let app_done = app.clone();
                            let ri = rec_item.clone();
                            let qi = quick_item.clone();
                            std::thread::spawn(move || {
                                if recording_was_active {
                                    if commands::wait_for_recording_shutdown(
                                        std::time::Duration::from_secs(120),
                                    ) {
                                        ri.set_text("Start Recording").ok();
                                        qi.set_text("Quick Thought").ok();
                                        sync_tray_state(&app_done);
                                    }
                                } else {
                                    // Live transcript / dictation paths:
                                    // both stop calls just flip a flag; the
                                    // session's own guard (LiveActiveGuard /
                                    // DictationActiveGuard) calls
                                    // sync_tray_state when it tears down.
                                    // Just reset transitional text here; do
                                    // NOT call sync_tray_state ourselves
                                    // (would race against the still-true
                                    // flag and re-enable Stop — codex attack
                                    // #1 generalized to all flag-based stop
                                    // paths).
                                    ri.set_text("Start Recording").ok();
                                    qi.set_text("Quick Thought").ok();
                                }
                            });
                        }
                        "mic-mute-toggle" => {
                            let new_state =
                                minutes_core::streaming::toggle_mic_mute_with_sentinel();
                            let label = if new_state {
                                "Mute My Mic (Recording Only) ✓"
                            } else {
                                "Mute My Mic (Recording Only)"
                            };
                            mic_mute_item_ref.set_text(label).ok();
                        }
                        "sensitive" => {
                            if minutes_core::sensitive::is_active() {
                                let app_handle = app.clone();
                                let state = app.state::<commands::AppState>();
                                match commands::cmd_sensitive_stop(app_handle, state) {
                                    Ok(result) => {
                                        sensitive_item_ref.set_text("Sensitive Meeting").ok();
                                        commands::show_user_notification(
                                            app,
                                            "Sensitive meeting saved",
                                            result
                                                .get("path")
                                                .and_then(|value| value.as_str())
                                                .unwrap_or("Saved."),
                                        );
                                    }
                                    Err(err) => commands::show_user_notification(
                                        app,
                                        "Sensitive meeting",
                                        &err,
                                    ),
                                }
                            } else {
                                match commands::cmd_sensitive_start(None) {
                                    Ok(result) => {
                                        sensitive_item_ref.set_text("Stop Sensitive Meeting").ok();
                                        commands::show_user_notification(
                                            app,
                                            "Sensitive meeting started",
                                            result
                                                .get("title")
                                                .and_then(|value| value.as_str())
                                                .unwrap_or("Sensitive meeting"),
                                        );
                                    }
                                    Err(err) => commands::show_user_notification(
                                        app,
                                        "Sensitive meeting",
                                        &err,
                                    ),
                                }
                            }
                        }
                        "note" => {
                            show_note_window(app);
                        }
                        "assistant" => {
                            let pty_mgr = app.state::<commands::AppState>().pty_manager.clone();
                            let app_handle = app.clone();
                            std::thread::spawn(move || {
                                if let Err(err) = commands::spawn_terminal(
                                    &app_handle,
                                    &pty_mgr,
                                    "assistant",
                                    None,
                                    None,
                                ) {
                                    commands::show_user_notification(
                                        &app_handle,
                                        "AI Assistant",
                                        &err,
                                    );
                                }
                            });
                        }
                        "list" => {
                            let meetings_dir = minutes_core::config::Config::load().output_dir;
                            if let Err(err) =
                                commands::open_target(app, &meetings_dir.display().to_string())
                            {
                                commands::show_user_notification(app, "Meetings", &err);
                            }
                        }
                        "paste-summary" | "paste-transcript" => {
                            let target_app = commands::frontmost_application_name();
                            let kind = if event.id.as_ref() == "paste-summary" {
                                "summary"
                            } else {
                                "transcript"
                            };
                            match commands::paste_latest_artifact(
                                &latest_output,
                                kind,
                                target_app.as_deref(),
                            ) {
                                Ok(message) => {
                                    commands::show_user_notification(
                                        app,
                                        &format!("Latest {}", kind),
                                        &message,
                                    );
                                }
                                Err(err) => {
                                    commands::show_user_notification(
                                        app,
                                        &format!("Latest {}", kind),
                                        &err,
                                    );
                                }
                            }
                        }
                        "screen-share-toggle" => {
                            let currently_hidden = screen_share_hidden.load(Ordering::Relaxed);
                            let new_state = !currently_hidden;
                            screen_share_hidden.store(new_state, Ordering::Relaxed);

                            let mut config = Config::load();
                            config.privacy.hide_from_screen_share = new_state;
                            if let Err(err) = config.save() {
                                eprintln!("[privacy] failed to save screen-share setting: {}", err);
                            }

                            // Update menu label
                            if new_state {
                                screen_share_item_ref
                                    .set_text("Hide from Screen Share ✓")
                                    .ok();
                            } else {
                                screen_share_item_ref
                                    .set_text("Hide from Screen Share")
                                    .ok();
                            }

                            // Apply to all existing windows
                            for (_, win) in app.webview_windows() {
                                win.set_content_protected(new_state).ok();
                            }
                        }
                        "check-for-updates" => {
                            let handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                check_for_update(&handle, true).await;
                            });
                        }
                        "quit" => {
                            request_clean_exit(app, 0);
                        }
                        // Calendar event items — start recording on click.
                        // Mirrors the "record" arm exactly; enabled-state flips
                        // flow through sync_tray_state, not local set_enabled
                        // calls (issue #223 / codex diff-review attack #8).
                        "cal-0" | "cal-1" | "cal-2" => {
                            let app_state = app.state::<commands::AppState>();
                            if commands::recording_active(&recording)
                                || app_state.starting.load(Ordering::Relaxed)
                            {
                                return;
                            }
                            show_main_window(app);
                            app.emit("recording:start-requested", serde_json::json!({}))
                                .ok();
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            // Register tray menu handles BEFORE the initial state sync below
            // (and before any commands.rs entry-point can fire sync_tray_state).
            // Issue #223: without these handles wired up, the record / stop
            // menu items are frozen in their construction-time enabled state.
            app.manage(TrayMenuHandles {
                record: record_item.clone(),
                quick_thought: quick_thought_item.clone(),
                stop: stop_item.clone(),
            });

            // Seed the appearance state from the main window's theme. The
            // main window is built earlier in setup() via `show_main_window`,
            // so `.theme()` returns the current system Aqua/DarkAqua choice
            // here. If the query errors, default to Light — the active-state
            // icons were originally designed for light menu bars (commit
            // 2c9d26d). After this seed the `WindowEvent::ThemeChanged`
            // listener in the run loop keeps the state updated and triggers
            // `sync_tray_appearance` so the icon variant tracks the system.
            let initial_appearance = app
                .get_webview_window("main")
                .and_then(|w| w.theme().ok())
                .map(TrayAppearance::from_theme)
                .unwrap_or(TrayAppearance::Light);
            app.manage(TrayAppearanceState::new(initial_appearance));

            // `sync_tray_state` reads the PID-aware `recording_active`
            // helper and the in-app live/dictation flags, so an external
            // CLI recording active at app launch keeps the tray rendering
            // as recording (and the menu items reflecting it).
            sync_tray_state(app.handle());

            // Start call detection background loop
            if commands::supports_call_detection() {
                let config = minutes_core::config::Config::load();
                let detector = Arc::new(call_detect::CallDetector::new(config.call_detection));
                detector.start(
                    app.handle().clone(),
                    recording_for_detector,
                    dictation_active.clone(),
                    live_transcript_active.clone(),
                    processing_clone,
                    call_detect::CallEndAutoStopHandles {
                        recording_started_by_call_detect: started_by_call_detect_for_detector,
                        countdown_cancel: countdown_cancel_for_detector,
                        countdown_active: countdown_active_for_detector,
                        countdown_terminal_state: countdown_terminal_state_for_detector,
                        stop_flag: stop_for_detector,
                    },
                );
            }

            let app_control = app.handle().clone();
            std::thread::spawn(move || loop {
                let status = minutes_core::desktop_control::DesktopAppStatus {
                    pid: std::process::id(),
                    updated_at: chrono::Local::now(),
                    platform: std::env::consts::OS.into(),
                };
                minutes_core::desktop_control::write_desktop_app_status(&status).ok();

                let pending = minutes_core::desktop_control::claim_pending_requests(
                    &std::process::id().to_string(),
                );
                if !pending.is_empty() {
                    let state = app_control.state::<commands::AppState>();
                    for claimed in pending {
                        let response = commands::handle_desktop_control_request(
                            app_control.clone(),
                            &state,
                            claimed.request.clone(),
                        );
                        minutes_core::desktop_control::write_response(&response).ok();
                        minutes_core::desktop_control::finish_claimed_request(&claimed.claim_path)
                            .ok();
                    }
                }

                std::thread::sleep(std::time::Duration::from_secs(2));
            });

            // Calendar items in tray menu — refresh every minute
            // Delay first refresh so the app window is interactive before
            // osascript Calendar queries block the main-thread menu updates.
            if commands::supports_calendar_integration() && startup_config.calendar.enabled {
                let app_cal = app.handle().clone();
                let menu_cal = menu.clone();
                let cal_timer = cal_state.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    let mut consecutive_timeouts: u32 = 0;
                    let mut backoff_secs: u64 = 300; // starts at 5 min
                    loop {
                        // Circuit breaker: back off with escalating delays.
                        // Calendar.app can hang on CalDAV sync or TCC prompts.
                        // After 2 failures, back off. Each cycle doubles the
                        // backoff (5 min → 10 min → 20 min, capped at 30 min).
                        if consecutive_timeouts >= 2 {
                            eprintln!(
                                "[calendar] {} consecutive timeouts, backing off {}s",
                                consecutive_timeouts, backoff_secs
                            );
                            std::thread::sleep(std::time::Duration::from_secs(backoff_secs));
                            backoff_secs = (backoff_secs * 2).min(1800); // cap at 30 min
                                                                         // Don't reset counter — one more failure keeps escalating
                        }

                        let start = std::time::Instant::now();
                        refresh_calendar_items(&app_cal, &menu_cal, &cal_timer);
                        let elapsed = start.elapsed();

                        // Subprocess timeout is 3s; anything over 2s means Calendar.app
                        // is unhealthy or hung.
                        if elapsed >= std::time::Duration::from_secs(2) {
                            consecutive_timeouts += 1;
                        } else {
                            consecutive_timeouts = 0;
                            backoff_secs = 300; // reset backoff on success
                        }

                        std::thread::sleep(std::time::Duration::from_secs(CALENDAR_REFRESH_SECS));
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                    // Hide main window on close instead of quitting (app stays in tray)
                    // PTY session persists — user can reopen and resume where they left off
                    api.prevent_close();
                    window.hide().ok();
                }
                tauri::WindowEvent::Focused(false) if window.label() == "palette" => {
                    let app_handle = window.app_handle().clone();
                    let state = app_handle.state::<commands::AppState>();
                    let is_open = match state.palette_lifecycle.lock() {
                        Ok(guard) => *guard == commands::PaletteLifecycle::Open,
                        Err(poisoned) => *poisoned.into_inner() == commands::PaletteLifecycle::Open,
                    };
                    if is_open {
                        commands::close_palette_window(&app_handle);
                    }
                }
                // Track macOS system appearance changes via the main
                // window's ThemeChanged event. Tao registers an
                // `AppleInterfaceThemeChangedNotification` observer on
                // the window delegate and fires this whenever the cached
                // theme flips. The event fires on hidden windows too
                // (no visibility check in the upstream observer), so
                // the menu-bar-only state still gets the update after
                // the user closes the main window. Filter to "main" so
                // we only sync once per system flip (the secondary
                // windows would otherwise re-fire the same event).
                tauri::WindowEvent::ThemeChanged(theme) if window.label() == "main" => {
                    let app_handle = window.app_handle().clone();
                    if let Some(state) = app_handle.try_state::<TrayAppearanceState>() {
                        state.set(TrayAppearance::from_theme(*theme));
                    }
                    // Re-paint the tray icon for the new appearance; do
                    // NOT emit `palette:refresh` (codex plan-review #4 —
                    // appearance changes are not lifecycle transitions).
                    sync_tray_appearance(&app_handle);
                }
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::cmd_capture_status,
            commands::cmd_status,
            commands::cmd_processing_jobs,
            commands::cmd_list_meetings,
            commands::cmd_search,
            commands::cmd_add_note,
            commands::cmd_sensitive_start,
            commands::cmd_sensitive_stop,
            commands::cmd_run_meeting_debrief,
            commands::cmd_toggle_palette,
            commands::cmd_start_recording,
            commands::cmd_stop_recording,
            commands::cmd_cancel_call_end_countdown,
            commands::cmd_extend_recording,
            commands::cmd_toggle_mic_mute,
            commands::cmd_mic_mute_state,
            commands::cmd_open_file,
            commands::cmd_read_text_file,
            commands::cmd_get_text_file_access,
            commands::cmd_get_text_file_review,
            commands::cmd_recent_artifacts,
            commands::cmd_get_recall_workspace_state,
            commands::cmd_set_recall_workspace_state,
            commands::cmd_write_text_file,
            commands::cmd_restore_text_file_snapshot,
            commands::cmd_promote_text_file_to_artifact,
            commands::cmd_create_artifact_from_meeting,
            commands::cmd_set_open_artifact,
            commands::cmd_clear_open_artifact,
            commands::cmd_clear_latest_output,
            commands::cmd_set_completion_notifications,
            commands::cmd_global_hotkey_settings,
            commands::cmd_set_global_hotkey,
            commands::cmd_dictation_shortcut_settings,
            commands::cmd_set_dictation_shortcut,
            commands::cmd_desktop_capabilities,
            commands::cmd_permission_center,
            commands::cmd_macos_permission_rows,
            commands::cmd_permission_restart_safety,
            commands::cmd_restart_for_permission,
            commands::cmd_recovery_items,
            commands::cmd_retry_all_recovery,
            commands::cmd_retry_recovery,
            commands::cmd_retry_processing_job,
            commands::cmd_weekly_summary,
            commands::cmd_proactive_context_bundle,
            commands::cmd_list_devices,
            commands::cmd_delete_meeting,
            commands::cmd_get_meeting_detail,
            commands::cmd_list_voices,
            commands::cmd_confirm_speaker,
            commands::cmd_remember_vocabulary_person,
            commands::cmd_needs_setup,
            commands::cmd_download_model,
            commands::cmd_mark_activation_nudge_shown,
            cmd_show_main_window,
            cmd_apply_recall_window_layout,
            cmd_scale_window,
            commands::cmd_upcoming_meetings,
            commands::cmd_spawn_terminal,
            commands::cmd_pty_input,
            commands::cmd_pty_resize,
            commands::cmd_pty_kill,
            commands::cmd_list_agents,
            commands::cmd_terminal_info,
            commands::cmd_get_settings,
            commands::cmd_warm_parakeet,
            commands::cmd_openai_compatible_secret_status,
            commands::cmd_set_openai_compatible_api_key,
            commands::cmd_clear_openai_compatible_api_key,
            commands::cmd_set_setting,
            commands::cmd_set_screen_share_hidden,
            commands::cmd_get_autostart,
            commands::cmd_set_autostart,
            commands::cmd_get_storage_stats,
            commands::cmd_vault_status,
            commands::cmd_vault_setup,
            commands::cmd_vault_unlink,
            commands::cmd_open_meeting_url,
            commands::cmd_get_meeting_prompt,
            commands::cmd_close_meeting_prompt,
            commands::cmd_start_dictation,
            commands::cmd_stop_dictation,
            commands::cmd_dismiss_dictation_overlay,
            commands::cmd_recent_dictations,
            commands::cmd_copy_dictation,
            commands::cmd_repaste_dictation,
            commands::cmd_set_shortcut,
            commands::cmd_shortcut_status,
            commands::cmd_suspend_shortcut,
            commands::cmd_probe_shortcut,
            commands::cmd_start_live_transcript,
            commands::cmd_stop_live_transcript,
            commands::cmd_live_transcript_status,
            commands::cmd_live_shortcut_settings,
            commands::cmd_set_live_shortcut,
            commands::cmd_install_update,
            commands::cmd_cancel_update_install,
            commands::cmd_debug_simulate_update,
            #[cfg(target_os = "macos")]
            cli_setup::cmd_cli_install_state,
            #[cfg(target_os = "macos")]
            cli_setup::cmd_cli_setup_run,
            #[cfg(target_os = "macos")]
            cli_setup::cmd_cli_snooze,
            #[cfg(target_os = "macos")]
            cli_setup::cmd_cli_recheck,
            #[cfg(target_os = "macos")]
            cli_setup::cmd_cli_clear_quarantine,
            commands::cmd_check_whats_new,
            commands::cmd_get_whats_new,
            commands::cmd_dismiss_whats_new,
            commands::palette_close,
            commands::palette_current_meeting,
            commands::cmd_palette_settings,
            commands::cmd_set_palette_shortcut,
            palette_dispatch::palette_list,
            palette_dispatch::palette_execute,
        ])
        .build(tauri::generate_context!())
        .expect("error while building minutes app")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { code, api, .. } => {
                if code == Some(tauri::RESTART_EXIT_CODE) {
                    cleanup_before_process_exit(app);
                } else {
                    api.prevent_exit();
                    request_clean_exit(app, code.unwrap_or(0));
                }
            }
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => {
                show_main_window(app);
            }
            tauri::RunEvent::Exit => cleanup_before_process_exit(app),
            _ => {}
        });
}

#[cfg(test)]
mod tray_activity_tests {
    use super::{
        derive_tray_activity, TrayActivity, TrayAppearance, TrayStateSnapshot,
        MAIN_WINDOW_TRANSPARENT,
    };

    fn snap(recording: bool, live: bool, dictation: bool) -> TrayStateSnapshot {
        TrayStateSnapshot {
            recording,
            live,
            dictation,
        }
    }

    #[test]
    fn main_window_uses_opaque_webview_for_hidden_tray_lifecycle() {
        const {
            assert!(
                !MAIN_WINDOW_TRANSPARENT,
                "the long-lived main WebView is hidden/shown from the tray; keep it opaque"
            );
        }

        #[cfg(target_os = "macos")]
        const {
            assert!(
                !super::MAIN_WINDOW_APPLY_VIBRANCY,
                "macOS vibrancy on the hidden main WebView can crash WebKit during frame updates"
            );
        }
    }

    #[test]
    fn show_main_window_restores_minimized_window_before_focus() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let main_rs =
            std::fs::read_to_string(format!("{}/src/main.rs", manifest)).expect("read main.rs");
        let restore_idx = main_rs
            .find("win.unminimize().ok()")
            .expect("main-window reveal should restore minimized windows");
        let focus_idx = main_rs
            .find("win.set_focus().ok()")
            .expect("main-window reveal should focus the window");

        assert!(
            restore_idx < focus_idx,
            "main-window reveal should unminimize before requesting focus"
        );
    }

    #[test]
    fn meeting_prompt_dismiss_uses_native_destroy_path() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let main_rs = std::fs::read_to_string(format!("{}/src/main.rs", manifest))
            .expect("failed to read main.rs");
        let prompt_html =
            std::fs::read_to_string(format!("{}/../src/meeting-prompt.html", manifest))
                .expect("failed to read meeting-prompt.html");

        assert!(
            main_rs.contains("commands::cmd_close_meeting_prompt"),
            "meeting prompt close command must be registered with Tauri"
        );
        assert!(
            main_rs.contains("win.destroy().ok()"),
            "replacing an existing meeting prompt should destroy, not close, the old WebView"
        );
        assert!(
            prompt_html.contains("cmd_close_meeting_prompt"),
            "meeting prompt dismiss must route through native destroy"
        );
        assert!(
            !prompt_html.contains("getCurrentWebviewWindow().close()"),
            "direct JS WebView close can crash WebKit during prompt frame updates"
        );
    }

    #[test]
    fn recall_layout_uses_native_guarded_command() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let main_rs = std::fs::read_to_string(format!("{}/src/main.rs", manifest))
            .expect("failed to read main.rs");
        let index_html = std::fs::read_to_string(format!("{}/../src/index.html", manifest))
            .expect("failed to read index.html");
        let capabilities =
            std::fs::read_to_string(format!("{}/capabilities/default.json", manifest))
                .expect("failed to read default capability");

        assert!(
            main_rs.contains("cmd_apply_recall_window_layout"),
            "Recall window layout changes should route through a native command"
        );
        assert!(
            main_rs.contains("FRAME_EPSILON") && main_rs.contains("win.outer_size()"),
            "Recall native layout should skip no-op frame size updates"
        );
        assert!(
            main_rs.contains("(shift_x - logical_x).abs() > FRAME_EPSILON"),
            "Recall native layout should skip no-op frame position updates"
        );
        assert!(
            index_html.contains("cmd_apply_recall_window_layout"),
            "Recall frontend should call the guarded native layout command"
        );
        assert!(
            !index_html.contains("currentWindow.setSize("),
            "direct JS setSize on the long-lived main WebView can crash WebKit during frame updates"
        );
        assert!(
            !index_html.contains("currentWindow.setPosition("),
            "direct JS setPosition on the long-lived main WebView can crash WebKit during frame updates"
        );
        for permission in [
            "core:window:allow-set-focus",
            "core:window:allow-set-size",
            "core:window:allow-set-position",
            "core:window:allow-outer-position",
            "core:window:allow-outer-size",
        ] {
            assert!(
                !capabilities.contains(permission),
                "frontend windows should not have {} while native commands own main-window frame updates",
                permission
            );
        }
    }

    #[test]
    fn dictation_overlay_lifecycle_uses_destroy_for_same_label_rebuilds() {
        let manifest = env!("CARGO_MANIFEST_DIR");
        let commands_rs = std::fs::read_to_string(format!("{}/src/commands.rs", manifest))
            .expect("failed to read commands.rs");

        assert!(
            commands_rs.contains("get_webview_window(\"dictation-overlay\")")
                && commands_rs.contains("win.destroy().ok()"),
            "replacing an existing dictation overlay should destroy, not close, the old WebView"
        );
        assert!(
            !commands_rs.contains("window.close()"),
            "dictation overlay lifecycle should not queue async WebView close during teardown"
        );
    }

    #[test]
    fn idle_when_all_flags_false() {
        assert_eq!(
            derive_tray_activity(snap(false, false, false)),
            TrayActivity::Idle
        );
    }

    #[test]
    fn recording_takes_priority_over_live_and_dictation() {
        // Recording > Live > Dictation. The acquisition gates are
        // check-then-CAS across separate atomics, so concurrent starts
        // can land in a transient double-true state until the loser's
        // session fails at the PID/flock layer in core and its RAII
        // guard re-syncs the tray. `recording_active` can also be true
        // from an external CLI PID independent of the in-app flags. In
        // any drift scenario the tray must render deterministically.
        assert_eq!(
            derive_tray_activity(snap(true, true, true)),
            TrayActivity::Recording
        );
        assert_eq!(
            derive_tray_activity(snap(true, false, true)),
            TrayActivity::Recording
        );
        assert_eq!(
            derive_tray_activity(snap(true, true, false)),
            TrayActivity::Recording
        );
    }

    #[test]
    fn live_beats_dictation_when_recording_is_false() {
        assert_eq!(
            derive_tray_activity(snap(false, true, true)),
            TrayActivity::Live
        );
        assert_eq!(
            derive_tray_activity(snap(false, true, false)),
            TrayActivity::Live
        );
    }

    #[test]
    fn dictation_when_only_dictation_set() {
        assert_eq!(
            derive_tray_activity(snap(false, false, true)),
            TrayActivity::Dictation
        );
    }

    #[test]
    fn is_active_only_for_non_idle() {
        assert!(!TrayActivity::Idle.is_active());
        assert!(TrayActivity::Recording.is_active());
        assert!(TrayActivity::Live.is_active());
        assert!(TrayActivity::Dictation.is_active());
    }

    #[test]
    fn stop_label_per_activity() {
        // Idle keeps the construction-time label so the menu reads "Stop
        // Recording" before any session starts; the active states each
        // disambiguate which flow Stop will target.
        assert_eq!(TrayActivity::Idle.stop_label(), "Stop Recording");
        assert_eq!(TrayActivity::Recording.stop_label(), "Stop Recording");
        assert_eq!(TrayActivity::Live.stop_label(), "Stop Live Transcript");
        assert_eq!(TrayActivity::Dictation.stop_label(), "Stop Dictation");
    }

    #[test]
    fn palette_source_per_activity() {
        assert_eq!(TrayActivity::Idle.palette_source(), "idle");
        assert_eq!(TrayActivity::Recording.palette_source(), "recording");
        assert_eq!(TrayActivity::Live.palette_source(), "live-transcript");
        assert_eq!(TrayActivity::Dictation.palette_source(), "dictation");
    }

    #[test]
    fn icon_bytes_pick_appearance_variant() {
        // Idle uses the templated tray icon regardless of appearance.
        let idle_light = TrayActivity::Idle.icon_bytes(TrayAppearance::Light);
        let idle_dark = TrayActivity::Idle.icon_bytes(TrayAppearance::Dark);
        assert_eq!(idle_light, idle_dark);

        // Active states must pick distinct bytes per appearance so the
        // M is legible on both light and dark menu bars. Compare slice
        // contents (not pointer identity) so the test fails if a future
        // refactor inadvertently routes both arms to the same PNG.
        let rec_light = TrayActivity::Recording.icon_bytes(TrayAppearance::Light);
        let rec_dark = TrayActivity::Recording.icon_bytes(TrayAppearance::Dark);
        assert_ne!(rec_light, rec_dark);

        let live_light = TrayActivity::Live.icon_bytes(TrayAppearance::Light);
        let live_dark = TrayActivity::Live.icon_bytes(TrayAppearance::Dark);
        assert_ne!(live_light, live_dark);

        // Dictation reuses the recording asset (no dedicated dictation
        // icon — out of scope for this commit). Same bytes per appearance
        // as recording, distinct across appearance variants.
        let dict_light = TrayActivity::Dictation.icon_bytes(TrayAppearance::Light);
        let dict_dark = TrayActivity::Dictation.icon_bytes(TrayAppearance::Dark);
        assert_eq!(dict_light, rec_light);
        assert_eq!(dict_dark, rec_dark);
        assert_ne!(dict_light, dict_dark);

        // All assets are valid PNGs (magic bytes 89 50 4E 47 0D 0A 1A 0A).
        // If a future asset substitution swaps in the wrong format this
        // catches it before runtime.
        for bytes in [
            idle_light, rec_light, rec_dark, live_light, live_dark, dict_light, dict_dark,
        ] {
            assert!(
                bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                "tray icon asset is not a valid PNG"
            );
        }
    }

    #[test]
    fn appearance_from_theme_maps_dark_only_to_dark() {
        // Tauri Theme is non-exhaustive (Light / Dark / future). Anything
        // other than Dark resolves to Light — the active-state assets
        // were originally designed for light menu bars (commit 2c9d26d),
        // so a future variant defaults to the lower-risk choice.
        assert_eq!(
            TrayAppearance::from_theme(tauri::Theme::Light),
            TrayAppearance::Light
        );
        assert_eq!(
            TrayAppearance::from_theme(tauri::Theme::Dark),
            TrayAppearance::Dark
        );
    }
}
