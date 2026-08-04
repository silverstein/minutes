#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use minutes_archive_convert::{
    run_worker_process as run_convert_worker, BoundedConverter,
    WORKER_MARKER as CONVERT_WORKER_MARKER,
};
use minutes_archive_core::retrieval::{LegalSearchResponse, VaultId};
use minutes_archive_core::vault::{
    build_authorized_document_vault, AuthorizedDocumentVault, DocumentVaultBuildReport,
    DocumentVaultLimits,
};
use minutes_archive_core::{
    approve_roots, scan_approved_roots, validate_approved_roots, ApprovedRoot, CensusLimits,
    CensusReport, CensusStatus,
};
use minutes_archive_semantic::{
    run_worker_process as run_semantic_worker, BoundedSemanticEngine,
    WORKER_MARKER as SEMANTIC_WORKER_MARKER,
};
use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};
use tauri_plugin_dialog::DialogExt;

const NATIVE_LIFECYCLE_SELFTEST_MARKER: &str = "--archive-native-lifecycle-selftest";

#[derive(Debug)]
struct ApprovedLocation {
    id: u64,
    root: ApprovedRoot,
}

#[derive(Debug, Default)]
struct ScanControl {
    running: bool,
    cancelled: Option<Arc<AtomicBool>>,
}

#[derive(Debug, Default)]
struct SessionState {
    locations: Vec<ApprovedLocation>,
    last_report: Option<CensusReport>,
    text_vault: Option<AuthorizedDocumentVault>,
    scan: ScanControl,
}

#[derive(Debug)]
struct ArchiveState {
    session: Mutex<SessionState>,
    next_location_id: AtomicU64,
    /// Snapshot directories of workers that are currently alive.
    ///
    /// During a vault build the converter and engine live inside a blocking
    /// task, not in `session`, so the close handler owns nothing to drop and
    /// `exit(0)` leaves both 40 MB snapshots behind. Registering the paths at
    /// creation lets the purge reclaim them whichever way the app exits.
    live_snapshots: Arc<Mutex<Vec<std::path::PathBuf>>>,
}

