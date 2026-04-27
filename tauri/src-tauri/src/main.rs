#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use minutes_core::Config;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem, SubmenuBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};

#[cfg(feature = "parakeet")]
#[used]
static PARAKEET_FEATURE_SENTINEL: &[u8] = b"transcribe_parakeet parakeet_helper\0";

mod call_capture;
mod call_detect;
mod commands;
mod context;
mod palette_dispatch;
mod pty;
mod secret_store;
mod shortcut_manager;

const MINUTES_WEBSITE_URL: &str = "https://useminutes.app";
const MINUTES_CHANGELOG_URL: &str = "https://github.com/silverstein/minutes/releases";
const MINUTES_DISCUSSIONS_URL: &str = "https://github.com/silverstein/minutes/discussions";

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

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        win.show().ok();
        win.set_focus().ok();
        return;
    }
    if let Ok(win) = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Minutes")
        .inner_size(520.0, 700.0)
        .min_inner_size(420.0, 520.0)
        .transparent(true)
        .content_protected(Config::load().privacy.hide_from_screen_share)
        .center()
        .focused(true)
        .build()
    {
        #[cfg(target_os = "macos")]
        {
            use window_vibrancy::{apply_vibrancy, NSVisualEffectMaterial};
            apply_vibrancy(&win, NSVisualEffectMaterial::Sidebar, None, None).ok();
        }
    }
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

/// Update tray to reflect recording state
pub fn update_tray_state(app: &tauri::AppHandle, is_recording: bool) {
    update_tray_state_with_mode(app, is_recording, false);
}

