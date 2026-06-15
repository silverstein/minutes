//! Structured macOS permission state for desktop readiness surfaces.
//!
//! This module separates OS/TCC permission truth from feature readiness. A
//! device can exist while microphone permission is still denied, and a
//! permission can be granted in System Settings while the current process still
//! needs a relaunch before the relevant API works.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MacPermissionKind {
    Microphone,
    ScreenRecording,
    InputMonitoring,
    Accessibility,
    Automation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacPermissionStatus {
    Granted,
    Denied,
    NotDetermined,
    NotNeeded,
    Unsupported,
    StaleOrRestartNeeded,
    #[default]
    Unknown,
}

impl MacPermissionStatus {
    pub fn is_granted(self) -> bool {
        matches!(self, Self::Granted | Self::NotNeeded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MacPermissionRow {
    pub kind: MacPermissionKind,
    pub label: String,
    pub status: MacPermissionStatus,
    pub runtime_usable: bool,
    pub optional: bool,
    pub required_for: Vec<String>,
    pub detail: String,
    pub settings_url: Option<String>,
    pub can_open_settings: bool,
    pub can_request: bool,
    pub restart_recommended: bool,
    pub restart_blocked_by: Vec<String>,
}

struct MacPermissionRowSpec<'a> {
    kind: MacPermissionKind,
    label: &'a str,
    status: MacPermissionStatus,
    optional: bool,
    required_for: &'a [&'a str],
    detail: String,
    settings_url: Option<&'a str>,
    can_request: bool,
}

impl MacPermissionRow {
    fn new(spec: MacPermissionRowSpec<'_>) -> Self {
        let runtime_usable = spec.status.is_granted();
        Self {
            kind: spec.kind,
            label: spec.label.into(),
            status: spec.status,
            runtime_usable,
            optional: spec.optional,
            required_for: spec
                .required_for
                .iter()
                .map(|value| (*value).into())
                .collect(),
            detail: spec.detail,
            settings_url: spec.settings_url.map(str::to_string),
            can_open_settings: spec.settings_url.is_some(),
            can_request: spec.can_request,
            restart_recommended: false,
            restart_blocked_by: Vec::new(),
        }
    }