impl Default for ArchiveState {
    fn default() -> Self {
        Self {
            session: Mutex::new(SessionState::default()),
            next_location_id: AtomicU64::new(1),
            live_snapshots: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationSummary {
    id: u64,
    label: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapState {
    locations: Vec<LocationSummary>,
    scan_running: bool,
    report: Option<CensusReport>,
    text_vault_report: Option<DocumentVaultBuildReport>,
}

fn lock_error() -> String {
    "Minutes Archive could not access its private session state.".to_string()
}

fn safe_census_error(error: impl std::fmt::Display) -> String {
    error.to_string()
}

fn location_summaries(locations: &[ApprovedLocation]) -> Vec<LocationSummary> {
    locations
        .iter()
        .enumerate()
        .map(|(index, location)| LocationSummary {
            id: location.id,
            label: format!("Approved location {}", index + 1),
        })
        .collect()
}

fn ensure_scan_idle(session: &SessionState) -> Result<(), String> {
    if session.scan.running {
        return Err("Wait for the current census to finish or cancel it first.".to_string());
    }
    Ok(())
}

#[tauri::command]
fn archive_bootstrap(state: State<'_, ArchiveState>) -> Result<BootstrapState, String> {
    let session = state.session.lock().map_err(|_| lock_error())?;
    Ok(BootstrapState {
        locations: location_summaries(&session.locations),
        scan_running: session.scan.running,
        report: session.last_report.clone(),
        text_vault_report: session
            .text_vault
            .as_ref()
            .map(|vault| vault.build_report().clone()),
    })
}

#[tauri::command]
async fn choose_archive_locations(
    app: tauri::AppHandle,
    state: State<'_, ArchiveState>,
) -> Result<Vec<LocationSummary>, String> {
    {
        let session = state.session.lock().map_err(|_| lock_error())?;
        ensure_scan_idle(&session)?;
    }
    let selected = app
        .dialog()
        .file()
        .set_title("Choose archive locations")
        .blocking_pick_folders();
    // AppKit records the chosen directory the moment the panel closes, so
    // erase it here rather than only at exit -- a crash between the two would
    // otherwise leave the path on disk.
    native_panel_state::forget();
    let Some(selected) = selected else {
        let session = state.session.lock().map_err(|_| lock_error())?;
        return Ok(location_summaries(&session.locations));
    };

    let selected = selected
        .into_iter()
        .map(|path| {
            path.into_path()
                .map_err(|_| "The selected location is not a local folder.".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let new_roots = approve_roots(&selected).map_err(safe_census_error)?;

    let mut session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;
    let mut combined = session
        .locations
        .iter()
        .map(|location| location.root.clone())
        .collect::<Vec<_>>();
    combined.extend(new_roots.iter().cloned());
    validate_approved_roots(&combined).map_err(safe_census_error)?;

    for root in new_roots {
        session.locations.push(ApprovedLocation {
            id: state.next_location_id.fetch_add(1, Ordering::Relaxed),
            root,
        });
    }
    session.last_report = None;
    session.text_vault = None;
    Ok(location_summaries(&session.locations))
}

#[tauri::command]
fn remove_archive_location(
    location_id: u64,
    state: State<'_, ArchiveState>,
) -> Result<Vec<LocationSummary>, String> {
    let mut session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;
    let before = session.locations.len();
    session
        .locations
        .retain(|location| location.id != location_id);
    if session.locations.len() == before {
        return Err("That approved location is no longer available.".to_string());
    }
    session.last_report = None;
    session.text_vault = None;
    Ok(location_summaries(&session.locations))
}

#[tauri::command]
async fn run_archive_census(state: State<'_, ArchiveState>) -> Result<CensusReport, String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let roots = {
        let mut session = state.session.lock().map_err(|_| lock_error())?;
        if session.locations.is_empty() {
            return Err("Choose at least one archive location first.".to_string());
        }
        if session.scan.running {
            return Err("A census is already running.".to_string());
        }
        session.scan.running = true;
        session.scan.cancelled = Some(Arc::clone(&cancelled));
        session.last_report = None;
        session.text_vault = None;
        session
            .locations
            .iter()
            .map(|location| location.root.clone())
            .collect::<Vec<_>>()
    };

    let scan_result = tauri::async_runtime::spawn_blocking(move || {
        scan_approved_roots(&roots, CensusLimits::default(), &cancelled)
    })
    .await
    .map_err(|_| "The private census worker stopped unexpectedly.".to_string());

    {
        let mut session = state.session.lock().map_err(|_| lock_error())?;
        session.scan.running = false;
        session.scan.cancelled = None;
    }

    let report = scan_result?.map_err(safe_census_error)?;
    if report.status != CensusStatus::Cancelled {
        state.session.lock().map_err(|_| lock_error())?.last_report = Some(report.clone());
    }
    Ok(report)
}

#[tauri::command]
fn cancel_archive_census(state: State<'_, ArchiveState>) -> Result<bool, String> {
    let session = state.session.lock().map_err(|_| lock_error())?;
    let Some(cancelled) = &session.scan.cancelled else {
        return Ok(false);
    };
    cancelled.store(true, Ordering::Release);
    Ok(true)
}

/// Whether an existing destination is an alias for an inode with other names.
fn is_multiply_linked(metadata: &fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        metadata.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        false
    }
}

fn refuse_link_target(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("The report destination cannot be a symbolic link.".to_string())
        }
        Ok(metadata) if !metadata.is_file() => {
            Err("The report destination must be a regular file.".to_string())
        }
        // A hard link IS a regular file, so both checks above pass it, and
        // `O_NOFOLLOW` refuses symlinks only. Truncating it destroys whatever
        // inode the name is an alias for -- so choosing a report name that
        // happens to be linked to a client document replaces that document
        // with census JSON. Same link class as the ingestion boundary, on the
        // write side, and destructive rather than disclosive.
        Ok(metadata) if is_multiply_linked(&metadata) => Err(
            "The report destination is a hard link to another file. Choose a different name."
                .to_string(),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err("The report destination is unavailable.".to_string()),
    }
}

fn write_private_report(path: &Path, report: &CensusReport) -> Result<(), String> {
    refuse_link_target(path)?;
    let json = serde_json::to_vec_pretty(report)
        .map_err(|_| "Minutes Archive could not prepare the aggregate report.".to_string())?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options
        .open(path)
        .map_err(|_| "Minutes Archive could not save the aggregate report.".to_string())?;
    file.write_all(&json)
        .and_then(|_| file.write_all(b"\n"))
        .and_then(|_| file.sync_all())
        .map_err(|_| "Minutes Archive could not finish saving the aggregate report.".to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o600))
            .map_err(|_| {
                "Minutes Archive could not protect the aggregate report permissions.".to_string()
            })?;
    }
    Ok(())
}

#[tauri::command]
async fn export_archive_census(
    app: tauri::AppHandle,
    state: State<'_, ArchiveState>,
) -> Result<bool, String> {
    let report = {
        let session = state.session.lock().map_err(|_| lock_error())?;
        ensure_scan_idle(&session)?;
        session
            .last_report
            .clone()
            .ok_or_else(|| "Run a complete census before exporting a report.".to_string())?
    };
    let selected = app
        .dialog()
        .file()
        .set_title("Save aggregate archive census")
        .set_file_name("archive-census.json")
        .blocking_save_file();
    // Same mechanism as the open panel: the save panel records its directory
    // in the app's own preference domain.
    native_panel_state::forget();
    let Some(selected) = selected else {
        return Ok(false);
    };
    let path = selected
        .into_path()
        .map_err(|_| "The report destination is not a local file.".to_string())?;
    write_private_report(&path, &report)?;
    Ok(true)
}

#[tauri::command]
async fn build_archive_text_vault(
    state: State<'_, ArchiveState>,
) -> Result<DocumentVaultBuildReport, String> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let roots = {
        let mut session = state.session.lock().map_err(|_| lock_error())?;
        if session.locations.is_empty() {
            return Err("Choose at least one archive location first.".to_string());
        }
        if session.last_report.is_none() {
            return Err(
                "Run and review the metadata-only census before opening any documents.".to_string(),
            );
        }
        if session.scan.running {
            return Err("Another private archive operation is already running.".to_string());
        }
        session.scan.running = true;
        session.scan.cancelled = Some(Arc::clone(&cancelled));
        session.text_vault = None;
        session
            .locations
            .iter()
            .map(|location| location.root.clone())
            .collect::<Vec<_>>()
    };

    let worker_executable = std::env::current_exe()
        .map_err(|_| "Minutes Archive could not bind its document converter.".to_string())?;
    let snapshot_registry = Arc::clone(&state.live_snapshots);
    let build_result = tauri::async_runtime::spawn_blocking(move || {
        let vault_id = VaultId::parse("local-private-vault")
            .map_err(|_| "Minutes Archive could not establish the private vault.".to_string())?;
        let converter =
            BoundedConverter::bind(&worker_executable).map_err(|error| error.to_string())?;
        let semantic_engine =
            BoundedSemanticEngine::bind(&worker_executable).map_err(|error| error.to_string())?;
        // Register both snapshots before the long build starts. Until the
        // vault exists these objects live only in this blocking task, so a
        // close during the build would otherwise leave both directories.
        if let Ok(mut registry) = snapshot_registry.lock() {
            registry.push(converter.snapshot_directory().to_path_buf());
            registry.push(semantic_engine.snapshot_directory().to_path_buf());
        }
        build_authorized_document_vault(
            vault_id,
            &roots,
            DocumentVaultLimits::default(),
            &cancelled,
            &converter,
            semantic_engine,
        )
        .map_err(|error| error.to_string())
    })
    .await
    .map_err(|_| "The private document-index worker stopped unexpectedly.".to_string());

    {
        let mut session = state.session.lock().map_err(|_| lock_error())?;
        session.scan.running = false;
        session.scan.cancelled = None;
    }

    let vault = build_result??;
    let report = vault.build_report().clone();
    state.session.lock().map_err(|_| lock_error())?.text_vault = Some(vault);
    Ok(report)
}

/// What the interface is allowed to see of a search result.
///
/// The retrieval types carry more than the interface renders: a SHA-256 of
/// every matched document's full bytes, its length, the vault and document
/// ids, lexical rank, matched concepts, and semantic similarity. An
/// independent reviewer found all of it crossing the IPC boundary and none of
/// it rendered -- a content hash of every privileged match sitting in the
/// WebView's JS heap for no purpose, which is a confirmation-of-possession
/// oracle against a known corpus. With `script-src 'self'` and no remote
/// content the exploit path is narrow, but the field is gratuitous, and the
/// app's whole claim is that the interface receives the minimum it needs.
///
/// Projecting here rather than trimming the retrieval type keeps the evidence
/// record complete where it is used for verification, and keeps the boundary
/// honest by construction: a field added to `EvidenceCard` later does not
/// silently reach the webview.
#[derive(serde::Serialize)]
struct UiEvidenceCard {
    document_title: String,
    provision_heading: Option<String>,
    source_anchor: String,
    exact_excerpt: String,
    sentence_count: u32,
    source_converter: String,
    why_matched: String,
    index_fresh: bool,
}

impl From<&minutes_archive_core::retrieval::EvidenceCard> for UiEvidenceCard {
    fn from(card: &minutes_archive_core::retrieval::EvidenceCard) -> Self {
        Self {
            document_title: card.document_title.clone(),
            provision_heading: card.provision_heading.clone(),
            source_anchor: card.source_anchor.clone(),
            exact_excerpt: card.exact_excerpt.clone(),
            sentence_count: card.sentence_count,
            source_converter: card.source_converter.clone(),
            why_matched: card.why_matched.clone(),
            index_fresh: card.index_fresh,
        }
    }
}

#[derive(serde::Serialize)]
struct UiSemanticCard {
    document_title: String,
    provision_heading: Option<String>,
    source_anchor: String,
    exact_excerpt: String,
    sentence_count: u32,
    source_converter: String,
    why_suggested: String,
    index_fresh: bool,
}

#[derive(serde::Serialize)]
struct UiDocumentCard {
    document_title: String,
    criterion_evidence: Vec<UiEvidenceCard>,
    index_fresh: bool,
}

#[derive(serde::Serialize)]
struct UiSearchResponse {
    query: minutes_archive_core::retrieval::LegalQuery,
    evidence: Vec<UiEvidenceCard>,
    documents: Vec<UiDocumentCard>,
    semantic_suggestions: Vec<UiSemanticCard>,
    lexical_candidates_considered: usize,
    semantic_candidates_considered: usize,
    semantic_query_applied: bool,
    stale_evidence_withdrawn: u64,
}

impl From<LegalSearchResponse> for UiSearchResponse {
    fn from(response: LegalSearchResponse) -> Self {
        Self {
            query: response.query,
            evidence: response.evidence.iter().map(UiEvidenceCard::from).collect(),
            documents: response
                .documents
                .iter()
                .map(|document| UiDocumentCard {
                    document_title: document.document_title.clone(),
                    criterion_evidence: document
                        .criterion_evidence
                        .iter()
                        .map(UiEvidenceCard::from)
                        .collect(),
                    index_fresh: document.index_fresh,
                })
                .collect(),
            semantic_suggestions: response
                .semantic_suggestions
                .iter()
                .map(|card| UiSemanticCard {
                    document_title: card.document_title.clone(),
                    provision_heading: card.provision_heading.clone(),
                    source_anchor: card.source_anchor.clone(),
                    exact_excerpt: card.exact_excerpt.clone(),
                    sentence_count: card.sentence_count,
                    source_converter: card.source_converter.clone(),
                    why_suggested: card.why_suggested.clone(),
                    index_fresh: card.index_fresh,
                })
                .collect(),
            lexical_candidates_considered: response.lexical_candidates_considered,
            semantic_candidates_considered: response.semantic_candidates_considered,
            semantic_query_applied: response.semantic_query_applied,
            stale_evidence_withdrawn: response.stale_evidence_withdrawn,
        }
    }
}

#[tauri::command]
fn search_archive_text_vault(
    query: String,
    state: State<'_, ArchiveState>,
) -> Result<UiSearchResponse, String> {
    let session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;
    let vault = session.text_vault.as_ref().ok_or_else(|| {
        "Build the private text index before searching. No partial index was retained.".to_string()
    })?;
    vault
        .interpret_and_search(query)
        .map(UiSearchResponse::from)
        .map_err(|error| error.to_string())
}

fn main() {
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let marker = arguments.next();
    if matches!(
        marker.as_deref(),
        Some(CONVERT_WORKER_MARKER | SEMANTIC_WORKER_MARKER)
    ) {
        let operation = arguments.next().unwrap_or_default();
        if arguments.next().is_some() {
            std::process::exit(64);
        }
        let status = match marker.as_deref() {
            Some(CONVERT_WORKER_MARKER) => run_convert_worker(&operation),
            Some(SEMANTIC_WORKER_MARKER) => run_semantic_worker(&operation),
            _ => unreachable!("worker marker was already validated"),
        };
        std::process::exit(status);
    }
    let native_lifecycle_selftest = marker.as_deref() == Some(NATIVE_LIFECYCLE_SELFTEST_MARKER);
    if native_lifecycle_selftest && arguments.next().is_some() {
        std::process::exit(64);
    }

    tauri::Builder::default()
        .manage(ArchiveState::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            if native_lifecycle_selftest {
                let window = app.get_webview_window("main").ok_or_else(|| {
                    std::io::Error::other("Archive native lifecycle self-test found no main window")
                })?;
                if !window.is_visible()? {
                    return Err(std::io::Error::other(
                        "Archive native lifecycle self-test found a hidden main window",
                    )
                    .into());
                }
                println!("archive_native_window=visible");
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                    println!("archive_native_close=requested");
                    if let Err(error) = window.close() {
                        eprintln!("archive_native_close_error={error}");
                    }
                });
            }
            Ok(())
        })
        .on_window_event(move |window, event| {
            if window.label() == "main"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                // Archive has no tray mode. Exiting with the only window prevents
                // an invisible process from retaining privileged source text,
                // FTS rows, or semantic vectors after the user closes the app.
                if native_lifecycle_selftest {
                    println!("archive_native_close_event=received");
                }
                // Release the session explicitly first. `exit(0)` terminates
                // the process without unwinding, so no destructor ever runs:
                // the worker snapshot directories are owned by `TempDir`
                // fields whose cleanup is `Drop`, and they were surviving the
                // process as two 40 MB copies of the executable in $TMPDIR.
                // Any future zeroization written as a destructor would have
                // been skipped the same way. Dropping the session here runs
                // that cleanup while the process is still alive.
                let app_handle = window.app_handle().clone();
                purge_session(&app_handle);
                app_handle.exit(0);
            }
        })
        .invoke_handler(tauri::generate_handler![
            archive_bootstrap,
            choose_archive_locations,
            remove_archive_location,
            run_archive_census,
            cancel_archive_census,
            export_archive_census,
            build_archive_text_vault,
            search_archive_text_vault,
        ])
        .build(tauri::generate_context!())
        .expect("Minutes Archive failed to start")
        .run(|app_handle, event| {
            // The window-close handler alone is not enough. Cmd-Q maps to
            // `[NSApp terminate:]`, which never calls `windowShouldClose:`,
            // so `CloseRequested` never fires -- and Cmd-Q is how most Mac
            // users quit an app. `Exit` covers that path, the Quit menu item,
            // and any other route out of the run loop.
            if matches!(event, tauri::RunEvent::Exit) {
                purge_session(app_handle);
            }
        });
}

