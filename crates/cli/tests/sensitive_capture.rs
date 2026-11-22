use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn minutes_command(dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_minutes"));
    command
        .env("MINUTES_CONFIG_PATH", dir.join("config.toml"))
        .env("MINUTES_DATA_DIR", dir.join("state"))
        .env("MINUTES_HOME", dir.join("home"))
        .env_remove("MINUTES_CLI_RESTRICTED_POLICY");
    command
}

fn isolated_minutes(dir: &Path) -> impl Fn(&[&str]) -> Output + '_ {
    let data_dir = dir.join("state");
    let config = dir.join("config.toml");
    let output_dir = dir.join("meetings");
    fs::create_dir(&data_dir).unwrap();
    fs::write(
        &config,
        format!(
            "output_dir = {}\n",
            serde_json::to_string(&output_dir).unwrap()
        ),
    )
    .unwrap();
    move |args: &[&str]| minutes_command(dir).args(args).output().unwrap()
}

#[test]
fn the_cli_sensitive_meeting_blocks_capture_until_explicitly_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let minutes = isolated_minutes(dir.path());
    let data_dir = dir.path().join("state");
    let owner = std::process::id().to_string();
    let state_file = data_dir.join("sensitive-session.json");
    for (file, mode) in [
        ("recording.pid", "recording"),
        ("dictation.pid", "dictation"),
        ("live-transcript.pid", "live transcript"),
        ("voice.pid", "voice session"),
    ] {
        let pid_file = data_dir.join(file);
        fs::write(&pid_file, &owner).unwrap();
        let result = minutes(&["sensitive", "start"]);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{mode} was running");
        assert!(error.contains(&format!("{mode} in progress")), "{error}");
        assert!(!state_file.exists());
        fs::remove_file(pid_file).unwrap();
    }

    let started = minutes(&["sensitive", "start", "--title", "Synthetic private meeting"]);
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );

    let assert_blocked = |args: &[&str]| {
        let result = minutes(args);
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(!result.status.success(), "{args:?} unexpectedly succeeded");
        assert!(
            error.contains("sensitive meeting already active"),
            "{error}"
        );
    };
    assert_blocked(&["record", "--mode", "quick-thought"]);
    assert_blocked(&["toggle-recording"]);
    assert_blocked(&["toggle-recording", "--mode", "live"]);
    assert_blocked(&["toggle-dictation"]);
    #[cfg(feature = "whisper")]
    {
        assert_blocked(&["dictate"]);
        assert_blocked(&["live"]);
    }
    assert!(state_file.exists(), "capture checks cleared the session");
    assert!(!data_dir.join("current.wav").exists());
    assert!(!data_dir.join("live-transcript.wav").exists());

    assert!(minutes(&["note", "synthetic marker"]).status.success());
    let stopped = minutes(&["sensitive", "stop"]);
    assert!(
        stopped.status.success(),
        "{}",
        String::from_utf8_lossy(&stopped.stderr)
    );
    assert!(!state_file.exists());
}

#[test]
fn stop_requires_an_explicit_flag_for_dictation() {
    let dir = tempfile::tempdir().unwrap();
    let minutes = isolated_minutes(dir.path());
    let data_dir = dir.path().join("state");

    let idle = minutes(&["stop", "--dictation"]);
    assert!(!idle.status.success());
    assert!(String::from_utf8_lossy(&idle.stderr).contains("No dictation in progress."));

    let pid_file = data_dir.join("dictation.pid");
    fs::write(&pid_file, std::process::id().to_string()).unwrap();
    let recording_stop = minutes(&["stop"]);
    assert!(!recording_stop.status.success());
    assert!(String::from_utf8_lossy(&recording_stop.stderr).contains("minutes stop --dictation"));

    let dictation_stop = minutes(&["stop", "--dictation"]);
    let error = String::from_utf8_lossy(&dictation_stop.stderr);
    assert!(
        error.contains("stop it there") || error.contains("stop it where it started"),
        "{error}"
    );
    assert!(pid_file.exists());
}

#[test]
fn status_reports_session_flags_only_in_the_full_cli_payload() {
    let dir = tempfile::tempdir().unwrap();
    let minutes = isolated_minutes(dir.path());
    let fields = [
        "dictation",
        "live_transcript",
        "voice_session",
        "sensitive_meeting",
    ];
    let idle = minutes(&["status"]);
    let idle: serde_json::Value = serde_json::from_slice(&idle.stdout).unwrap();
    for field in fields {
        assert_eq!(idle[field], false, "{field}");
    }

    let private_title = "Private status fixture";
    assert!(minutes(&["sensitive", "start", "--title", private_title])
        .status
        .success());
    let data_dir = dir.path().join("state");
    let owner = std::process::id().to_string();
    for path in ["dictation.pid", "live-transcript.pid", "voice.pid"] {
        fs::write(data_dir.join(path), &owner).unwrap();
    }
    let full = minutes(&["status"]);
    assert!(full.status.success());
    assert!(!String::from_utf8_lossy(&full.stdout).contains(private_title));
    let status: serde_json::Value = serde_json::from_slice(&full.stdout).unwrap();
    assert_eq!(status["recording"], false);
    for field in fields {
        assert_eq!(status[field], true, "{field}");
    }

    let restricted = minutes_command(dir.path())
        .arg("status")
        .env("MINUTES_CLI_RESTRICTED_POLICY", "deny")
        .output()
        .unwrap();
    assert!(restricted.status.success());
    let restricted: serde_json::Value = serde_json::from_slice(&restricted.stdout).unwrap();
    assert_eq!(restricted["recording"], false);
    for field in fields {
        assert!(restricted.get(field).is_none(), "{field}");
    }

    let stale = data_dir.join("dictation.pid");
    fs::write(&stale, "99999999").unwrap();
    let status: serde_json::Value = serde_json::from_slice(&minutes(&["status"]).stdout).unwrap();
    assert_eq!(status["dictation"], false);
    assert!(stale.exists(), "status should not remove stale PID files");
}