    fn mark_runtime_unusable(&mut self, detail: &str) {
        self.runtime_usable = false;
        self.restart_recommended = true;
        if !detail.is_empty() {
            self.detail = format!("{} {}", self.detail, detail);
        }
    }
}

pub fn permission_rows() -> Vec<MacPermissionRow> {
    vec![
        microphone_row(),
        screen_recording_row(),
        input_monitoring_row(),
        accessibility_row(),
        automation_row(),
    ]
}

pub fn microphone_status() -> MacPermissionStatus {
    platform::microphone_status()
}

pub fn screen_recording_status() -> MacPermissionStatus {
    platform::screen_recording_status()
}

pub fn microphone_row() -> MacPermissionRow {
    let status = microphone_status();
    MacPermissionRow::new(MacPermissionRowSpec {
        kind: MacPermissionKind::Microphone,
        label: "Microphone",
        status,
        optional: false,
        required_for: &["recording", "dictation", "live transcript"],
        detail: match status {
            MacPermissionStatus::Granted => {
                "Minutes can access microphone audio for recording and dictation.".into()
            }
            MacPermissionStatus::NotDetermined => {
                "macOS has not asked for microphone access yet.".into()
            }
            MacPermissionStatus::Denied => "Microphone access is denied in System Settings.".into(),
            MacPermissionStatus::NotNeeded => {
                "This platform does not require a separate microphone permission.".into()
            }
            MacPermissionStatus::Unsupported => {
                "Microphone permission checks are not supported on this platform.".into()
            }
            MacPermissionStatus::StaleOrRestartNeeded => {
                "macOS may need Minutes to restart before microphone access is usable.".into()
            }
            MacPermissionStatus::Unknown => {
                "Minutes could not determine microphone permission state.".into()
            }
        },
        settings_url: platform::MICROPHONE_SETTINGS_URL,
        can_request: true,
    })
}

pub fn screen_recording_row() -> MacPermissionRow {
    let status = screen_recording_status();
    MacPermissionRow::new(MacPermissionRowSpec {
        kind: MacPermissionKind::ScreenRecording,
        label: "Screen Recording",
        status,
        optional: true,
        required_for: &["native call capture", "desktop screenshots"],
        detail: match status {
            MacPermissionStatus::Granted => {
                "Minutes can use screen-capture APIs for optional call and context capture.".into()
            }
            MacPermissionStatus::Denied | MacPermissionStatus::NotDetermined => {
                "Screen Recording is not currently available to Minutes.".into()
            }
            MacPermissionStatus::NotNeeded => {
                "This platform does not require a separate Screen Recording permission.".into()
            }
            MacPermissionStatus::Unsupported => {
                "Screen Recording permission checks are not supported on this platform.".into()
            }
            MacPermissionStatus::StaleOrRestartNeeded => {
                "macOS may need Minutes to restart before Screen Recording is usable.".into()
            }
            MacPermissionStatus::Unknown => {
                "Minutes could not determine Screen Recording permission state.".into()
            }
        },
        settings_url: platform::SCREEN_RECORDING_SETTINGS_URL,
        can_request: true,
    })
}

pub fn input_monitoring_row() -> MacPermissionRow {
    let status = platform::input_monitoring_status();
    let mut row = MacPermissionRow::new(MacPermissionRowSpec {
        kind: MacPermissionKind::InputMonitoring,
        label: "Input Monitoring",
        status,
        optional: true,
        required_for: &["Caps Lock or fn dictation shortcut"],
        detail: match status {
            MacPermissionStatus::Granted => {
                "Minutes can observe the selected raw-key dictation shortcut.".into()
            }
            MacPermissionStatus::Denied | MacPermissionStatus::NotDetermined => {
                "Input Monitoring is not currently available to Minutes.".into()
            }
            MacPermissionStatus::NotNeeded => {
                "This platform does not require a separate Input Monitoring permission.".into()
            }
            MacPermissionStatus::Unsupported => {
                "Input Monitoring permission checks are not supported on this platform.".into()
            }
            MacPermissionStatus::StaleOrRestartNeeded => {
                "macOS may need Minutes to restart before Input Monitoring is usable.".into()
            }
            MacPermissionStatus::Unknown => {
                "Minutes could not determine Input Monitoring permission state.".into()
            }
        },
        settings_url: platform::INPUT_MONITORING_SETTINGS_URL,
        can_request: true,
    });

    if status == MacPermissionStatus::Granted {
        if let Some(detail) = platform::input_monitoring_runtime_issue() {
            row.mark_runtime_unusable(detail);
        }
    }

    row
}

pub fn accessibility_row() -> MacPermissionRow {
    let status = platform::accessibility_status();
    MacPermissionRow::new(MacPermissionRowSpec {
        kind: MacPermissionKind::Accessibility,
        label: "Accessibility",
        status,
        optional: true,
        required_for: &["desktop window titles", "browser/app context"],
        detail: match status {
            MacPermissionStatus::Granted => {
                "Minutes can read focused app and window context when enabled.".into()
            }
            MacPermissionStatus::Denied | MacPermissionStatus::NotDetermined => {
                "Accessibility is not currently available to Minutes.".into()
            }
            MacPermissionStatus::NotNeeded => {
                "This platform does not require a separate Accessibility permission.".into()
            }
            MacPermissionStatus::Unsupported => {
                "Accessibility permission checks are not supported on this platform.".into()
            }
            MacPermissionStatus::StaleOrRestartNeeded => {
                "macOS may need Minutes to restart before Accessibility is usable.".into()
            }
            MacPermissionStatus::Unknown => {
                "Minutes could not determine Accessibility permission state.".into()
            }
        },
        settings_url: platform::ACCESSIBILITY_SETTINGS_URL,
        can_request: true,
    })
}

pub fn automation_row() -> MacPermissionRow {
    let status = platform::automation_status();
    MacPermissionRow::new(MacPermissionRowSpec {
        kind: MacPermissionKind::Automation,
        label: "Automation",
        status,
        optional: true,
        required_for: &["browser tab detection", "calendar suggestions"],
        detail: match status {
            MacPermissionStatus::NotNeeded => {
                "Automation is target-app specific and checked only when a feature asks another app."
                    .into()
            }
            MacPermissionStatus::NotDetermined => {
                "Automation permission is granted per target app when Minutes first asks.".into()
            }
            _ => "Automation permission is target-app specific and checked at point of use.".into(),
        },
        settings_url: None,
        can_request: false,
    })
}

#[cfg(target_os = "macos")]
mod platform {
    use super::MacPermissionStatus;
    use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
    use std::time::Duration;

    pub const MICROPHONE_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
    pub const SCREEN_RECORDING_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
    pub const INPUT_MONITORING_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");
    pub const ACCESSIBILITY_SETTINGS_URL: Option<&str> =
        Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
    }

    pub fn microphone_status() -> MacPermissionStatus {
        unsafe {
            let Some(media_type) = AVMediaTypeAudio else {
                return MacPermissionStatus::Unsupported;
            };
            let status = AVCaptureDevice::authorizationStatusForMediaType(media_type);
            match status {
                AVAuthorizationStatus::NotDetermined => MacPermissionStatus::NotDetermined,
                AVAuthorizationStatus::Authorized => MacPermissionStatus::Granted,
                AVAuthorizationStatus::Restricted | AVAuthorizationStatus::Denied => {
                    MacPermissionStatus::Denied
                }
                _ => MacPermissionStatus::Unknown,
            }
        }
    }

    pub fn screen_recording_status() -> MacPermissionStatus {
        if unsafe { CGPreflightScreenCaptureAccess() } {
            MacPermissionStatus::Granted
        } else {
            MacPermissionStatus::Denied
        }
    }

    pub fn input_monitoring_status() -> MacPermissionStatus {
        if crate::hotkey_macos::is_input_monitoring_granted() {
            MacPermissionStatus::Granted
        } else {
            MacPermissionStatus::Denied
        }
    }

