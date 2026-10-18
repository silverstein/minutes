//! Explicit, Dev-only acceptance entry point for LaunchServices-based QA.
//! This never starts a recording. The normal UI and consent checks still apply.

use std::ffi::OsString;
use std::path::PathBuf;

fn test_root(identifier: &str, arguments: &[OsString]) -> Result<PathBuf, &'static str> {
    if identifier != "com.useminutes.desktop.dev" {
        return Err("only Minutes Dev accepts the recovery test argument");
    }
    if arguments.len() != 2 || arguments[0] != "--capture-recovery-test-root" {
        return Err("provide exactly one absolute test directory");
    }
    let root = PathBuf::from(&arguments[1]);
    if !root.is_absolute() || !root.is_dir() {
        return Err("the test directory must already exist and be absolute");
    }
    root.canonicalize()
        .map_err(|_| "the test directory could not be resolved")
}

pub fn configure(identifier: &str) -> Result<(), String> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    let root = test_root(identifier, &arguments)?;
    let config_path = root.join("config.toml");
    if !config_path.is_file() {
        return Err("the test directory needs a valid config.toml".into());
    }
    let config = minutes_core::Config::load_strict_from(&config_path)?;
    let output_dir = config
        .output_dir
        .canonicalize()
        .map_err(|_| "create the test output directory before launch")?;
    if !output_dir.starts_with(&root)
        || config.knowledge.enabled
        || config.daily_notes.enabled
        || config.hooks.post_record.is_some()
    {
        return Err("test output must stay inside the test directory with exports disabled".into());
    }
    std::fs::create_dir_all(root.join("state"))
        .map_err(|_| "the isolated test state could not be created")?;
    // Called at the first line of main, before any application threads exist.
    std::env::set_var("MINUTES_CONFIG_PATH", config_path);
    std::env::set_var("MINUTES_DATA_DIR", root.join("state"));
    std::env::set_var("MINUTES_CAPTURE_RECOVERY_TEST", "1");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_acceptance_requires_dev_identity_and_exact_arguments() {
        let args = vec![
            "--capture-recovery-test-root".into(),
            std::env::temp_dir().into_os_string(),
        ];
        assert!(test_root("com.useminutes.desktop", &args).is_err());
        assert!(test_root("com.useminutes.desktop.dev", &args).is_ok());
        assert!(test_root("com.useminutes.desktop.dev", &args[..1]).is_err());
        let mut extra = args;
        extra.push("unexpected".into());
        assert!(test_root("com.useminutes.desktop.dev", &extra).is_err());
    }

    #[test]
    fn recovery_acceptance_rejects_relative_or_missing_directories() {
        for path in [
            PathBuf::from("."),
            std::env::temp_dir().join("minutes-missing-recovery-qa-root"),
        ] {
            assert!(test_root(
                "com.useminutes.desktop.dev",
                &["--capture-recovery-test-root".into(), path.into_os_string()]
            )
            .is_err());
        }
    }
}
