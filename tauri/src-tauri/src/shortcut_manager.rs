use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{Emitter, Manager};

// ── Types ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutSlot {
    QuickThought,
    Dictation,
}

impl ShortcutSlot {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::QuickThought => "quick_thought",
            Self::Dictation => "dictation",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "quick_thought" => Ok(Self::QuickThought),
            "dictation" => Ok(Self::Dictation),
            _ => Err(format!("Unknown shortcut slot: {}", s)),
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::QuickThought => "Quick Thought",
            Self::Dictation => "Dictation",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutBackend {
    /// Carbon RegisterEventHotKey via tauri-plugin-global-shortcut. No permissions.
    Standard,
    /// CGEventTap via hotkey_macos.rs. Requires Input Monitoring.
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureStyle {
    Hold,
    Locked,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutStatus {
    pub slot: String,
    pub enabled: bool,
    pub pending: bool,
    pub shortcut: String,
    pub keycode: i64,
    pub backend: String,
    pub needs_permission: bool,
    pub message: String,
}

// ── Constants ────────────────────────────────────────────────

const HOLD_THRESHOLD_MS: u64 = 300;
const MIN_CAPTURE_DURATION_MS: u64 = 400;

/// macOS virtual keycodes that require the native CGEventTap backend
/// because Carbon RegisterEventHotKey cannot intercept them.
const NATIVE_KEYCODES: &[i64] = &[
    57, // Caps Lock
    63, // fn/Globe
];

// ── Classification ───────────────────────────────────────────

/// Determine which backend a shortcut needs based on its keycode.
/// Keycodes in NATIVE_KEYCODES require CGEventTap (Input Monitoring).
/// Keycode -1 means "standard combo, no specific keycode."
pub fn classify_shortcut(keycode: i64) -> ShortcutBackend {
    if NATIVE_KEYCODES.contains(&keycode) {
        ShortcutBackend::Native
    } else {
        ShortcutBackend::Standard
    }
}

// ── State Machine ────────────────────────────────────────────

/// Unified three-mode state machine for shortcut-triggered actions.
///
/// Modes:
/// - Hold-to-talk: press past HOLD_THRESHOLD_MS starts action, release stops it.
/// - Double-tap-lock: quick tap (<HOLD_THRESHOLD_MS) toggles action on/off.
/// - Quick-tap-discard: captures shorter than MIN_CAPTURE_DURATION_MS are discarded.
#[derive(Debug, Default)]
pub struct ShortcutStateMachine {
    pub key_down: bool,
    pub key_down_started_at: Option<Instant>,
    pub active_capture: Option<CaptureStyle>,
    pub capture_started_at: Option<Instant>,
    pub hold_generation: u64,
}

/// Actions the state machine tells the caller to take.
#[derive(Debug)]
pub enum StateMachineAction {
    /// Start a new session in hold mode (user is holding the key).
    StartHold,
    /// Start a new session in locked mode (user tapped quickly).
    StartLocked,
    /// Stop the active session (release after hold, or second tap after lock).
    Stop { discard: bool },
    /// No action (key event consumed but nothing to do).
    None,
}

impl ShortcutStateMachine {
    pub fn clear(&mut self) {
        self.key_down = false;
        self.key_down_started_at = None;
        self.active_capture = None;
        self.capture_started_at = None;
    }

    /// Record that a session was started with the given capture style.
    pub fn mark_session_started(&mut self, style: CaptureStyle) {
        self.active_capture = Some(style);
        self.capture_started_at = Some(Instant::now());
    }

    /// Handle a key press event. Returns the action to take.
    ///
    /// For Standard backend (global-shortcut plugin), this is called on ShortcutState::Pressed.
    /// For Native backend (CGEventTap), this is called on HotkeyEvent::Press.
    pub fn handle_press(&mut self) -> StateMachineAction {
        if self.key_down {
            return StateMachineAction::None; // Key repeat, ignore
        }
        self.key_down = true;
        self.key_down_started_at = Some(Instant::now());
        self.hold_generation = self.hold_generation.wrapping_add(1);

        // The hold-to-talk check happens after a delay (see schedule_hold_check).
        // We return None here; the hold timer will call start_hold_if_still_down().
        StateMachineAction::None
    }

    /// Called after HOLD_THRESHOLD_MS to check if the key is still held.
    /// If so, returns StartHold. Otherwise returns None.
    pub fn start_hold_if_still_down(&self, generation: u64) -> StateMachineAction {
        if self.key_down && self.hold_generation == generation && self.active_capture.is_none() {
            StateMachineAction::StartHold
        } else {
            StateMachineAction::None
        }
    }

    /// Handle a key release event. Returns the action to take.
    pub fn handle_release(&mut self, is_session_active: bool) -> StateMachineAction {
        let pressed_at = self.key_down_started_at;
        self.key_down = false;
        self.key_down_started_at = None;

        let was_short_tap = pressed_at
            .map(|pressed| {
                Instant::now().duration_since(pressed).as_millis() < HOLD_THRESHOLD_MS as u128
            })
            .unwrap_or(false);

        // If we're in a hold capture, stop it on release.
        if matches!(self.active_capture, Some(CaptureStyle::Hold)) {
            let discard = self.should_discard_capture();
            self.active_capture = None;
            self.capture_started_at = None;
            return StateMachineAction::Stop { discard };
        }

        // Not a short tap = we were waiting for hold but never started. Ignore.
        if !was_short_tap {
            return StateMachineAction::None;
        }

        // Short tap while session is active = stop (locked mode off).
        if is_session_active {
            self.active_capture = None;
            self.capture_started_at = None;
            return StateMachineAction::Stop { discard: false };
        }

        // Short tap with no active session = start locked mode.
        StateMachineAction::StartLocked
    }

    /// Whether the current capture should be discarded (too short to be useful).
    fn should_discard_capture(&self) -> bool {
        self.capture_started_at
            .map(|started| {
                Instant::now().duration_since(started).as_millis() < MIN_CAPTURE_DURATION_MS as u128
            })
            .unwrap_or(false)
    }

    /// Get the current hold generation for scheduling a hold check.
    pub fn hold_generation(&self) -> u64 {
        self.hold_generation
    }
}

// ── Active Shortcut Registration ─────────────────────────────

pub(crate) struct ActiveShortcut {
    shortcut_string: String,
    keycode: i64,
    backend: ShortcutBackend,
    state_machine: ShortcutStateMachine,
    /// For Standard backend: the shortcut ID for matching global-shortcut events.
    standard_shortcut_id: Option<u32>,
    /// For Native backend (macOS only): the running HotkeyMonitor.
    #[cfg(target_os = "macos")]
    native_monitor: Option<minutes_core::hotkey_macos::HotkeyMonitor>,
    /// Lifecycle for native monitors.
    native_lifecycle: NativeLifecycle,
    native_generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NativeLifecycle {
    NotApplicable,
    Starting,
    Active,
    Failed,
}

// ── ShortcutManager ──────────────────────────────────────────

pub struct ShortcutManager {
    slots: HashMap<ShortcutSlot, ActiveShortcut>,
    last_error: HashMap<ShortcutSlot, String>,
}

impl ShortcutManager {
    pub fn new() -> Self {
        Self {
            slots: HashMap::new(),
            last_error: HashMap::new(),
        }
    }

    /// Register a shortcut for a slot. Unregisters the previous one first.
    pub fn register(
        &mut self,
        slot: ShortcutSlot,
        shortcut_string: String,
        keycode: i64,
        app: &tauri::AppHandle,
    ) -> Result<ShortcutStatus, String> {
        let backend = classify_shortcut(keycode);

        // Conflict detection: check the other slot by both string and keycode.
        for (&other_slot, other_active) in &self.slots {
            if other_slot == slot {
                continue;
            }
            let string_conflict = other_active.shortcut_string == shortcut_string;
            let keycode_conflict =
                keycode >= 0 && other_active.keycode >= 0 && other_active.keycode == keycode;
            if string_conflict || keycode_conflict {
                return Err(format!(
                    "{} is already used by {}. Choose a different shortcut.",
                    shortcut_string,
                    other_slot.label()
                ));
            }
        }

        // Unregister the previous shortcut for this slot.
        self.unregister(slot, app)?;

        match backend {
            ShortcutBackend::Standard => {
                self.register_standard(slot, shortcut_string, keycode, app)
            }
            ShortcutBackend::Native => self.register_native(slot, shortcut_string, keycode, app),
        }
    }

    fn register_standard(
        &mut self,
        slot: ShortcutSlot,
        shortcut_string: String,
        keycode: i64,
        app: &tauri::AppHandle,
    ) -> Result<ShortcutStatus, String> {
        use tauri_plugin_global_shortcut::GlobalShortcutExt;

        // Parse the shortcut string first to get the ID for event matching.
        // If parsing fails, don't register -- the string is malformed.
        let parsed_shortcut =
            <tauri_plugin_global_shortcut::Shortcut as std::str::FromStr>::from_str(
                shortcut_string.as_str(),
            )
            .map_err(|e| format!("Invalid shortcut format: {}. ({})", shortcut_string, e))?;
        let shortcut_id = Some(parsed_shortcut.id());

        let manager = app.global_shortcut();
        if let Err(e) = manager.register(shortcut_string.as_str()) {
            let msg = format!(
                "Could not register {}. Another app may already be using it. ({})",
                shortcut_string, e
            );
            self.last_error.insert(slot, msg.clone());
            return Err(msg);
        }

        self.last_error.remove(&slot);
        self.slots.insert(
            slot,
            ActiveShortcut {
                shortcut_string: shortcut_string.clone(),
                keycode,
                backend: ShortcutBackend::Standard,
                state_machine: ShortcutStateMachine::default(),
                standard_shortcut_id: shortcut_id,
                #[cfg(target_os = "macos")]
                native_monitor: None,
                native_lifecycle: NativeLifecycle::NotApplicable,
                native_generation: 0,
            },
        );

        Ok(self.build_status(slot))
    }

    #[allow(unused_variables)]
    fn register_native(
        &mut self,
        slot: ShortcutSlot,
        shortcut_string: String,
        keycode: i64,
        app: &tauri::AppHandle,
    ) -> Result<ShortcutStatus, String> {
        #[cfg(target_os = "macos")]
        {
            self.register_native_macos(slot, shortcut_string, keycode, app)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Native hotkey (Caps Lock, fn) is only available on macOS.".into())
        }
    }

    #[cfg(target_os = "macos")]
    fn register_native_macos(
        &mut self,
        slot: ShortcutSlot,
        shortcut_string: String,
        keycode: i64,
        app: &tauri::AppHandle,
    ) -> Result<ShortcutStatus, String> {
        use minutes_core::hotkey_macos::HotkeyMonitor;

        let generation: u64 = self
            .slots
            .get(&slot)
            .map(|s| s.native_generation.wrapping_add(1))
            .unwrap_or(1);

        // Insert placeholder while monitor starts
        self.slots.insert(
            slot,
            ActiveShortcut {
                shortcut_string: shortcut_string.clone(),
                keycode,
                backend: ShortcutBackend::Native,
                state_machine: ShortcutStateMachine::default(),
                standard_shortcut_id: None,
                native_monitor: None,
                native_lifecycle: NativeLifecycle::Starting,
                native_generation: generation,
            },
        );
        self.last_error.remove(&slot);

        // We need to use a shared reference for the callbacks since HotkeyMonitor
        // takes ownership of the closures. The ShortcutManager is behind an
        // Arc<Mutex<>> in AppState, so the callbacks will lock it.
        let app_for_events = app.clone();
        let app_for_status = app.clone();

        let monitor = HotkeyMonitor::start(
            keycode,
            move |event| {
                handle_native_event_callback(&app_for_events, slot, event);
            },
            move |status| {
                handle_native_status_callback(&app_for_status, slot, generation, status);
            },
        )?;

        // Store the monitor
        if let Some(active) = self.slots.get_mut(&slot) {
            if active.native_generation == generation {
                active.native_monitor = Some(monitor);
            } else {
                // Generation changed while we were starting, stop this one
                monitor.stop();
            }
        }

        Ok(self.build_status(slot))
    }

    /// Unregister the shortcut for a slot.
    pub fn unregister(&mut self, slot: ShortcutSlot, app: &tauri::AppHandle) -> Result<(), String> {
        if let Some(active) = self.slots.remove(&slot) {
            match active.backend {
                ShortcutBackend::Standard => {
                    use tauri_plugin_global_shortcut::GlobalShortcutExt;
                    let manager = app.global_shortcut();
                    // Ignore unregister errors (shortcut may already be gone)
                    let _ = manager.unregister(active.shortcut_string.as_str());
                }
                ShortcutBackend::Native =>
                {
                    #[cfg(target_os = "macos")]
                    if let Some(monitor) = active.native_monitor {
                        monitor.stop();
                    }
                }
            }
        }
        self.last_error.remove(&slot);
        Ok(())
    }

    /// Find which slot owns a given shortcut ID (for the global-shortcut handler).
    pub fn find_slot_for_shortcut_id(&self, shortcut_id: u32) -> Option<ShortcutSlot> {
        self.slots
            .iter()
            .find(|(_, active)| active.standard_shortcut_id == Some(shortcut_id))
            .map(|(&slot, _)| slot)
    }

    /// Handle a press event for a slot. Returns the slot + generation for the
    /// hold-check timer. The caller must schedule the hold check AFTER dropping
    /// the lock.
    pub fn handle_press(&mut self, slot: ShortcutSlot) -> Option<(ShortcutSlot, u64)> {
        let active = self.slots.get_mut(&slot)?;

        let _action = active.state_machine.handle_press();
        let generation = active.state_machine.hold_generation();

        // Return slot+generation so the caller can schedule a hold check
        // after dropping the lock.
        Some((slot, generation))
    }

    /// Handle a release event for a slot. Returns the action to execute
    /// AFTER the caller drops the lock.
    pub fn handle_release(
        &mut self,
        slot: ShortcutSlot,
        is_session_active: bool,
    ) -> (ShortcutSlot, StateMachineAction) {
        let active = match self.slots.get_mut(&slot) {
            Some(active) => active,
            None => return (slot, StateMachineAction::None),
        };

        let action = active.state_machine.handle_release(is_session_active);
        (slot, action)
    }

    /// Check if a hold should start (called from the hold-check timer thread
    /// after re-acquiring the lock).
    pub fn check_hold_start(&self, slot: ShortcutSlot, generation: u64) -> StateMachineAction {
        match self.slots.get(&slot) {
            Some(active) => active.state_machine.start_hold_if_still_down(generation),
            None => StateMachineAction::None,
        }
    }

    /// Update native monitor lifecycle status.
    #[cfg(target_os = "macos")]
    pub fn update_native_status(
        &mut self,
        slot: ShortcutSlot,
        generation: u64,
        lifecycle: NativeLifecycle,
        error: Option<String>,
    ) {
        if let Some(active) = self.slots.get_mut(&slot) {
            if active.native_generation != generation {
                return; // Stale update
            }
            active.native_lifecycle = lifecycle;
            if lifecycle == NativeLifecycle::Failed {
                if let Some(monitor) = active.native_monitor.take() {
                    monitor.stop();
                }
            }
        }
        if let Some(err) = error {
            self.last_error.insert(slot, err);
        } else {
            self.last_error.remove(&slot);
        }
    }

    /// Build status for a slot.
    pub fn build_status(&self, slot: ShortcutSlot) -> ShortcutStatus {
        match self.slots.get(&slot) {
            Some(active) => {
                let needs_permission = active.backend == ShortcutBackend::Native && {
                    #[cfg(target_os = "macos")]
                    {
                        !minutes_core::hotkey_macos::is_input_monitoring_granted()
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        false
                    }
                };

                let enabled = match active.backend {
                    ShortcutBackend::Standard => true,
                    ShortcutBackend::Native => active.native_lifecycle == NativeLifecycle::Active,
                };
                let pending = active.native_lifecycle == NativeLifecycle::Starting;

                let message = if let Some(err) = self.last_error.get(&slot) {
                    err.clone()
                } else if pending {
                    format!("Starting {}...", active.shortcut_string)
                } else if needs_permission {
                    "Requires Input Monitoring permission.".into()
                } else if enabled {
                    format!(
                        "Active. Hold {} to {}, or tap to lock and tap again to stop.",
                        active.shortcut_string,
                        match slot {
                            ShortcutSlot::QuickThought => "record a quick thought",
                            ShortcutSlot::Dictation => "dictate",
                        }
                    )
                } else {
                    "Off.".into()
                };

                ShortcutStatus {
                    slot: slot.as_str().into(),
                    enabled,
                    pending,
                    shortcut: active.shortcut_string.clone(),
                    keycode: active.keycode,
                    backend: match active.backend {
                        ShortcutBackend::Standard => "standard".into(),
                        ShortcutBackend::Native => "native".into(),
                    },
                    needs_permission,
                    message,
                }
            }
            None => ShortcutStatus {
                slot: slot.as_str().into(),
                enabled: false,
                pending: false,
                shortcut: default_shortcut_for_slot(slot).into(),
                keycode: default_keycode_for_slot(slot),
                backend: "standard".into(),
                needs_permission: false,
                message: "Off.".into(),
            },
        }
    }

    /// Mark a session as started for a slot's state machine.
    pub fn mark_session_started(&mut self, slot: ShortcutSlot, style: CaptureStyle) {
        if let Some(active) = self.slots.get_mut(&slot) {
            active.state_machine.mark_session_started(style);
        }
    }
}

// ── Defaults ─────────────────────────────────────────────────

pub fn default_shortcut_for_slot(slot: ShortcutSlot) -> &'static str {
    match slot {
        ShortcutSlot::QuickThought => "CmdOrCtrl+Shift+M",
        ShortcutSlot::Dictation => "CmdOrCtrl+Shift+Space",
    }
}

pub fn default_keycode_for_slot(slot: ShortcutSlot) -> i64 {
    match slot {
        ShortcutSlot::QuickThought => -1,
        ShortcutSlot::Dictation => -1,
    }
}

// ── Helpers ──────────────────────────────────────────────────

/// Fast in-memory check if a session is active. No filesystem I/O.
/// Safe to call from any thread, including CGEventTap callbacks.
pub fn is_slot_session_active_fast(app: &tauri::AppHandle, slot: ShortcutSlot) -> bool {
    let state = match app.try_state::<crate::commands::AppState>() {
        Some(s) => s,
        None => return false,
    };
    match slot {
        ShortcutSlot::QuickThought => {
            // Only block on active recording/starting, NOT processing.
            // Processing happens in the background and should not prevent new captures.
            state.recording.load(Ordering::Relaxed) || state.starting.load(Ordering::Relaxed)
        }
        ShortcutSlot::Dictation => state.dictation_active.load(Ordering::Relaxed),
    }
}

/// Execute a state machine action for a slot.
/// IMPORTANT: This must be called AFTER the ShortcutManager lock is dropped.
/// It may re-acquire the lock briefly to update state on success.
pub fn execute_action(app: &tauri::AppHandle, slot: ShortcutSlot, action: StateMachineAction) {
    match action {
        StateMachineAction::StartHold => {
            // Mark session BEFORE starting to close the TOCTOU gap:
            // a fast key release between start and mark would miss the active_capture.
            if let Some(mgr_state) = app.try_state::<Arc<Mutex<ShortcutManager>>>() {
                if let Ok(mut mgr) = mgr_state.lock() {
                    mgr.mark_session_started(slot, CaptureStyle::Hold);
                }
            }
            if let Err(e) = start_slot_session(app, slot, CaptureStyle::Hold) {
                // Revert the mark on failure
                if let Some(mgr_state) = app.try_state::<Arc<Mutex<ShortcutManager>>>() {
                    if let Ok(mut mgr) = mgr_state.lock() {
                        if let Some(active) = mgr.slots.get_mut(&slot) {
                            active.state_machine.clear();
                        }
                    }
                }
                crate::commands::show_user_notification(app, slot.label(), &e);
            }
        }
        StateMachineAction::StartLocked => {
            if let Some(mgr_state) = app.try_state::<Arc<Mutex<ShortcutManager>>>() {
                if let Ok(mut mgr) = mgr_state.lock() {
                    mgr.mark_session_started(slot, CaptureStyle::Locked);
                }
            }
            if let Err(e) = start_slot_session(app, slot, CaptureStyle::Locked) {
                if let Some(mgr_state) = app.try_state::<Arc<Mutex<ShortcutManager>>>() {
                    if let Ok(mut mgr) = mgr_state.lock() {
                        if let Some(active) = mgr.slots.get_mut(&slot) {
                            active.state_machine.clear();
                        }
                    }
                }
                crate::commands::show_user_notification(app, slot.label(), &e);
            }
        }
        StateMachineAction::Stop { discard } => {
            stop_slot_session(app, slot, discard);
        }
        StateMachineAction::None => {}
    }
}

/// Schedule a hold-check timer. Called AFTER the lock is dropped.
pub fn schedule_hold_check(app: &tauri::AppHandle, slot: ShortcutSlot, generation: u64) {
    let app_clone = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(HOLD_THRESHOLD_MS));
        let mgr_state: tauri::State<Arc<Mutex<ShortcutManager>>> = match app_clone.try_state() {
            Some(state) => state,
            None => return,
        };
        let action = {
            let mgr = match mgr_state.lock() {
                Ok(mgr) => mgr,
                Err(e) => {
                    eprintln!("[shortcut_manager] mutex poisoned in hold check: {}", e);
                    return;
                }
            };
            if is_slot_session_active_fast(&app_clone, slot) {
                return;
            }
            mgr.check_hold_start(slot, generation)
        }; // lock dropped
        if matches!(action, StateMachineAction::StartHold) {
            execute_action(&app_clone, slot, StateMachineAction::StartHold);
        }
    });
}