    pub fn input_monitoring_runtime_issue() -> Option<&'static str> {
        let probe = crate::hotkey_macos::probe_hotkey_monitor(
            crate::hotkey_macos::KEYCODE_FN,
            Duration::from_millis(250),
        );
        if probe.status == "active" {
            None
        } else {
            Some("macOS reports Input Monitoring as granted, but this running process could not start the native event tap. Restart Minutes and confirm the enabled System Settings entry matches this app copy.")
        }
    }

    pub fn accessibility_status() -> MacPermissionStatus {
        if crate::hotkey_macos::is_accessibility_trusted() {
            MacPermissionStatus::Granted
        } else {
            MacPermissionStatus::Denied
        }
    }

    pub fn automation_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::MacPermissionStatus;

    pub const MICROPHONE_SETTINGS_URL: Option<&str> = None;
    pub const SCREEN_RECORDING_SETTINGS_URL: Option<&str> = None;
    pub const INPUT_MONITORING_SETTINGS_URL: Option<&str> = None;
    pub const ACCESSIBILITY_SETTINGS_URL: Option<&str> = None;

    pub fn microphone_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }

    pub fn screen_recording_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }

    pub fn input_monitoring_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }

    pub fn input_monitoring_runtime_issue() -> Option<&'static str> {
        None
    }

    pub fn accessibility_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }

    pub fn automation_status() -> MacPermissionStatus {
        MacPermissionStatus::NotNeeded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn granted_status_is_runtime_granted() {
        assert!(MacPermissionStatus::Granted.is_granted());
        assert!(MacPermissionStatus::NotNeeded.is_granted());
        assert!(!MacPermissionStatus::Denied.is_granted());
        assert!(!MacPermissionStatus::StaleOrRestartNeeded.is_granted());
    }

    #[test]
    fn row_uses_status_for_initial_runtime_usable() {
        let granted = MacPermissionRow::new(MacPermissionRowSpec {
            kind: MacPermissionKind::InputMonitoring,
            label: "Input Monitoring",
            status: MacPermissionStatus::Granted,
            optional: true,
            required_for: &["dictation"],
            detail: "ok".into(),
            settings_url: Some("settings://example"),
            can_request: true,
        });
        assert!(granted.runtime_usable);

        let denied = MacPermissionRow::new(MacPermissionRowSpec {
            kind: MacPermissionKind::InputMonitoring,
            label: "Input Monitoring",
            status: MacPermissionStatus::Denied,
            optional: true,
            required_for: &["dictation"],
            detail: "no".into(),
            settings_url: Some("settings://example"),
            can_request: true,
        });
        assert!(!denied.runtime_usable);
    }

    #[test]
    fn row_can_represent_granted_but_runtime_unusable() {
        let mut row = MacPermissionRow::new(MacPermissionRowSpec {
            kind: MacPermissionKind::InputMonitoring,
            label: "Input Monitoring",
            status: MacPermissionStatus::Granted,
            optional: true,
            required_for: &["dictation"],
            detail: "granted".into(),
            settings_url: Some("settings://example"),
            can_request: true,
        });

        row.mark_runtime_unusable("restart needed");

        assert_eq!(row.status, MacPermissionStatus::Granted);
        assert!(!row.runtime_usable);
        assert!(row.restart_recommended);
        assert!(row.detail.contains("restart needed"));
    }

    #[test]
    fn automation_is_not_a_global_actionable_permission() {
        let row = automation_row();
        assert_eq!(row.status, MacPermissionStatus::NotNeeded);
        assert!(row.runtime_usable);
        assert!(!row.can_open_settings);
        assert!(!row.can_request);
    }

    #[test]
    fn permission_rows_cover_core_macos_permissions() {
        let rows = permission_rows();
        let kinds: std::collections::HashSet<_> = rows.iter().map(|row| row.kind).collect();
        assert!(kinds.contains(&MacPermissionKind::Microphone));
        assert!(kinds.contains(&MacPermissionKind::ScreenRecording));
        assert!(kinds.contains(&MacPermissionKind::InputMonitoring));
        assert!(kinds.contains(&MacPermissionKind::Accessibility));
        assert!(kinds.contains(&MacPermissionKind::Automation));
    }

    #[test]
    fn row_serializes_with_camel_case_fields() {
        let row = MacPermissionRow::new(MacPermissionRowSpec {
            kind: MacPermissionKind::InputMonitoring,
            label: "Input Monitoring",
            status: MacPermissionStatus::StaleOrRestartNeeded,
            optional: true,
            required_for: &["fn dictation shortcut"],
            detail: "restart needed".into(),
            settings_url: Some("settings://example"),
            can_request: false,
        });
        let value = serde_json::to_value(row).expect("serialize row");
        assert_eq!(value["kind"], "inputMonitoring");
        assert_eq!(value["status"], "stale_or_restart_needed");
        assert_eq!(value["runtimeUsable"], false);
        assert_eq!(value["restartRecommended"], false);
    }
}
