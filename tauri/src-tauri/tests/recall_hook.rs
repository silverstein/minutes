//! Exercise the actual pre-GUI hook entrypoint in isolated child environments.
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

fn invoke(root: &Path, tab: Option<&str>, event: &str) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_minutes-app"));
    command
        .args(["--recall-hook", root.to_str().unwrap(), event])
        .env_remove("MINUTES_RECALL_TAB")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(tab) = tab {
        command.env("MINUTES_RECALL_TAB", tab);
    }
    let mut child = command.spawn().unwrap();
    // Invalid identities can exit before reading stdin.
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"session_id":"saved-session","prompt":""}"#);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Recall hook did not exit before its pre-GUI deadline");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    child.wait_with_output().unwrap()
}

#[test]
fn native_hook_routes_context_and_status_by_environment_without_gui_startup() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(root.join(".recall-root"), "root").unwrap();
    let tabs = ["tab-a", "tab-b", "tab-live"].map(|id| {
        serde_json::json!({
            "id":id, "title":id, "meeting_path":null, "claude_session_id":null
        })
    });
    let manifest = serde_json::to_vec(&serde_json::json!({
        "active":"tab-a", "tabs":tabs
    }))
    .unwrap();
    std::fs::write(root.join("recall-tabs.json"), &manifest).unwrap();
    for (id, focus) in [
        ("tab-a", "ALPHA"),
        ("tab-b", "BRAVO"),
        ("tab-live", "LIVE GUIDANCE"),
    ] {
        let dir = root.join("tabs").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".recall-tab"), "tab").unwrap();
        std::fs::write(dir.join("CLAUDE.md"), focus).unwrap();
    }
    for tab in [
        None,
        Some("unknown"),
        Some("../tab-a"),
        Some("tabs/tab-a"),
        Some("/tmp"),
    ] {
        let output = invoke(root, tab, "SessionStart");
        assert!(output.status.success());
        assert!(output.stdout.is_empty());
        assert!(output.stderr.is_empty());
        assert!(!root.join(".recall-state.json").exists());
        for id in ["tab-a", "tab-b", "tab-live"] {
            assert!(!root
                .join("tabs")
                .join(id)
                .join(".recall-state.json")
                .exists());
        }
    }
    for event in ["SessionStart", "UserPromptSubmit"] {
        for (id, focus) in [
            ("tab-a", "ALPHA"),
            ("tab-b", "BRAVO"),
            ("tab-live", "LIVE GUIDANCE"),
        ] {
            if event == "UserPromptSubmit" {
                let unchanged = invoke(root, Some(id), event);
                assert!(unchanged.status.success());
                assert!(
                    unchanged.stdout.is_empty(),
                    "unchanged prompts must not repeat context"
                );
                assert!(unchanged.stderr.is_empty());
                std::fs::write(
                    root.join("tabs").join(id).join("CLAUDE.md"),
                    format!("{focus} changed"),
                )
                .unwrap();
            }
            let output = invoke(root, Some(id), event);
            assert!(output.status.success());
            assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
            let context = json["hookSpecificOutput"]["additionalContext"]
                .as_str()
                .unwrap();
            assert_eq!(json["hookSpecificOutput"]["hookEventName"], event);
            for token in ["ALPHA", "BRAVO", "LIVE GUIDANCE"] {
                assert_eq!(context.contains(token), token == focus);
            }
            let state: serde_json::Value = serde_json::from_slice(
                &std::fs::read(root.join("tabs").join(id).join(".recall-state.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                std::fs::read(root.join("recall-tabs.json")).unwrap(),
                manifest,
                "hook helpers cannot rewrite the manifest"
            );
            assert_eq!(state["session_id"], "saved-session");
            assert_eq!(
                state["status"],
                if event == "SessionStart" {
                    "done"
                } else {
                    "working"
                }
            );
        }
    }
}

#[test]
fn native_hook_uses_the_explicit_dev_root_with_a_matching_production_tab() {
    use minutes_core::assistant_workspace::AssistantWorkspace;
    use std::fs;

    let home = tempfile::tempdir().unwrap();
    let data_dir = home.path().join(".minutes");
    let production = AssistantWorkspace::Production.root(&data_dir);
    let dev = AssistantWorkspace::Dev.root(&data_dir);
    let tab_id = "same-tab-id";
    let original = b"production-only focus";
    for (root, instructions) in [(&production, &original[..]), (&dev, b"dev-only focus")] {
        let tab = root.join("tabs").join(tab_id);
        fs::create_dir_all(&tab).unwrap();
        fs::write(root.join(".recall-root"), "root").unwrap();
        fs::write(tab.join(".recall-tab"), "tab").unwrap();
        fs::write(tab.join("CLAUDE.md"), instructions).unwrap();
    }
    let production_tab = production.join("tabs").join(tab_id);
    let production_state = production_tab.join(".recall-state.json");
    fs::write(&production_state, b"production state").unwrap();

    let output = invoke(&dev, Some(tab_id), "SessionStart");
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let context = String::from_utf8(output.stdout).unwrap();
    assert!(context.contains("dev-only focus"));
    assert!(!context.contains("production-only focus"));
    assert!(dev
        .join("tabs")
        .join(tab_id)
        .join(".recall-state.json")
        .is_file());
    assert_eq!(fs::read(production_state).unwrap(), b"production state");
    assert_eq!(
        fs::read(production_tab.join("CLAUDE.md")).unwrap(),
        original
    );
    assert!(!production_tab.join(".recall-injected-focus.json").exists());
}
