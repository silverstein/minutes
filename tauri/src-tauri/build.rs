use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    compile_system_audio_helper();
    compile_calendar_helper();
    stage_assistant_skill_bundle();
    tauri_build::build()
}

fn compile_system_audio_helper() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"),
    );
    let source = manifest_dir.join("src/system_audio_record.swift");
    let bin_dir = manifest_dir.join("bin");
    let binary = bin_dir.join("system_audio_record");
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown-target".into());
    let target_binary = bin_dir.join(format!("system_audio_record-{}", target));

    println!("cargo:rerun-if-changed={}", source.display());
    std::fs::create_dir_all(&bin_dir).expect("failed to create helper bin dir");

    let output = Command::new("swiftc")
        .args(["-parse-as-library"])
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("failed to run swiftc for system_audio_record");

    if !output.status.success() {
        panic!(
            "failed to compile system_audio_record.swift: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    std::fs::copy(&binary, &target_binary)
        .expect("failed to copy target-specific system_audio_record helper");
}

fn compile_calendar_helper() {
    // The Swift EventKit helper (`calendar-events`) is the fast, non-intrusive
    // path for reading Apple Calendar. Without it, callers fall through to
    // AppleScript — which, since PR #164, returns empty unless Calendar.app
    // is already running. Bundling the helper inside the signed .app means
    // release DMG users get real calendar data regardless of Calendar.app state.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"),
    );
    let repo_root = manifest_dir.join("../..");
    let source = repo_root.join("scripts/calendar-events.swift");
    let info_plist = repo_root.join("scripts/calendar-helper-Info.plist");
    let resources_dir = manifest_dir.join("resources");
    let binary = resources_dir.join("calendar-events");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", info_plist.display());

    fs::create_dir_all(&resources_dir).expect("failed to create tauri resources dir");

    // Mirrors `scripts/build.sh` / `scripts/install-dev-app.sh`: `-sectcreate
    // __TEXT __info_plist` embeds the plist so macOS can display the
    // NSCalendarsFullAccessUsageDescription string on the EventKit prompt.
    let output = Command::new("swiftc")
        .arg("-O")
        .args(["-Xlinker", "-sectcreate"])
        .args(["-Xlinker", "__TEXT"])
        .args(["-Xlinker", "__info_plist"])
        .arg("-Xlinker")
        .arg(&info_plist)
        .arg(&source)
        .arg("-o")
        .arg(&binary)
        .output()
        .expect("failed to run swiftc for calendar-events");

    if !output.status.success() {
        panic!(
            "failed to compile calendar-events.swift: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn stage_assistant_skill_bundle() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"),
    );
    let repo_root = manifest_dir.join("../..");
    let resources_root = manifest_dir
        .join("resources")
        .join("assistant-skill-bundle");

    let sources = [
        ("agents-skills", repo_root.join(".agents").join("skills")),
        (
            "opencode-skills",
            repo_root.join(".opencode").join("skills"),
        ),
        (
            "opencode-commands",
            repo_root.join(".opencode").join("commands"),
        ),
    ];

    for (_, source) in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }

    if resources_root.exists() {
        fs::remove_dir_all(&resources_root).expect("failed to clear staged assistant skill bundle");
    }
    fs::create_dir_all(&resources_root).expect("failed to create assistant skill bundle root");

    for (relative_name, source) in &sources {
        copy_dir_recursive(source, &resources_root.join(relative_name))
            .unwrap_or_else(|error| panic!("failed to stage {}: {}", source.display(), error));
    }
}

fn copy_dir_recursive(source: &Path, target: &Path) -> std::io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}
