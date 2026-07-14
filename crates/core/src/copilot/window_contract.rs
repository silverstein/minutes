use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowPlatform {
    Macos,
    Windows,
    Linux,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowEnvironment {
    pub platform: WindowPlatform,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux_session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub linux_desktop: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractSupport {
    Honored,
    BestEffort,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopilotWindowCapabilities {
    pub non_focusing: ContractSupport,
    pub always_on_top: ContractSupport,
    pub content_protected: ContractSupport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum CopilotHudContractDecision {
    Show {
        capabilities: CopilotWindowCapabilities,
    },
    WarnBeforeShow {
        capabilities: CopilotWindowCapabilities,
        warning: String,
    },
}

/// Evaluate the native window guarantees before showing live coaching.
///
/// macOS and Windows have concrete native exclusion APIs in Tao/Tauri.
/// Linux's Tao backend documents content protection as unsupported; X11 and
/// Wayland compositors also retain final authority over focus and z-order
/// hints. A Linux host must therefore obtain an explicit acknowledgement
/// before showing coaching when screen-share protection was requested.
pub fn evaluate_copilot_window_contract(
    environment: &WindowEnvironment,
    content_protection_requested: bool,
) -> CopilotHudContractDecision {
    match environment.platform {
        WindowPlatform::Macos | WindowPlatform::Windows => CopilotHudContractDecision::Show {
            capabilities: CopilotWindowCapabilities {
                non_focusing: ContractSupport::Honored,
                always_on_top: ContractSupport::Honored,
                content_protected: if content_protection_requested {
                    ContractSupport::Honored
                } else {
                    ContractSupport::Unavailable
                },
            },
        },
        WindowPlatform::Linux => {
            let capabilities = CopilotWindowCapabilities {
                non_focusing: ContractSupport::BestEffort,
                always_on_top: ContractSupport::BestEffort,
                content_protected: ContractSupport::Unavailable,
            };
            let session = environment.linux_session.as_deref().unwrap_or("unknown");
            let desktop = environment.linux_desktop.as_deref().unwrap_or("your window manager");
            let warning = if content_protection_requested {
                format!(
                    "Minutes cannot keep the coaching overlay out of screen sharing on {desktop} ({session}). Your window manager also controls whether it stays above the meeting without taking focus. Hide the overlay while sharing, or confirm that you want to show it anyway."
                )
            } else {
                format!(
                    "On {desktop} ({session}), the window manager controls whether coaching stays above the meeting without taking focus. Confirm that you want to show the overlay."
                )
            };
            CopilotHudContractDecision::WarnBeforeShow {
                capabilities,
                warning,
            }
        }
        WindowPlatform::Other => CopilotHudContractDecision::WarnBeforeShow {
            capabilities: CopilotWindowCapabilities {
                non_focusing: ContractSupport::BestEffort,
                always_on_top: ContractSupport::BestEffort,
                content_protected: ContractSupport::Unavailable,
            },
            warning: "Minutes cannot verify the coaching overlay's focus, always-on-top, or screen-share protection on this desktop. Confirm that you want to show it.".into(),
        },
    }
}

pub fn current_window_environment() -> WindowEnvironment {
    #[cfg(target_os = "macos")]
    let platform = WindowPlatform::Macos;
    #[cfg(target_os = "windows")]
    let platform = WindowPlatform::Windows;
    #[cfg(target_os = "linux")]
    let platform = WindowPlatform::Linux;
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    let platform = WindowPlatform::Other;

    WindowEnvironment {
        platform,
        linux_session: (platform == WindowPlatform::Linux).then(|| {
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| {
                if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    "wayland".into()
                } else if std::env::var_os("DISPLAY").is_some() {
                    "x11".into()
                } else {
                    "unknown".into()
                }
            })
        }),
        linux_desktop: (platform == WindowPlatform::Linux).then(|| {
            std::env::var("XDG_CURRENT_DESKTOP")
                .or_else(|_| std::env::var("DESKTOP_SESSION"))
                .unwrap_or_else(|_| "unknown desktop".into())
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linux(session: &str, desktop: &str) -> WindowEnvironment {
        WindowEnvironment {
            platform: WindowPlatform::Linux,
            linux_session: Some(session.into()),
            linux_desktop: Some(desktop.into()),
        }
    }

    #[test]
    fn macos_and_windows_report_the_native_overlay_contract() {
        for platform in [WindowPlatform::Macos, WindowPlatform::Windows] {
            let decision = evaluate_copilot_window_contract(
                &WindowEnvironment {
                    platform,
                    linux_session: None,
                    linux_desktop: None,
                },
                true,
            );
            assert_eq!(
                decision,
                CopilotHudContractDecision::Show {
                    capabilities: CopilotWindowCapabilities {
                        non_focusing: ContractSupport::Honored,
                        always_on_top: ContractSupport::Honored,
                        content_protected: ContractSupport::Honored,
                    }
                }
            );
        }
    }

    #[test]
    fn common_linux_window_managers_degrade_honestly_before_showing() {
        for environment in [
            linux("wayland", "GNOME"),
            linux("x11", "GNOME"),
            linux("wayland", "KDE"),
            linux("x11", "KDE"),
            linux("x11", "XFCE"),
            linux("wayland", "Sway"),
        ] {
            let decision = evaluate_copilot_window_contract(&environment, true);
            let CopilotHudContractDecision::WarnBeforeShow {
                capabilities,
                warning,
            } = decision
            else {
                panic!("Linux must warn before showing protected coaching");
            };
            assert_eq!(capabilities.content_protected, ContractSupport::Unavailable);
            assert_eq!(capabilities.non_focusing, ContractSupport::BestEffort);
            assert_eq!(capabilities.always_on_top, ContractSupport::BestEffort);
            assert!(warning.contains("cannot keep the coaching overlay out of screen sharing"));
            assert!(warning.contains("confirm"));
        }
    }

    #[test]
    fn unknown_platform_never_claims_window_guarantees() {
        let decision = evaluate_copilot_window_contract(
            &WindowEnvironment {
                platform: WindowPlatform::Other,
                linux_session: None,
                linux_desktop: None,
            },
            true,
        );
        assert!(matches!(
            decision,
            CopilotHudContractDecision::WarnBeforeShow { .. }
        ));
    }
}
