//! Applies `customize.toml` to the running app. `minutes_core::customize` owns
//! the file; this module forces the appearance, hands windows the accent, and
//! keeps the tray icon matched to the real menu bar.

use std::sync::Mutex;
use std::time::Duration;

use minutes_core::customize::{self, Customization, ThemeMode, ThemeSettings};
use serde::Serialize;
use tauri::{Emitter, Manager};

use crate::{sync_tray_appearance, TrayAppearance, TrayAppearanceState};

const THEME_CHANGED_EVENT: &str = "customize-theme-changed";
const SYSTEM_APPEARANCE_POLL: Duration = Duration::from_secs(5);

/// The customization currently applied to the app.
pub struct ThemeState(Mutex<Customization>);

impl ThemeState {
    pub fn new(initial: Customization) -> Self {
        Self(Mutex::new(initial))
    }

    fn get(&self) -> Customization {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Store `next` and return whether it differs from the current value.
    fn replace(&self, next: Customization) -> bool {
        let mut current = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = *current != next;
        *current = next;
        changed
    }
}

/// What a window needs to paint the theme, and what Settings needs to show it.
#[derive(Debug, Clone, Serialize)]
pub struct ResolvedTheme {
    mode: ThemeMode,
    accent_light: Option<String>,
    accent_dark: Option<String>,
    accent_light_hex: Option<String>,
    accent_dark_hex: Option<String>,
    css: String,
    problems: Vec<String>,
    path: String,
}

fn resolved(customization: &Customization) -> ResolvedTheme {
    ResolvedTheme {
        mode: customization.theme.mode,
        accent_light: customization.theme.accent_light.clone(),
        accent_dark: customization.theme.accent_dark.clone(),
        accent_light_hex: customization
            .theme
            .accent_light
            .as_deref()
            .and_then(customize::accent_hex),
        accent_dark_hex: customization
            .theme
            .accent_dark
            .as_deref()
            .and_then(customize::accent_hex),
        css: customize::theme_css(&customization.theme),
        problems: customization.problems.clone(),
        path: customize::customize_path().display().to_string(),
    }
}

/// Plugin whose init script gives every window the launch-time theme before its
/// first paint. The script only sets a global: under WKWebView it runs before
/// `documentElement` exists, so `scripts/customize-theme.js` applies it.
pub fn plugin<R: tauri::Runtime>(initial: &Customization) -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("minutes-theme")
        .js_init_script(init_script(initial))
        .build()
}

fn init_script(customization: &Customization) -> String {
    let payload = serde_json::to_string(&resolved(customization))
        .unwrap_or_else(|_| "null".into())
        .replace('<', "\\u003c");
    format!("window.__MINUTES_THEME__ = {payload};")
}

fn native_theme(mode: ThemeMode) -> Option<tauri::Theme> {
    match mode {
        ThemeMode::Auto => None,
        ThemeMode::Light => Some(tauri::Theme::Light),
        ThemeMode::Dark => Some(tauri::Theme::Dark),
    }
}

/// Force the launch-time appearance before any window is built, and start
/// watching the OS appearance for the tray.
pub fn apply_at_startup(app: &tauri::AppHandle) {
    let mode = app.state::<ThemeState>().get().theme.mode;
    if mode != ThemeMode::Auto {
        app.set_theme(native_theme(mode));
    }
    let app = app.clone();
    let _ = std::thread::Builder::new()
        .name("minutes-tray-appearance".into())
        .spawn(move || loop {
            std::thread::sleep(SYSTEM_APPEARANCE_POLL);
            // A forced window gets no ThemeChanged when the OS flips, so the
            // tray has to ask. In auto mode ThemeChanged already handles this.
            if is_forced(&app) {
                refresh_tray(&app, None);
            }
        });
}

fn is_forced(app: &tauri::AppHandle) -> bool {
    app.try_state::<ThemeState>()
        .is_some_and(|state| state.get().theme.mode != ThemeMode::Auto)
}

/// The appearance the tray icon should use. The menu bar follows the OS, not
/// this app, so a window's theme can only be trusted when nothing is forced.
pub fn tray_appearance(
    app: &tauri::AppHandle,
    window_theme: Option<tauri::Theme>,
) -> Option<TrayAppearance> {
    let mode = app
        .try_state::<ThemeState>()
        .map_or(ThemeMode::Auto, |state| state.get().theme.mode);
    tray_appearance_for(mode, customize::system_prefers_dark(), window_theme)
}

fn tray_appearance_for(
    mode: ThemeMode,
    system_prefers_dark: Option<bool>,
    window_theme: Option<tauri::Theme>,
) -> Option<TrayAppearance> {
    if mode == ThemeMode::Auto {
        return window_theme.map(TrayAppearance::from_theme);
    }
    system_prefers_dark.map(|dark| {
        if dark {
            TrayAppearance::Dark
        } else {
            TrayAppearance::Light
        }
    })
}

