// Exercise the production macOS lookup code on Unix CI without pulling it into
// the desktop entrypoint on platforms where CLI setup is not exposed.
#[cfg(unix)]
#[path = "../src/cli_install_detection.rs"]
mod cli_install_detection;
