#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, WebviewUrl, WebviewWindowBuilder,
};

mod call_detect;
mod commands;
mod context;
mod pty;

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
    if let Some(tray) = app.tray_by_id("minutes-tray") {
        let icon_bytes: &[u8] = if is_recording {
            include_bytes!("../icons/icon-recording.png")
        } else {
            include_bytes!("../icons/icon.png")
        };
        if let Ok(icon) = tauri::image::Image::from_bytes(icon_bytes) {
            tray.set_icon(Some(icon)).ok();
            tray.set_icon_as_template(!is_recording).ok();
        }
        tray.set_tooltip(Some(if is_recording {
            "Minutes — Recording..."
        } else {
            "Minutes"
        }))
        .ok();
    }
}

// ── Calendar items in tray menu ──────────────────────────────

const MAX_CALENDAR_ITEMS: usize = 3;
const CALENDAR_REFRESH_SECS: u64 = 60;
const CALENDAR_LOOKAHEAD_MINUTES: u32 = 240; // 4 hours
const MEETING_NOTIFY_MINUTES: i64 = 2; // Notify this many minutes before

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
    // Notify for meetings starting in ≤ MEETING_NOTIFY_MINUTES (once per event)
    for e in &all_events {
        if e.minutes_until >= 0
            && e.minutes_until <= MEETING_NOTIFY_MINUTES
            && !state.notified.contains(&e.title)
        {
            let body = if e.minutes_until == 0 {
                format!("{} is starting now. Click to record.", e.title)
            } else if e.minutes_until == 1 {
                format!("{} starts in 1 minute. Click to record.", e.title)
            } else {
                format!(
                    "{} starts in {} minutes. Click to record.",
                    e.title, e.minutes_until
                )
            };
            crate::commands::show_user_notification("Upcoming Meeting", &body);
            state.notified.insert(e.title.clone());
            eprintln!("[calendar] notified: {} (in {} min)", e.title, e.minutes_until);
        }
    }

    // Clean up old notifications (events that have passed)
    state
        .notified
        .retain(|title| all_events.iter().any(|e| &e.title == title && e.minutes_until >= -5));

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
                .with_handler(|app, _shortcut, event| {
                    commands::handle_global_hotkey_event(app, event.state());
                })
                .build(),
        )
        .plugin(tauri_plugin_shell::init())
        .manage(commands::AppState {
            recording: recording.clone(),
            starting: starting.clone(),
            stop_flag: stop_flag.clone(),
            processing: processing.clone(),
            processing_stage: processing_stage.clone(),
            latest_output: latest_output.clone(),
            completion_notifications_enabled: completion_notifications_enabled.clone(),
            global_hotkey_enabled: global_hotkey_enabled.clone(),
            global_hotkey_shortcut: global_hotkey_shortcut.clone(),
            hotkey_runtime: hotkey_runtime.clone(),
            discard_short_hotkey_capture: discard_short_hotkey_capture.clone(),
            pty_manager: Arc::new(Mutex::new(pty::PtyManager::default())),
        })
        .setup(move |app| {
            let initial_recording = minutes_core::pid::status().recording;

            // Clean up stale terminal workspaces from previous sessions
            context::cleanup_stale_workspaces();

            // Create main window on launch
            show_main_window(app.handle());

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

            let menu = Menu::with_items(
                app,
                &[
                    &open_item,
                    &sep0,
                    &record_item,
                    &quick_thought_item,
                    &stop_item,
                    &sep,
                    &note_item,
                    &assistant_item,
                    &list_item,
                    &paste_summary_item,
                    &paste_transcript_item,
                    &sep2,
                    &screen_share_item,
                    &quit_item,
                ],
            )?;

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
                            let rec = recording.clone();
                            let starting = starting.clone();
                            let sf = stop.clone();
                            let processing = processing.clone();
                            let processing_stage = processing_stage.clone();
                            let latest_output = latest_output.clone();
                            let completion_notifications_enabled =
                                completion_notifications_enabled.clone();
                            let ri = rec_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                commands::start_recording(
                                    app_handle,
                                    rec,
                                    starting,
                                    sf,
                                    processing,
                                    processing_stage,
                                    latest_output,
                                    completion_notifications_enabled,
                                    None,
                                    None,
                                    minutes_core::CaptureMode::Meeting,
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
                            let rec = recording.clone();
                            let starting = starting.clone();
                            let sf = stop.clone();
                            let processing = processing.clone();
                            let processing_stage = processing_stage.clone();
                            let latest_output = latest_output.clone();
                            let completion_notifications_enabled =
                                completion_notifications_enabled.clone();
                            let ri = rec_item.clone();
                            let qi = quick_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                commands::start_recording(
                                    app_handle,
                                    rec,
                                    starting,
                                    sf,
                                    processing,
                                    processing_stage,
                                    latest_output,
                                    completion_notifications_enabled,
                                    None,
                                    None,
                                    minutes_core::CaptureMode::QuickThought,
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
                                    commands::show_user_notification("AI Assistant", &err);
                                }
                            });
                        }
                        "list" => {
                            let meetings_dir =
                                dirs::home_dir().unwrap_or_default().join("meetings");
                            let _ = std::process::Command::new("open").arg(meetings_dir).spawn();
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
                                        &format!("Latest {}", kind),
                                        &message,
                                    );
                                }
                                Err(err) => {
                                    commands::show_user_notification(
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
                            let rec = recording.clone();
                            let starting = starting.clone();
                            let sf = stop.clone();
                            let processing = processing.clone();
                            let processing_stage = processing_stage.clone();
                            let latest_output = latest_output.clone();
                            let completion_notifications_enabled =
                                completion_notifications_enabled.clone();
                            let ri = rec_item.clone();
                            let si = stp_item.clone();
                            std::thread::spawn(move || {
                                commands::start_recording(
                                    app_handle,
                                    rec,
                                    starting,
                                    sf,
                                    processing,
                                    processing_stage,
                                    latest_output,
                                    completion_notifications_enabled,
                                    None,
                                    None,
                                    minutes_core::CaptureMode::Meeting,
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
            {
                let config = minutes_core::config::Config::load();
                let detector = Arc::new(call_detect::CallDetector::new(config.call_detection));
                detector.start(
                    app.handle().clone(),
                    recording_for_detector,
                    processing_clone,
                );
            }

            // Calendar items in tray menu — refresh every minute
            {
                let app_cal = app.handle().clone();
                let menu_cal = menu.clone();
                let cal_timer = cal_state.clone();
                std::thread::spawn(move || loop {
                    refresh_calendar_items(&app_cal, &menu_cal, &cal_timer);
                    std::thread::sleep(std::time::Duration::from_secs(CALENDAR_REFRESH_SECS));
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
            commands::cmd_list_meetings,
            commands::cmd_search,
            commands::cmd_add_note,
            commands::cmd_start_recording,
            commands::cmd_stop_recording,
            commands::cmd_open_file,
            commands::cmd_clear_latest_output,
            commands::cmd_set_completion_notifications,
            commands::cmd_global_hotkey_settings,
            commands::cmd_set_global_hotkey,
            commands::cmd_permission_center,
            commands::cmd_recovery_items,
            commands::cmd_retry_recovery,
            commands::cmd_get_meeting_detail,
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
            commands::cmd_get_storage_stats,
        ])
        .run(tauri::generate_context!())
        .expect("error while running minutes app");
}