/// Repaint the tray if its appearance changed.
pub fn refresh_tray(app: &tauri::AppHandle, window_theme: Option<tauri::Theme>) {
    let Some(next) = tray_appearance(app, window_theme) else {
        return;
    };
    if let Some(state) = app.try_state::<TrayAppearanceState>() {
        if state.get() != next {
            state.set(next);
            sync_tray_appearance(app);
        }
    }
}

/// Re-read the file and apply any change. Hand and agent edits arrive here when
/// a window opens or the main window regains focus.
pub fn refresh(app: &tauri::AppHandle) -> Customization {
    let next = customize::load();
    let state = app.state::<ThemeState>();
    let previous_mode = state.get().theme.mode;
    if state.replace(next.clone()) {
        if next.theme.mode != previous_mode {
            app.set_theme(native_theme(next.theme.mode));
            let window_theme = app
                .get_webview_window("main")
                .and_then(|window| window.theme().ok());
            refresh_tray(app, window_theme);
        }
        if let Err(error) = app.emit(THEME_CHANGED_EVENT, resolved(&next)) {
            tracing::warn!(%error, "failed to announce the theme change");
        }
    }
    next
}

/// Return the current theme, picking up any edit to the file.
#[tauri::command]
pub fn cmd_get_customization(app: tauri::AppHandle) -> ResolvedTheme {
    resolved(&refresh(&app))
}

/// Save theme settings from the Settings panel. A blank accent means the
/// built-in color.
#[tauri::command]
pub fn cmd_set_theme(
    app: tauri::AppHandle,
    mode: String,
    accent_light: Option<String>,
    accent_dark: Option<String>,
) -> Result<ResolvedTheme, String> {
    let blank_to_none = |value: Option<String>| {
        value
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
    };
    let theme = ThemeSettings {
        mode: ThemeMode::parse(&mode).ok_or_else(|| format!("Unknown theme mode: {mode}"))?,
        accent_light: blank_to_none(accent_light),
        accent_dark: blank_to_none(accent_dark),
    };
    customize::save_theme(&customize::customize_path(), &theme)?;
    Ok(resolved(&refresh(&app)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_mode_keeps_using_the_window_theme() {
        assert_eq!(
            tray_appearance_for(ThemeMode::Auto, Some(false), Some(tauri::Theme::Dark)),
            Some(TrayAppearance::Dark)
        );
        assert_eq!(
            tray_appearance_for(ThemeMode::Auto, Some(true), Some(tauri::Theme::Light)),
            Some(TrayAppearance::Light)
        );
    }

    #[test]
    fn a_forced_theme_never_decides_the_tray_icon() {
        // Forced light on a dark menu bar must keep the dark-menu-bar icon.
        assert_eq!(
            tray_appearance_for(ThemeMode::Light, Some(true), Some(tauri::Theme::Light)),
            Some(TrayAppearance::Dark)
        );
        assert_eq!(
            tray_appearance_for(ThemeMode::Dark, Some(false), Some(tauri::Theme::Dark)),
            Some(TrayAppearance::Light)
        );
    }

    #[test]
    fn forced_theme_without_an_os_reading_leaves_the_tray_alone() {
        assert_eq!(
            tray_appearance_for(ThemeMode::Dark, None, Some(tauri::Theme::Dark)),
            None
        );
    }

    #[test]
    fn init_script_cannot_close_a_script_tag() {
        let customization = Customization {
            problems: vec!["</script><script>alert(1)</script>".into()],
            ..Customization::default()
        };
        assert!(!init_script(&customization).contains('<'));
    }

    #[test]
    fn post_dictation_command_is_visible_in_the_dictation_tab() {
        let index = include_str!("../../src/index.html");
        let (_, dictation) = index
            .split_once("id=\"panel-dictation\"")
            .expect("dictation tab should exist");
        let setup = dictation
            .split("id=\"dictation-pane-words\"")
            .next()
            .expect("dictation setup pane should be bounded");
        let (before, _) = setup
            .split_once("id=\"settings-hooks-post-dictation\"")
            .expect("post-dictation field should be in the dictation setup pane");
        assert_eq!(
            before.matches("<details").count(),
            before.matches("</details>").count(),
            "post-dictation field should not be inside a closed disclosure"
        );
    }

    #[test]
    fn dictation_actions_use_the_accent_not_a_fixed_color() {
        assert!(!include_str!("../../src/index.html").contains("--dictation-action-bg"));
        assert!(!include_str!("../../src/styles/dictation-experience.css")
            .contains("--dictation-action-bg"));
    }

    #[test]
    fn every_window_loads_the_theme_script() {
        for html in [
            include_str!("../../src/index.html"),
            include_str!("../../src/note.html"),
            include_str!("../../src/meeting-prompt.html"),
            include_str!("../../src/dictation-overlay.html"),
            include_str!("../../src/copilot-hud.html"),
            include_str!("../../src/terminal.html"),
            include_str!("../../src/palette/index.html"),
        ] {
            let head = html.split("</head>").next().unwrap();
            assert!(head.contains("customize-theme.js\"></script>"));
        }
    }
}
