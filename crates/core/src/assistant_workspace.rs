//! Recall workspace selection for the desktop app and its shared core writers.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssistantWorkspace {
    Production,
    Dev,
}

static DESKTOP_WORKSPACE: OnceLock<AssistantWorkspace> = OnceLock::new();

impl AssistantWorkspace {
    pub fn from_desktop_identifier(identifier: &str) -> Self {
        match identifier {
            "com.useminutes.desktop.dev" => Self::Dev,
            _ => Self::Production,
        }
    }

    /// Select the workspace early in desktop startup, including processing
    /// workers launched with the same executable. This stays process-local.
    pub fn configure_desktop(identifier: &str) {
        DESKTOP_WORKSPACE.get_or_init(|| Self::from_desktop_identifier(identifier));
    }

    /// Standalone CLI and MCP processes use the production workspace.
    pub fn current() -> Self {
        DESKTOP_WORKSPACE.get().copied().unwrap_or(Self::Production)
    }

    pub fn root(self, minutes_dir: &Path) -> PathBuf {
        minutes_dir.join(match self {
            Self::Production => "assistant",
            Self::Dev => "assistant-dev",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_dev_bundle_identifier_changes_the_root() {
        let data_dir = Path::new("test-home").join(".minutes");
        for (identifier, directory) in [
            ("com.useminutes.desktop", "assistant"),
            ("com.useminutes.desktop.dev", "assistant-dev"),
            ("com.useminutes.desktop.preview", "assistant"),
        ] {
            assert_eq!(
                AssistantWorkspace::from_desktop_identifier(identifier).root(&data_dir),
                data_dir.join(directory),
            );
        }
        assert_eq!(
            AssistantWorkspace::current().root(&data_dir),
            data_dir.join("assistant"),
        );
    }
}
