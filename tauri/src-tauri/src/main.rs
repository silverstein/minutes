#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};

mod call_capture;
mod call_detect;
mod commands;
mod context;
mod pty;
mod shortcut_manager;

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
    let _win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("Minutes")
        .inner_size(480.0, 640.0)
        .min_inner_size(380.0, 480.0)
        .content_protected(true)
        .center()
        .focused(true)
        .build();
}

fn show_note_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("note") {
        win.show().ok();
        win.set_focus().ok();
        return;
    }
    let _win = WebviewWindowBuilder::new(app, "note", WebviewUrl::App("note.html".into()))
        .title("Add Note")
        .inner_size(360.0, 200.0)
        .resizable(false)
        .content_protected(true)
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
        .content_protected(true)
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

    // Encode event data in URL fragment: title|minutesUntil|url
    let url_part = event.url.as_deref().unwrap_or("");
    let fragment = format!(
        "{}|{}|{}",
        event.title.replace('|', " "),
        event.minutes_until,
        url_part
    );
    let encoded = fragment
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-._~ |/".contains(c) {
                c.to_string()
            } else {
                format!("%{:02X}", c as u32)
            }
        })
        .collect::<String>();
    let url = format!("meeting-prompt.html#{}", encoded);

    // Position: top-right of main screen, below menu bar
    let (pos_x, pos_y) = get_top_right_position(340.0, 140.0);

    match WebviewWindowBuilder::new(app, "meeting-prompt", WebviewUrl::App(url.into()))
        .title("Upcoming Meeting")
        .inner_size(340.0, 140.0)
        .position(pos_x, pos_y)
        .resizable(false)
        .decorations(false)
        .always_on_top(true)
        .focused(true)
        .skip_taskbar(true)
        .build()
    {
        Ok(_) => eprintln!("[calendar] meeting prompt shown for: {}", event.title),
        Err(e) => eprintln!("[calendar] failed to show meeting prompt: {}", e),
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

fn main() {
    #[cfg(target_os = "macos")]
    if let Some(code) = maybe_run_hotkey_diagnostic() {
        std::process::exit(code);
    }

    let startup_config_snapshot = minutes_core::config::Config::load();
    let recording = Arc::new(AtomicBool::new(false));
    let starting = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::new(AtomicBool::new(false));
    let processing = Arc::new(AtomicBool::new(false));
    let processing_stage = Arc::new(Mutex::new(None));
    let latest_output = Arc::new(Mutex::new(None));
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
    let screen_share_hidden = Arc::new(AtomicBool::new(true));
    let recording_clone = recording.clone();
    let recording_for_detector = recording.clone();
    let processing_clone = processing.clone();
    let stop_clone = stop_flag.clone();

    tauri::Builder::default()
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

                    if Some(shortcut_id) == dictation_shortcut_id {
                        commands::handle_dictation_shortcut_event(app, event.state());
                    } else if Some(shortcut_id) == live_shortcut_id {
                        commands::handle_live_shortcut_event(app, event.state());
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
        .manage(commands::AppState {
            recording: recording.clone(),
            starting: starting.clone(),
            stop_flag: stop_flag.clone(),
            processing: processing.clone(),
            processing_stage: processing_stage.clone(),
            latest_output: latest_output.clone(),
            call_capture_health: Arc::new(Mutex::new(None)),
            completion_notifications_enabled: completion_notifications_enabled.clone(),
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
        })
        .manage(Arc::new(Mutex::new(
            shortcut_manager::ShortcutManager::new(),
        )))
        .setup(move |app| {
            let initial_recording = minutes_core::pid::status().recording;
            let startup_config = minutes_core::config::Config::load();

            // Clean up stale terminal workspaces from previous sessions
            context::cleanup_stale_workspaces();

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
                "Hide from Screen Share ✓",
                true,
                None::<&str>,
            )?;
            let screen_share_item_ref = screen_share_item.clone();
            let sep2 = MenuItem::with_id(app, "sep2", "──────────", false, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit Minutes", true, None::<&str>)?;

            let menu = Menu::new(app)?;
            menu.append_items(&[
                &open_item,
                &sep0,
                &record_item,
                &quick_thought_item,
                &stop_item,
                &sep,
                &note_item,
                &assistant_item,
                &list_item,
            ])?;
            if commands::supports_tray_artifact_copy() {
                menu.append_items(&[&paste_summary_item, &paste_transcript_item])?;
            }
            menu.append_items(&[&sep2, &screen_share_item, &quit_item])?;

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
                        "stop" => {
                            if commands::request_stop(&recording, &stop).is_ok() {
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
                    loop {
                        refresh_calendar_items(&app_cal, &menu_cal, &cal_timer);
                        std::thread::sleep(std::time::Duration::from_secs(CALENDAR_REFRESH_SECS));
                    }
                });
            }

            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    // Hide main window on close instead of quitting (app stays in tray)
                    // PTY session persists — user can reopen and resume where they left off
                    api.prevent_close();
                    window.hide().ok();
                }
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
            commands::cmd_extend_recording,
            commands::cmd_open_file,
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
            commands::cmd_list_devices,
            commands::cmd_delete_meeting,
            commands::cmd_get_meeting_detail,
            commands::cmd_list_voices,
            commands::cmd_confirm_speaker,
            commands::cmd_needs_setup,
            commands::cmd_download_model,
            commands::cmd_upcoming_meetings,
            commands::cmd_spawn_terminal,
            commands::cmd_pty_input,
            commands::cmd_pty_resize,
            commands::cmd_pty_kill,
            commands::cmd_list_agents,
            commands::cmd_terminal_info,
            commands::cmd_get_settings,
            commands::cmd_set_setting,
            commands::cmd_get_autostart,
            commands::cmd_set_autostart,
            commands::cmd_get_storage_stats,
            commands::cmd_vault_status,
            commands::cmd_vault_setup,
            commands::cmd_vault_unlink,
            commands::cmd_open_meeting_url,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running minutes app");
}
