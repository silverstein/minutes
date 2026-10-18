//! Filesystem and subprocess checks shared by CLI setup and its portable tests.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub(crate) fn executable_candidates(output: &Output) -> Vec<PathBuf> {
    if !output.status.success() {
        return Vec::new();
    }
    let mut seen = std::collections::HashSet::new();
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|line| PathBuf::from(line.trim()))
        .filter(|path| is_executable(path))
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    path.is_absolute()
        && path
            .metadata()
            .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

pub(crate) fn known_install_method(path: &Path) -> Option<&'static str> {
    let resolved = path.canonicalize().ok()?;
    if resolved.ancestors().any(|parent| {
        matches!(
            parent.file_name().and_then(|name| name.to_str()),
            Some("Minutes.app" | "Minutes Dev.app")
        )
    }) {
        Some("bundled")
    } else if path.to_string_lossy().contains(".cargo/bin/") {
        Some("cargo")
    } else {
        None
    }
}

pub(crate) fn find_homebrew(path_candidate: Option<PathBuf>) -> Option<PathBuf> {
    path_candidate
        .into_iter()
        .chain([
            PathBuf::from("/opt/homebrew/bin/brew"),
            PathBuf::from("/usr/local/bin/brew"),
        ])
        .find(|path| is_executable(path))
}

pub(crate) fn homebrew_owns_binary(brew: &Path, binary: &Path) -> bool {
    let Ok(binary) = binary.canonicalize() else {
        return false;
    };
    let Ok(output) = Command::new(brew)
        .args(["list", "--formula", "--verbose", "silverstein/tap/minutes"])
        .output()
    else {
        return false;
    };
    output.status.success()
        && String::from_utf8_lossy(&output.stdout).lines().any(|line| {
            Path::new(line.trim())
                .canonicalize()
                .is_ok_and(|path| path == binary)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};

    fn executable(path: &Path, script: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, script).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn failed_shell_lookup_never_becomes_a_binary_candidate() {
        let mut output = Command::new("sh")
            .args(["-c", "printf 'minutes not found\\n'; exit 1"])
            .output()
            .unwrap();
        assert!(executable_candidates(&output).is_empty());
        // Even a valid executable printed by shell startup code cannot turn
        // the failed lookup into a successful detection.
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("minutes");
        executable(&binary, "#!/bin/sh\n");
        output.stdout = format!("{}\n", binary.display()).into_bytes();
        assert!(executable_candidates(&output).is_empty());
    }

    #[test]
    fn shell_candidates_require_executable_absolute_files_and_preserve_path_order() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first/minutes");
        let second = root.path().join("second/minutes");
        let text = root.path().join("text");
        executable(&first, "#!/bin/sh\n");
        executable(&second, "#!/bin/sh\n");
        std::fs::write(&text, "not an executable").unwrap();
        let mut output = Command::new("true").output().unwrap();
        output.stdout = format!(
            "minutes not found\n{}\n{}\n{}\n{}\n{}\n/missing-minutes\n",
            first.display(),
            text.display(),
            root.path().display(),
            first.display(),
            second.display()
        )
        .into_bytes();
        assert_eq!(executable_candidates(&output), vec![first, second]);
    }

    #[test]
    fn setup_symlink_is_classified_as_bundled() {
        let root = tempfile::tempdir().unwrap();
        for name in ["Minutes.app", "Minutes Dev.app"] {
            let binary = root.path().join(name).join("Contents/MacOS/minutes");
            executable(&binary, "#!/bin/sh\n");
            let link = root.path().join("minutes");
            symlink(&binary, &link).unwrap();
            assert_eq!(known_install_method(&link), Some("bundled"));
            std::fs::remove_file(&link).unwrap();
        }
    }

    #[test]
    fn brew_detection_requires_formula_membership_of_the_selected_binary() {
        let root = tempfile::tempdir().unwrap();
        let binary = root.path().join("formula/minutes");
        let custom = root.path().join("custom/minutes");
        let brew = root.path().join("brew");
        executable(&binary, "#!/bin/sh\n");
        executable(&custom, "#!/bin/sh\n");
        // Exit unsuccessfully unless the production invocation explicitly
        // asks for formula files; a cask must never satisfy this probe.
        executable(&brew, "#!/bin/sh\n[ \"$*\" = 'list --formula --verbose silverstein/tap/minutes' ] || exit 1\nprintf '%s/formula/minutes\\n' \"$(dirname \"$0\")\"\n");
        assert_eq!(find_homebrew(Some(brew.clone())), Some(brew.clone()));
        assert!(homebrew_owns_binary(&brew, &binary));
        let link = root.path().join("minutes");
        symlink(&binary, &link).unwrap();
        assert!(homebrew_owns_binary(&brew, &link));
        assert!(!homebrew_owns_binary(&brew, &custom));
        executable(&brew, "#!/bin/sh\nexit 1\n");
        assert!(!homebrew_owns_binary(&brew, &binary));
    }
}