/// Erases the location record AppKit keeps after a native panel is used.
///
/// `NSOpenPanel` writes the last directory into the app's own preference
/// domain as an `NSOSPLastRootDirectory` bookmark. An independent reviewer
/// decoded that blob and recovered the full path of the approved archive --
/// volume name, volume UUID, and every directory component -- and it survived
/// application exit. `~/Library/Preferences` carries no TCC protection, so a
/// post-install script, a sync agent, a backup, or a forensic image reads the
/// exact on-disk location of a client archive with no prompt. Folder names in
/// legal practice are client names.
///
/// The app tells the operator on screen that it receives "opaque location
/// numbers, not folder paths", and it exits on window close so that nothing
/// privileged outlives the session. This closes the one artifact that did.
///
/// No application code writes these keys; AppKit does, so they are removed
/// after the panel closes and again when the session is purged.
#[cfg(target_os = "macos")]
mod native_panel_state {
    use std::ffi::c_void;

    type CFStringRef = *const c_void;
    type CFPropertyListRef = *const c_void;

    extern "C" {
        static kCFPreferencesCurrentApplication: CFStringRef;
        fn CFStringCreateWithBytes(
            allocator: *const c_void,
            bytes: *const u8,
            num_bytes: isize,
            encoding: u32,
            is_external_representation: u8,
        ) -> CFStringRef;
        fn CFPreferencesSetAppValue(
            key: CFStringRef,
            value: CFPropertyListRef,
            application_id: CFStringRef,
        );
        fn CFPreferencesAppSynchronize(application_id: CFStringRef) -> u8;
        fn CFRelease(cf: *const c_void);
    }