/// Start a session for a slot.
fn start_slot_session(
    app: &tauri::AppHandle,
    slot: ShortcutSlot,
    style: CaptureStyle,
) -> Result<(), String> {
    match slot {
        ShortcutSlot::QuickThought => {
            let state = app.state::<crate::commands::AppState>();
            let capture_style = match style {
                CaptureStyle::Hold => crate::commands::HotkeyCaptureStyle::Hold,
                CaptureStyle::Locked => crate::commands::HotkeyCaptureStyle::Locked,
            };
            // Set up runtime tracking for the old start_recording path
            if let Ok(mut runtime) = state.hotkey_runtime.lock() {
                runtime.active_capture = Some(capture_style);
                runtime.recording_started_at = Some(Instant::now());
            }
            state
                .discard_short_hotkey_capture
                .store(false, Ordering::Relaxed);
            let hotkey_runtime = state.hotkey_runtime.clone();
            let discard_flag = state.discard_short_hotkey_capture.clone();
            crate::commands::launch_recording(
                app.clone(),
                &state,
                minutes_core::CaptureMode::QuickThought,
                Some(minutes_core::capture::RecordingIntent::Memo),
                false,
                None,
                None,
                Some(hotkey_runtime),
                Some(discard_flag),
            )
        }
        ShortcutSlot::Dictation => {
            let capture = match style {
                CaptureStyle::Hold => Some(crate::commands::HotkeyCaptureStyle::Hold),
                CaptureStyle::Locked => Some(crate::commands::HotkeyCaptureStyle::Locked),
            };
            crate::commands::start_dictation_session_public(app, capture)
        }
    }
}

