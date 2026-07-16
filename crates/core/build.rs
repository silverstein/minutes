use std::fs;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/engine_process.rs"]
mod engine_process;

fn main() {
    stage_calendar_helper();
}

fn stage_calendar_helper() {
    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR should be set for build scripts"));
    let output_path = out_dir.join("calendar-events");
    fs::create_dir_all(&out_dir).expect("failed to create OUT_DIR");

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        fs::write(&output_path, []).expect("failed to write empty calendar helper placeholder");
        return;
    }

    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"),
    );
    let repo_root = manifest_dir.join("../..");
    let source = repo_root.join("scripts/calendar-events.swift");
    let info_plist = repo_root.join("scripts/calendar-helper-Info.plist");

    if !source.exists() || !info_plist.exists() {
        println!(
            "cargo:warning=minutes-core calendar helper sources missing; embedded helper disabled"
        );
        fs::write(&output_path, []).expect("failed to write empty calendar helper placeholder");
        return;
    }

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", info_plist.display());

    let output = engine_process::command("swiftc")
        .arg("-O")
        .args(["-Xlinker", "-sectcreate"])
        .args(["-Xlinker", "__TEXT"])
        .args(["-Xlinker", "__info_plist"])
        .arg("-Xlinker")
        .arg(&info_plist)
        .arg(&source)
        .arg("-o")
        .arg(&output_path)
        .output()
        .expect("failed to run swiftc for embedded calendar-events helper");

    if !output.status.success() {
        panic!(
            "failed to compile embedded calendar-events helper: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
