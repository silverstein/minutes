use std::fs;
use std::path::PathBuf;

#[allow(dead_code)]
#[path = "src/engine_process.rs"]
mod engine_process;

fn main() {
    stage_calendar_helper();
    compile_macos_graph_xpc_authority_bridge();
}

fn compile_macos_graph_xpc_authority_bridge() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "macos" {
        return;
    }

    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR should be set for build scripts"));
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"),
    );
    let source = manifest_dir.join("src/macos_graph_xpc_authority.swift");
    let apple_speech_source = manifest_dir.join("src/macos_apple_speech_bridge.swift");
    let archive = out_dir.join("libminutes_macos_graph_xpc_authority.a");
    let target = std::env::var("TARGET").expect("TARGET should be set");
    let architecture = target
        .split('-')
        .next()
        .expect("Apple target should have an architecture");
    let swift_target = format!("{architecture}-apple-macos11.0");

    println!("cargo:rerun-if-changed={}", source.display());
    println!("cargo:rerun-if-changed={}", apple_speech_source.display());
    let output = engine_process::command("swiftc")
        .args(["-parse-as-library", "-O"])
        .args(["-target", &swift_target])
        .args(["-emit-library", "-static"])
        .arg(&source)
        .arg(&apple_speech_source)
        .arg("-o")
        .arg(&archive)
        .output()
        .expect("failed to run swiftc for the graph XPC authority bridge");
    if !output.status.success() {
        panic!(
            "failed to compile graph XPC authority bridge: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=minutes_macos_graph_xpc_authority");
    println!("cargo:rustc-link-search=native=/usr/lib/swift");
    if let Some(swiftc) = engine_process::command("xcrun")
        .args(["--find", "swiftc"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|path| PathBuf::from(path.trim()))
        .and_then(|swiftc| swiftc.parent()?.parent().map(PathBuf::from))
    {
        println!(
            "cargo:rustc-link-search=native={}",
            swiftc.join("lib/swift/macosx").display()
        );
    }
    println!("cargo:rustc-link-lib=dylib=swiftCore");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=Security");
    println!("cargo:rustc-link-lib=framework=AVFAudio");
    println!("cargo:rustc-link-lib=framework=CoreMedia");
    println!("cargo:rustc-link-lib=framework=Speech");
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
