// Exercise the production macOS lookup code on Unix CI without pulling it into
// the desktop entrypoint on platforms where CLI setup is not exposed.
#[cfg(unix)]
#[path = "../src/cli_install_detection.rs"]
mod cli_install_detection;

#[test]
fn every_about_entry_point_refreshes_cli_state() {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let index_html = std::fs::read_to_string(format!("{}/../src/index.html", manifest))
        .expect("failed to read index.html");
    let open_about = index_html
        .split("function openAbout() {")
        .nth(1)
        .and_then(|tail| tail.split("function closeInputModal").next())
        .expect("openAbout function should be extractable");

    assert!(
        open_about.contains("refreshCliStatus(false)"),
        "the shared About entry point must refresh CLI state for native menu opens"
    );
    assert!(
        !index_html.contains("const _origOpenAbout = openAbout"),
        "About refresh must not depend on replacing the function after native listeners are wired"
    );
}