/// Stop a session for a slot.
fn stop_slot_session(app: &tauri::AppHandle, slot: ShortcutSlot, discard: bool) {
    let state = match app.try_state::<crate::commands::AppState>() {
        Some(s) => s,
        None => return,
    };

    match slot {
        ShortcutSlot::QuickThought => {
            if discard {
                state
                    .discard_short_hotkey_capture
                    .store(true, Ordering::Relaxed);
            }
            if let Ok(mut runtime) = state.hotkey_runtime.lock() {
                runtime.active_capture = None;
                runtime.recording_started_at = None;
            }
            if let Err(e) = crate::commands::request_stop(&state.recording, &state.stop_flag) {
                crate::commands::show_user_notification(
                    app,
                    "Quick Thought",
                    &format!("Could not stop recording: {}", e),
                );
            }
        }
        ShortcutSlot::Dictation => {
            state.dictation_stop_flag.store(true, Ordering::Relaxed);
        }
    }
}

/// Emit shortcut status for a slot.
pub fn emit_status(app: &tauri::AppHandle, slot: ShortcutSlot) {
    if let Some(mgr_state) = app.try_state::<Arc<Mutex<ShortcutManager>>>() {
        if let Ok(mgr) = mgr_state.lock() {
            let status = mgr.build_status(slot);
            app.emit("shortcut:status", &status).ok();
        }
    }
}

