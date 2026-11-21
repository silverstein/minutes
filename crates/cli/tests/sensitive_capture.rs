use std::fs;
use std::process::Command;

#[test]
fn the_cli_sensitive_meeting_blocks_capture_until_explicitly_stopped() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("state");
    let config = dir.path().join("config.toml");
    let output_dir = dir.path().join("meetings");
    fs::create_dir(&data_dir).unwrap();
    fs::write(
        &config,
        format!(
            "output_dir = {}\n",
            serde_json::to_string(&output_dir).unwrap()
        ),
    )
    .unwrap();
    let minutes = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_minutes"))
            .args(args)
            .env("MINUTES_CONFIG_PATH", &config)
            .env("MINUTES_DATA_DIR", &data_dir)
            .env("MINUTES_HOME", dir.path().join("home"))
            .output()
            .unwrap()
    };

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