pub fn update_tray_state_with_mode(app: &tauri::AppHandle, is_active: bool, is_live: bool) {
    if let Some(tray) = app.tray_by_id("minutes-tray") {
        let icon_bytes: &[u8] = if is_live {
            include_bytes!("../icons/icon-live.png")
        } else if is_active {
            include_bytes!("../icons/icon-recording.png")
        } else {
            include_bytes!("../icons/icon.png")
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            tray.set_icon(Some(icon)).ok();
            tray.set_icon_as_template(!is_active).ok();
        }
        let tooltip = if is_live {
            "Minutes — Live Transcribing..."
        } else if is_active {
            "Minutes — Recording..."
        } else {
            "Minutes"
        };
        tray.set_tooltip(Some(tooltip)).ok();
    }

    // Notify the palette overlay that recording / live state changed
    // so it can re-fetch its visible command list. Dictation transitions
    // emit `palette:refresh` separately from `commands.rs`.
    let _ = app.emit(
        "palette:refresh",
        serde_json::json!({
            "source": if is_live { "live-transcript" } else { "recording" },
            "active": is_active,
        }),
    );
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
        win.close().ok();
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
    #[cfg(target_os = "macos")]
    if let Some(code) = maybe_run_hotkey_diagnostic() {
        std::process::exit(code);
    }

    // Load with first-run and upgrade migrations so palette defaults
    // stay enabled across upgrades and fresh installs.
    let mut startup_config_snapshot = minutes_core::config::Config::load_with_migrations();
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
    let completion_notifications_enabled = Arc::new(AtomicBool::new(true));
    let global_hotkey_enabled = Arc::new(AtomicBool::new(false));
    let global_hotkey_shortcut =
        Arc::new(Mutex::new(commands::default_hotkey_shortcut().to_string()));
    let dictation_shortcut_enabled = Arc::new(AtomicBool::new(false));
    let dictation_shortcut = Arc::new(Mutex::new(
        startup_config_snapshot.dictation.shortcut.clone(),
    ));
    let hotkey_runtime = Arc::new(Mutex::new(commands::HotkeyRuntime::default()));
    let discard_short_hotkey_capture = Arc::new(AtomicBool::new(false));
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
    let started_by_call_detect_for_detector = recording_started_by_call_detect.clone();
    let countdown_cancel_for_detector = call_end_countdown_cancel.clone();
    let countdown_active_for_detector = call_end_countdown_active.clone();
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
                    let _ = window.unminimize();
                    let _ = window.show();
                }
                if let Some(main) = app.get_webview_window("main") {
                    let _ = main.set_focus();
                }
            }
            "app-quit" => {
                if let Ok(mut mgr) = app.state::<commands::AppState>().pty_manager.lock() {
                    mgr.kill_all();
                }
                let state = app.state::<commands::AppState>();
                let recording = state.recording.clone();
                let stop = state.stop_flag.clone();
                if commands::recording_active(&recording) {
                    if commands::request_stop(&recording, &stop).is_err() {
                        return;
                    }
                    std::thread::spawn(|| {
                        commands::wait_for_recording_shutdown_forever();
                        std::process::exit(0);
                    });
                } else {
                    std::process::exit(0);
                }
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
            dictation_active: Arc::new(AtomicBool::new(false)),
            dictation_stop_flag: Arc::new(AtomicBool::new(false)),
            live_transcript_active: Arc::new(AtomicBool::new(false)),
            live_transcript_stop_flag: Arc::new(AtomicBool::new(false)),
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
        })
        .manage(Arc::new(Mutex::new(
            shortcut_manager::ShortcutManager::new(),
        )))
        .setup(move |app| {
            let initial_recording = minutes_core::pid::status().recording;
            let startup_config = minutes_core::config::Config::load();

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

            if !(allow_debug_update_state && debug_update_state.is_some()) {
                // Auto-update: check on launch, then every 6 hours.
                // Check-only (no download). Download starts only when the user
                // accepts the update from the desktop banner.
                // Defers notification if recording/live/dictation is active.
                // Between checks, polls every 30s to surface deferred updates once sessions end.
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
            show_main_window(app.handle());

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

            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/icon.png"))
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
                    let stp_item = stop_item_ref.clone();
                    let screen_share_hidden = screen_share_hidden.clone();
                    let screen_share_item_ref = screen_share_item_ref.clone();
                    let mic_mute_item_ref = mic_mute_item_ref.clone();
                    match event.id.as_ref() {
                        "open" => {
                            show_main_window(app);
                        }
                        "record" => {
                            if commands::recording_active(&recording) {
                                return;
                            }
                            rec_item.set_text("Starting...").ok();
                            rec_item.set_enabled(false).ok();
                            quick_item.set_enabled(false).ok();
                            stp_item.set_enabled(true).ok();
                            let app_handle = app.clone();
                            let app_done = app.clone();
                            let ri = rec_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                let app_for_launch = app_handle.clone();
                                let state = app_handle.state::<commands::AppState>();
                                let _ = commands::launch_recording(
                                    app_for_launch,
                                    &state,
                                    minutes_core::CaptureMode::Meeting,
                                    None,
                                    false,
                                    None,
                                    None,
                                    None,
                                    None,
                                );
                                ri.set_text("Start Recording").ok();
                                ri.set_enabled(true).ok();
                                quick_item.set_enabled(true).ok();
                                si.set_enabled(false).ok();
                                update_tray_state(&app_done, false);
                            });
                        }
                        "quick-thought" => {
                            if commands::recording_active(&recording) {
                                return;
                            }
                            rec_item.set_enabled(false).ok();
                            quick_item.set_text("Starting Quick Thought…").ok();
                            quick_item.set_enabled(false).ok();
                            stp_item.set_enabled(true).ok();
                            let app_handle = app.clone();
                            let app_done = app.clone();
                            let ri = rec_item.clone();
                            let qi = quick_item.clone();
                            let si = stp_item.clone();
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
                                ri.set_enabled(true).ok();
                                qi.set_text("Quick Thought").ok();
                                qi.set_enabled(true).ok();
                                si.set_enabled(false).ok();
                                update_tray_state(&app_done, false);
                            });
                        }
                        "stop" if commands::request_stop(&recording, &stop).is_ok() => {
                            rec_item.set_text("Stopping...").ok();
                            rec_item.set_enabled(false).ok();
                            quick_item.set_text("Quick Thought").ok();
                            quick_item.set_enabled(false).ok();
                            stp_item.set_enabled(false).ok();
                            let app_done = app.clone();
                            let ri = rec_item.clone();
                            let qi = quick_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                if commands::wait_for_recording_shutdown(
                                    std::time::Duration::from_secs(120),
                                ) {
                                    ri.set_text("Start Recording").ok();
                                    ri.set_enabled(true).ok();
                                    qi.set_text("Quick Thought").ok();
                                    qi.set_enabled(true).ok();
                                    si.set_enabled(false).ok();
                                    update_tray_state(&app_done, false);
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
                            // Kill all PTY sessions before exiting
                            if let Ok(mut mgr) =
                                app.state::<commands::AppState>().pty_manager.lock()
                            {
                                mgr.kill_all();
                            }
                            if commands::recording_active(&recording) {
                                if commands::request_stop(&recording, &stop).is_err() {
                                    return;
                                }
                                std::thread::spawn(|| {
                                    commands::wait_for_recording_shutdown_forever();
                                    std::process::exit(0);
                                });
                            } else {
                                std::process::exit(0);
                            }
                        }
                        // Calendar event items — start recording on click
                        "cal-0" | "cal-1" | "cal-2" => {
                            if commands::recording_active(&recording) {
                                return;
                            }
                            rec_item.set_text("Starting...").ok();
                            rec_item.set_enabled(false).ok();
                            quick_item.set_enabled(false).ok();
                            stp_item.set_enabled(true).ok();
                            let app_handle = app.clone();
                            let app_done = app.clone();
                            let ri = rec_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                let app_for_launch = app_handle.clone();
                                let state = app_handle.state::<commands::AppState>();
                                let _ = commands::launch_recording(
                                    app_for_launch,
                                    &state,
                                    minutes_core::CaptureMode::Meeting,
                                    None,
                                    false,
                                    None,
                                    None,
                                    None,
                                    None,
                                );
                                ri.set_text("Start Recording").ok();
                                ri.set_enabled(true).ok();
                                quick_item.set_enabled(true).ok();
                                si.set_enabled(false).ok();
                                update_tray_state(&app_done, false);
                            });
                        }
                        _ => {}
                    }
                })
                .build(app)?;

            update_tray_state(app.handle(), initial_recording);

            // Start call detection background loop
            if commands::supports_call_detection() {
                let config = minutes_core::config::Config::load();
                let detector = Arc::new(call_detect::CallDetector::new(config.call_detection));
                detector.start(
                    app.handle().clone(),
                    recording_for_detector,
                    processing_clone,
                    call_detect::CallEndAutoStopHandles {
                        recording_started_by_call_detect: started_by_call_detect_for_detector,
                        countdown_cancel: countdown_cancel_for_detector,
                        countdown_active: countdown_active_for_detector,
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
                _ => {}
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::cmd_status,
            commands::cmd_processing_jobs,
            commands::cmd_list_meetings,
            commands::cmd_search,
            commands::cmd_add_note,
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
            commands::cmd_recovery_items,
            commands::cmd_retry_recovery,
            commands::cmd_retry_processing_job,
            commands::cmd_weekly_summary,
            commands::cmd_proactive_context_bundle,
            commands::cmd_list_devices,
            commands::cmd_delete_meeting,
            commands::cmd_get_meeting_detail,
            commands::cmd_list_voices,
            commands::cmd_confirm_speaker,
            commands::cmd_needs_setup,
            commands::cmd_download_model,
            commands::cmd_mark_activation_nudge_shown,
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
            commands::cmd_start_dictation,
            commands::cmd_stop_dictation,
            commands::cmd_enable_dictation_hotkey,
            commands::cmd_dictation_hotkey_status,
            commands::cmd_check_accessibility,
            commands::cmd_request_accessibility,
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
        .run(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
            }
        });
}
