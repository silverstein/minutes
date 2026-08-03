fn main() {
    #[cfg(target_os = "macos")]
    minutes_core::apple_speech_worker::run_xpc_service_main();

    #[cfg(not(target_os = "macos"))]
    std::process::exit(64);
}