    const UTF8: u32 = 0x0800_0100;

    /// Every key AppKit is known to write from an open or save panel. Removing
    /// a key the domain does not have is a no-op, so listing the save-panel
    /// and recent-places keys costs nothing and covers the export path too.
    const PANEL_KEYS: &[&str] = &[
        "NSOSPLastRootDirectory",
        "NSNavLastRootDirectory",
        "NSNavLastCurrentDirectory",
        "NSNavRecentPlaces",
        "NSNavPanelExpandedSizeForOpenMode",
        "NSNavPanelExpandedSizeForSaveMode",
        "NSWindow Frame GoToSheet",
        "NSWindow Frame NSNavPanelAutosaveName",
    ];

    pub fn forget() {
        // SAFETY: every pointer is either a CFString this function created and
        // releases, or the framework-owned current-application constant. A
        // null value is CFPreferences' documented "remove this key".
        unsafe {
            for key in PANEL_KEYS {
                let cf_key = CFStringCreateWithBytes(
                    std::ptr::null(),
                    key.as_ptr(),
                    key.len() as isize,
                    UTF8,
                    0,
                );
                if cf_key.is_null() {
                    continue;
                }
                CFPreferencesSetAppValue(
                    cf_key,
                    std::ptr::null(),
                    kCFPreferencesCurrentApplication,
                );
                CFRelease(cf_key);
            }
            CFPreferencesAppSynchronize(kCFPreferencesCurrentApplication);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod native_panel_state {
    pub fn forget() {}
}

/// Releases everything the session owns while the process is still alive.
///
/// `exit(0)` terminates without unwinding, so no destructor runs at exit: the
/// worker snapshot directories are owned by `TempDir` fields whose cleanup is
/// `Drop`, and anything written as a destructor later would be skipped the
/// same way.
fn purge_session(app_handle: &tauri::AppHandle) {
    let Some(state) = app_handle.try_state::<ArchiveState>() else {
        return;
    };
    // Recover from poisoning rather than skipping the purge. A panic anywhere
    // under this lock would otherwise leave the session permanently
    // un-purgeable, and a poisoned app is precisely the app a user closes.
    let mut session = state
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    *session = SessionState::default();
    drop(session);

    let mut snapshots = state
        .live_snapshots
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    for snapshot in snapshots.drain(..) {
        let _ = fs::remove_dir_all(&snapshot);
    }
    drop(snapshots);

    native_panel_state::forget();
}

#[cfg(test)]
mod tests {
    /// Exporting the census must never overwrite a hard-linked document.
    ///
    /// An independent reviewer found that `refuse_link_target` rejects
    /// symlinks but accepts any regular file, and `O_NOFOLLOW` does not refuse
    /// a hard link either -- so `truncate(true)` destroys whatever inode the
    /// destination name is an alias for. Same link class as the ingestion
    /// escape, but on the write side, and destructive rather than disclosive:
    /// a client document is replaced by census JSON.
    #[test]
    #[cfg(unix)]
    fn exporting_over_a_hard_link_never_destroys_the_linked_document() {
        let temp = tempfile::tempdir().expect("temp");
        let precious = temp.path().join("client-matter.txt");
        let original = b"PRIVILEGED CLIENT DOCUMENT";
        std::fs::write(&precious, original).expect("write precious");

        // The operator picks a report name that is an alias for that document.
        let destination = temp.path().join("archive-census.json");
        std::fs::hard_link(&precious, &destination).expect("hard link");

        let refusal = refuse_link_target(&destination);
        assert!(
            refusal.is_err(),
            "a multiply linked destination was accepted for truncation"
        );
        assert_eq!(
            std::fs::read(&precious).expect("read back"),
            original,
            "the linked client document was modified"
        );

        // An ordinary new destination is still allowed.
        assert!(refuse_link_target(&temp.path().join("fresh-report.json")).is_ok());
    }

    /// The interface must not receive a content hash of every match.
    ///
    /// An independent reviewer found `source_revision.sha256` and `byte_len`
    /// crossing IPC unrendered. Serializing the projection is the only way to
    /// prove the boundary: asserting on struct fields would pass even if the
    /// command went back to returning the retrieval type.
    #[test]
    fn the_ui_projection_carries_no_hash_identifier_or_rank() {
        use minutes_archive_core::retrieval::{
            interpret_legal_query, normalize_text_document, CurrentRevisionSet, DocumentId,
            LegalIndex, VaultId,
        };

        let vault = VaultId::parse("dto-probe").expect("vault");
        let document = normalize_text_document(
            DocumentId::parse("probe-doc").expect("id"),
            "Probe Document",
            b"7. CONFIDENTIALITY\nRecipient shall protect Confidential Information and its affiliates.",
        )
        .expect("normalize");
        let mut index = LegalIndex::new(vault.clone()).expect("index");
        index.replace_document(&document).expect("replace");
        let revisions = CurrentRevisionSet::from_documents([&document]);
        let query = interpret_legal_query("Find confidentiality provisions covering affiliates.")
            .expect("query");
        let response = index.search(&vault, query, &revisions).expect("search");
        assert!(
            !response.evidence.is_empty(),
            "fixture returned no evidence"
        );

        let full = serde_json::to_string(&response).expect("serialize full");
        let projected =
            serde_json::to_string(&UiSearchResponse::from(response)).expect("serialize projection");

        // The full record really does carry these; the projection must not.
        for field in [
            "sha256",
            "byte_len",
            "vault_id",
            "document_id",
            "lexical_rank",
        ] {
            assert!(
                full.contains(field),
                "fixture no longer exercises {field}; this test would pass vacuously"
            );
            assert!(
                !projected.contains(field),
                "{field} still reaches the interface"
            );
        }
        // ...while what the interface renders survives.
        assert!(projected.contains("exact_excerpt"));
        assert!(projected.contains("why_matched"));
        assert!(projected.contains("source_anchor"));
    }

    use super::*;
    use std::sync::atomic::AtomicBool;
    use tempfile::TempDir;

    fn synthetic_report(temp: &TempDir) -> CensusReport {
        let root = temp.path().join("approved");
        fs::create_dir(&root).expect("approved root");
        fs::write(
            root.join("Privileged Client Name.pdf"),
            b"SYNTHETIC_CONTENT_CANARY",
        )
        .expect("synthetic document");
        minutes_archive_core::scan_roots(&[root], CensusLimits::default(), &AtomicBool::new(false))
            .expect("synthetic report")
    }

    #[test]
    fn location_summaries_never_expose_paths() {
        let temp = TempDir::new().expect("temp");
        let client_alpha = temp.path().join("client-alpha");
        let client_beta = temp.path().join("client-beta");
        fs::create_dir(&client_alpha).expect("alpha");
        fs::create_dir(&client_beta).expect("beta");
        let mut roots =
            approve_roots(&[client_alpha, client_beta]).expect("approve synthetic roots");
        let locations = vec![
            ApprovedLocation {
                id: 7,
                root: roots.remove(0),
            },
            ApprovedLocation {
                id: 8,
                root: roots.remove(0),
            },
        ];
        let serialized =
            serde_json::to_string(&location_summaries(&locations)).expect("serialize summaries");
        assert_eq!(
            serialized,
            r#"[{"id":7,"label":"Approved location 1"},{"id":8,"label":"Approved location 2"}]"#
        );
        assert!(!serialized.contains("client-alpha"));
        assert!(!serialized.contains("client-beta"));
    }

    #[test]
    fn exported_report_is_private_and_contains_no_source_canaries() {
        let temp = TempDir::new().expect("temp");
        let report = synthetic_report(&temp);
        let output = temp.path().join("archive-census.json");
        write_private_report(&output, &report).expect("export");
        let exported = fs::read_to_string(&output).expect("read aggregate export");
        assert!(!exported.contains("Privileged Client Name"));
        assert!(!exported.contains("SYNTHETIC_CONTENT_CANARY"));
        assert!(!exported.contains(&temp.path().to_string_lossy().to_string()));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&output)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn exported_report_refuses_symbolic_link_destination() {
        let temp = TempDir::new().expect("temp");
        let report = synthetic_report(&temp);
        let real_output = temp.path().join("real.json");
        fs::write(&real_output, b"do not overwrite").expect("real output");
        let link_output = temp.path().join("linked.json");
        std::os::unix::fs::symlink(&real_output, &link_output).expect("link");

        assert_eq!(
            write_private_report(&link_output, &report),
            Err("The report destination cannot be a symbolic link.".to_string())
        );
        assert_eq!(
            fs::read(&real_output).expect("preserved output"),
            b"do not overwrite"
        );
    }
}