// ── Native backend callbacks (called from CGEventTap thread) ─

#[cfg(target_os = "macos")]
fn handle_native_event_callback(
    app: &tauri::AppHandle,
    slot: ShortcutSlot,
    event: minutes_core::hotkey_macos::HotkeyEvent,
) {
    let mgr_state = match app.try_state::<Arc<Mutex<ShortcutManager>>>() {
        Some(state) => state,
        None => return,
    };

    match event {
        minutes_core::hotkey_macos::HotkeyEvent::Press => {
            let hold_info = {
                let mut mgr = match mgr_state.lock() {
                    Ok(mgr) => mgr,
                    Err(e) => {
                        eprintln!("[shortcut_manager] mutex poisoned on press: {}", e);
                        return;
                    }
                };
                mgr.handle_press(slot)
            }; // lock dropped
            if let Some((slot, generation)) = hold_info {
                schedule_hold_check(app, slot, generation);
            }
        }
        minutes_core::hotkey_macos::HotkeyEvent::Release => {
            let session_active = is_slot_session_active_fast(app, slot);
            let (_slot, action) = {
                let mut mgr = match mgr_state.lock() {
                    Ok(mgr) => mgr,
                    Err(e) => {
                        eprintln!("[shortcut_manager] mutex poisoned on release: {}", e);
                        return;
                    }
                };
                mgr.handle_release(slot, session_active)
            }; // lock dropped

            // Spawn action off the CGEventTap thread to avoid blocking the
            // macOS HID event pipeline with file I/O and window creation.
            if !matches!(action, StateMachineAction::None) {
                let app_clone = app.clone();
                std::thread::spawn(move || {
                    execute_action(&app_clone, slot, action);
                });
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn handle_native_status_callback(
    app: &tauri::AppHandle,
    slot: ShortcutSlot,
    generation: u64,
    status: minutes_core::hotkey_macos::HotkeyMonitorStatus,
) {
    use minutes_core::hotkey_macos::HotkeyMonitorStatus;

    let mgr_state = match app.try_state::<Arc<Mutex<ShortcutManager>>>() {
        Some(state) => state,
        None => return,
    };

    let should_prompt = {
        let mut mgr = match mgr_state.lock() {
            Ok(mgr) => mgr,
            Err(_) => return,
        };

        match status {
            HotkeyMonitorStatus::Starting => {
                mgr.update_native_status(slot, generation, NativeLifecycle::Starting, None);
                false
            }
            HotkeyMonitorStatus::Active => {
                mgr.update_native_status(slot, generation, NativeLifecycle::Active, None);
                false
            }
            HotkeyMonitorStatus::Failed(message) => {
                mgr.update_native_status(slot, generation, NativeLifecycle::Failed, Some(message));
                true
            }
            HotkeyMonitorStatus::Stopped => {
                mgr.update_native_status(slot, generation, NativeLifecycle::NotApplicable, None);
                false
            }
        }
    };

    if should_prompt {
        #[cfg(target_os = "macos")]
        minutes_core::hotkey_macos::open_input_monitoring_settings();
    }

    emit_status(app, slot);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_caps_lock_as_native() {
        assert_eq!(classify_shortcut(57), ShortcutBackend::Native);
    }

    #[test]
    fn classify_fn_as_native() {
        assert_eq!(classify_shortcut(63), ShortcutBackend::Native);
    }

    #[test]
    fn classify_regular_key_as_standard() {
        assert_eq!(classify_shortcut(0), ShortcutBackend::Standard);
        assert_eq!(classify_shortcut(-1), ShortcutBackend::Standard);
        assert_eq!(classify_shortcut(49), ShortcutBackend::Standard); // Space
    }

    #[test]
    fn slot_roundtrip() {
        assert_eq!(
            ShortcutSlot::from_str("quick_thought").unwrap(),
            ShortcutSlot::QuickThought
        );
        assert_eq!(
            ShortcutSlot::from_str("dictation").unwrap(),
            ShortcutSlot::Dictation
        );
        assert!(ShortcutSlot::from_str("bogus").is_err());
    }

    #[test]
    fn state_machine_hold_to_talk() {
        let mut sm = ShortcutStateMachine::default();

        // Press
        let action = sm.handle_press();
        assert!(matches!(action, StateMachineAction::None));
        assert!(sm.key_down);

        let gen = sm.hold_generation();

        // After threshold, still holding
        let action = sm.start_hold_if_still_down(gen);
        assert!(matches!(action, StateMachineAction::StartHold));

        // Mark session started
        sm.mark_session_started(CaptureStyle::Hold);

        // Release stops the hold
        let action = sm.handle_release(true);
        assert!(matches!(action, StateMachineAction::Stop { .. }));
    }

    #[test]
    fn state_machine_tap_to_lock() {
        let mut sm = ShortcutStateMachine::default();

        // Quick press
        sm.handle_press();

        // Quick release (before threshold) with no active session
        sm.key_down_started_at = Some(Instant::now()); // ensure it's "just now"
        let action = sm.handle_release(false);
        assert!(matches!(action, StateMachineAction::StartLocked));
    }

    #[test]
    fn state_machine_tap_to_stop_locked() {
        let mut sm = ShortcutStateMachine::default();

        // First tap to start (mocked as if session is now active)
        sm.handle_press();
        sm.key_down_started_at = Some(Instant::now());
        let _ = sm.handle_release(false); // Would return StartLocked

        // Session is now active. Second tap to stop.
        sm.handle_press();
        sm.key_down_started_at = Some(Instant::now());
        let action = sm.handle_release(true); // session active
        assert!(matches!(
            action,
            StateMachineAction::Stop { discard: false }
        ));
    }

    #[test]
    fn state_machine_ignores_key_repeat() {
        let mut sm = ShortcutStateMachine::default();
        sm.handle_press();
        assert!(sm.key_down);

        // Second press while key is down = ignored
        let action = sm.handle_press();
        assert!(matches!(action, StateMachineAction::None));
    }

    #[test]
    fn stale_hold_generation_ignored() {
        let mut sm = ShortcutStateMachine::default();
        sm.handle_press();
        let gen = sm.hold_generation();

        // Simulate: key was released and pressed again (new generation)
        sm.key_down = false;
        sm.handle_press();

        // Old generation check should return None
        let action = sm.start_hold_if_still_down(gen);
        assert!(matches!(action, StateMachineAction::None));
    }
}
