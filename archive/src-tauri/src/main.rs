#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use minutes_archive_convert::{
    run_worker_process as run_convert_worker, BoundedConverter,
    WORKER_MARKER as CONVERT_WORKER_MARKER,
};
use minutes_archive_core::retrieval::{LegalSearchResponse, VaultId};
use minutes_archive_core::vault::BuildProgress;
use minutes_archive_core::vault::{
    build_authorized_document_vault, AuthorizedDocumentVault, DocumentVaultBuildReport,
    DocumentVaultLimits, ExcludedFolder,
};
use minutes_archive_core::{
    authorize_roots, reduce_approved_roots, scan_approved_roots, validate_approved_roots,
    ApprovedRoot, CensusLimits, CensusReport, CensusStatus,
};
use minutes_archive_ocr::{BoundedTranscriber, WORKER_MARKER as OCR_WORKER_MARKER};
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
/// Proves the SIGNED application can actually run its own workers.
///
/// Everything else was verified on a build that could not do this. A
/// Developer ID signature with the hardened runtime is bound to its bundle, so
/// when the workers were copied to a temp directory and run from there, the
/// copy failed validation and the kernel killed it -- every notarized build
/// was unable to index a single document. Signature, staple, Gatekeeper and
/// launch all passed, the window opened, and the first click on "Build
/// document pilot" failed. The gap was that local testing used an
/// ad-hoc-signed app, whose copy runs fine, and CI exercised the unsigned
/// build. This mode closes it: run it against the notarized artifact.
const SIGNED_WORKER_SELFTEST_MARKER: &str = "--archive-signed-worker-selftest";

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
    /// Folders inside approved locations that the build must not enter.
    ///
    /// Chosen through the same native panel as the locations themselves, so a
    /// folder path still never crosses into the webview in either direction --
    /// the interface asks for the panel and is told a count.
    exclusions: Vec<SessionExclusion>,
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
    /// Live counts for the build in flight, polled by the interface.
    ///
    /// A build over tens of thousands of documents with no visible progress is
    /// indistinguishable from a hung one. Counts only -- no filename, no path,
    /// nothing derived from a document -- so this cannot become a channel for
    /// anything but the two numbers.
    build_progress: Mutex<Arc<BuildProgress>>,
}

impl Default for ArchiveState {
    fn default() -> Self {
        Self {
            session: Mutex::new(SessionState::default()),
            next_location_id: AtomicU64::new(1),
            live_snapshots: Arc::new(Mutex::new(Vec::new())),
            build_progress: Mutex::new(Arc::new(BuildProgress::default())),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationSummary {
    id: u64,
    label: String,
}

/// The result of a folder-picker round.
///
/// `folded` counts the chosen folders that some approved location already
/// covers. They are not failures and not losses -- every document beneath them
/// is still indexed through the containing location -- but the owner picked
/// them deliberately and is owed an account of where they went.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocationChoice {
    locations: Vec<LocationSummary>,
    folded: usize,
    /// Skipped folders forgotten because the location holding them was folded.
    ///
    /// Silently dropping these reads *more* than the owner asked for, not less,
    /// so nothing is lost from the index -- but the folder they pointed at was
    /// excluded on purpose. On the archive this pilot is for that is 2,873
    /// screenshots and roughly seventeen minutes of text recognition arriving
    /// unannounced, in an index they believed excluded them.
    forgotten_skips: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BootstrapState {
    /// Which build this is, for a support conversation.
    ///
    /// The version alone does not identify a build: two candidates carried the
    /// same version and one of them could not index a single document. The
    /// short digest of the running executable is what the signed provenance
    /// record already names a candidate by, so it is the thing to ask for when
    /// someone reports a problem -- and it is computed from the file on disk
    /// rather than compiled in, so it cannot claim to be a build it is not.
    build_identity: String,
    locations: Vec<LocationSummary>,
    scan_running: bool,
    report: Option<CensusReport>,
    text_vault_report: Option<DocumentVaultBuildReport>,
}

/// Version plus a short digest of the executable that is actually running.
///
/// Reads nothing but its own binary and reaches no network -- the whole point
/// of this application is that it does not, and knowing which build you have
/// must not be the exception. Auto-update was considered and deliberately not
/// wired: docs/investigations/auto-update-evaluation.md holds the decision, and
/// a shipped updater endpoint sat in the configuration for a while contradicting
/// the "networking disabled by design" line in this app's own footer.
fn build_identity() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let digest = std::env::current_exe()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .map(|bytes| {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(&bytes);
            digest
                .iter()
                .take(6)
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>()
        })
        .unwrap_or_else(|| "unidentified".to_string());
    format!("v{version} · build {digest}")
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

/// Counts for the build in flight.
///
/// Polled rather than pushed: two integers on a timer needs no event channel,
/// and a channel that exists only to carry progress is a channel that could
/// later carry something else. Nothing derived from a document crosses here.
#[derive(serde::Serialize)]
struct UiBuildProgress {
    examined: u64,
    indexed: u64,
}

/// Show a document in Finder, named by opaque id.
///
/// The interface has never received a path and still does not: it sends back
/// the id it was given on the card, and the path is resolved here, used to ask
/// Finder to select the file, and dropped. Nothing about the location crosses
/// into the webview, so the property the census screen states -- "the interface
/// receives opaque location numbers, not folder paths" -- is unchanged.
#[tauri::command]
fn reveal_archive_document(
    document_id: String,
    state: tauri::State<'_, ArchiveState>,
) -> Result<(), String> {
    let document_id = minutes_archive_core::retrieval::DocumentId::parse(document_id)
        .map_err(|_| "That document could not be identified.".to_string())?;
    let session = state
        .session
        .lock()
        .map_err(|_| "Minutes Archive could not read its session.".to_string())?;
    let vault = session
        .text_vault
        .as_ref()
        .ok_or_else(|| "There is no open index.".to_string())?;
    // Refuses if the file moved, changed identity, or is no longer a regular
    // file inside its approved root -- the same check a quotation gets.
    let path = vault
        .source_path_for_reveal(&document_id)
        .ok_or_else(|| "That source is no longer where it was indexed.".to_string())?;
    std::process::Command::new("/usr/bin/open")
        .arg("-R")
        .arg(&path)
        .status()
        .map_err(|_| "Finder could not be asked to show the document.".to_string())?;
    Ok(())
}

#[tauri::command]
fn archive_index_progress(state: tauri::State<'_, ArchiveState>) -> UiBuildProgress {
    let progress = state
        .build_progress
        .lock()
        .map(|slot| Arc::clone(&slot))
        .unwrap_or_default();
    UiBuildProgress {
        examined: progress.examined(),
        indexed: progress.indexed(),
    }
}

#[tauri::command]
fn archive_bootstrap(state: State<'_, ArchiveState>) -> Result<BootstrapState, String> {
    let session = state.session.lock().map_err(|_| lock_error())?;
    Ok(BootstrapState {
        build_identity: build_identity(),
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
) -> Result<LocationChoice, String> {
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
        return Ok(LocationChoice {
            folded: 0,
            forgotten_skips: 0,
            locations: location_summaries(&session.locations),
        });
    };

    let selected = selected
        .into_iter()
        .map(|path| {
            path.into_path()
                .map_err(|_| "The selected location is not a local folder.".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let new_roots = authorize_roots(&selected).map_err(safe_census_error)?;

    let mut session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;

    // A folder chosen twice, or chosen inside one that is already approved, is
    // folded into the location that covers it. Refusing the batch instead
    // discarded every other folder the owner had just picked and left them
    // with an empty list and the word "overlap".
    //
    // Existing locations come first so that re-choosing an approved folder
    // keeps the location already on screen, id and all, rather than replacing
    // it with an identical one.
    let existing = session.locations.len();
    let mut combined = session
        .locations
        .iter()
        .map(|location| location.root.clone())
        .collect::<Vec<_>>();
    combined.extend(new_roots.iter().cloned());
    let kept = reduce_approved_roots(&combined);
    let folded = combined.len().saturating_sub(kept.len());

    let surviving = kept
        .iter()
        .map(|&index| combined[index].clone())
        .collect::<Vec<_>>();
    validate_approved_roots(&surviving).map_err(safe_census_error)?;

    let mut locations = Vec::with_capacity(kept.len());
    for index in kept {
        match session.locations.get(index) {
            Some(location) if index < existing => locations.push(ApprovedLocation {
                id: location.id,
                root: location.root.clone(),
            }),
            _ => locations.push(ApprovedLocation {
                id: state.next_location_id.fetch_add(1, Ordering::Relaxed),
                root: combined[index].clone(),
            }),
        }
    }
    session.locations = locations;
    // A location folded into one that covers it takes its skipped folders with
    // it. They were named relative to a root that is no longer in the list, and
    // silently reattaching them to the containing root would skip folders the
    // owner never pointed at.
    let surviving_ids = session
        .locations
        .iter()
        .map(|location| location.id)
        .collect::<Vec<_>>();
    let skips_before = session.exclusions.len();
    session
        .exclusions
        .retain(|exclusion| surviving_ids.contains(&exclusion.location_id));
    let forgotten_skips = skips_before.saturating_sub(session.exclusions.len());
    session.last_report = None;
    session.text_vault = None;
    Ok(LocationChoice {
        folded,
        forgotten_skips,
        locations: location_summaries(&session.locations),
    })
}

/// A skipped folder, held against the location's stable id rather than its
/// position in the list.
///
/// `ExcludedFolder` names its root by index, which is correct for one build and
/// wrong to store: removing a location, or folding one into a folder that
/// covers it, renumbers everything after it, and an exclusion would quietly
/// start skipping a folder in some other location. The index is derived at
/// build time from the list as it stands then, and an exclusion whose location
/// is gone simply does not appear.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionExclusion {
    location_id: u64,
    relative_path: std::path::PathBuf,
}

/// Resolves stored exclusions against the locations as they stand right now.
fn exclusions_for_build(session: &SessionState) -> Vec<ExcludedFolder> {
    session
        .exclusions
        .iter()
        .filter_map(|exclusion| {
            let root_index = session
                .locations
                .iter()
                .position(|location| location.id == exclusion.location_id)?;
            Some(ExcludedFolder {
                root_index,
                relative_path: exclusion.relative_path.clone(),
            })
        })
        .collect()
}

/// What a round of the "skip folders" panel did.
///
/// Counts only. `outside` is the number of chosen folders that are not inside
/// any approved location -- they are not silently dropped, because an operator
/// who believes a folder is being skipped and is wrong ends up with an index
/// they cannot account for.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExclusionChoice {
    skipped: usize,
    outside: usize,
    refused_whole_location: usize,
    /// Every folder currently being skipped, counted here rather than tallied
    /// by the interface. A running total kept on the other side of the boundary
    /// drifts the moment a location is removed, and a confidently wrong count
    /// of what is being skipped is worse than none.
    total: usize,
}

/// Choose folders inside approved locations that the build must not read.
///
/// A thirty-year archive is not uniformly relevant: the folder that prompted
/// this held 2,873 screenshots and screen recordings, which cost OCR time and
/// index nothing an attorney would search for. The alternative was to approve
/// the parent whole or not at all.
#[tauri::command]
async fn choose_archive_exclusions(
    app: tauri::AppHandle,
    state: State<'_, ArchiveState>,
) -> Result<ExclusionChoice, String> {
    {
        let session = state.session.lock().map_err(|_| lock_error())?;
        ensure_scan_idle(&session)?;
        if session.locations.is_empty() {
            return Err("Approve at least one location before choosing what to skip.".to_string());
        }
    }
    let selected = app
        .dialog()
        .file()
        .set_title("Choose folders to skip")
        .blocking_pick_folders();
    native_panel_state::forget();
    let Some(selected) = selected else {
        let session = state.session.lock().map_err(|_| lock_error())?;
        return Ok(ExclusionChoice {
            skipped: 0,
            outside: 0,
            refused_whole_location: 0,
            total: session.exclusions.len(),
        });
    };

    let mut session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;
    let mut choice = ExclusionChoice {
        skipped: 0,
        outside: 0,
        refused_whole_location: 0,
        total: 0,
    };

    for path in selected {
        let Ok(path) = path.into_path() else {
            choice.outside += 1;
            continue;
        };
        let Ok(canonical) = fs::canonicalize(&path) else {
            choice.outside += 1;
            continue;
        };
        let Some((root_index, location_id, relative)) = session
            .locations
            .iter()
            .enumerate()
            .find_map(|(index, location)| {
                canonical
                    .strip_prefix(location.root.canonical_path())
                    .ok()
                    .map(|relative| (index, location.id, relative.to_path_buf()))
            })
        else {
            choice.outside += 1;
            continue;
        };
        // An empty relative path is the approved location itself. Excluding it
        // would silently index nothing from that location while it still sat
        // in the list looking approved; removing the location says the same
        // thing honestly.
        if relative.as_os_str().is_empty() {
            choice.refused_whole_location += 1;
            continue;
        }
        // The prefix match above is on strings. Confirm it by identity before
        // trusting it, the same way roots are compared: the folder reached by
        // rejoining the relative path to the root must be the folder that was
        // actually picked.
        let rejoined = session.locations[root_index]
            .root
            .canonical_path()
            .join(&relative);
        let same_folder = fs::metadata(&rejoined)
            .ok()
            .zip(fs::metadata(&canonical).ok())
            .is_some_and(|(left, right)| {
                use std::os::unix::fs::MetadataExt;
                left.is_dir() && left.dev() == right.dev() && left.ino() == right.ino()
            });
        if !same_folder {
            choice.outside += 1;
            continue;
        }

        let exclusion = SessionExclusion {
            location_id,
            relative_path: relative,
        };
        if !session.exclusions.contains(&exclusion) {
            session.exclusions.push(exclusion);
        }
        choice.skipped += 1;
    }

    if choice.skipped > 0 {
        session.last_report = None;
        session.text_vault = None;
    }
    choice.total = session.exclusions.len();
    Ok(choice)
}

/// Forget every skipped folder, so the next build reads the locations whole.
#[tauri::command]
fn clear_archive_exclusions(state: State<'_, ArchiveState>) -> Result<usize, String> {
    let mut session = state.session.lock().map_err(|_| lock_error())?;
    ensure_scan_idle(&session)?;
    let cleared = session.exclusions.len();
    session.exclusions.clear();
    if cleared > 0 {
        session.last_report = None;
        session.text_vault = None;
    }
    Ok(cleared)
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
    // Skipped folders belong to the location that contained them. Leaving them
    // behind means a later location could inherit them by id reuse, and it
    // makes the count of what is being skipped a lie.
    session
        .exclusions
        .retain(|exclusion| exclusion.location_id != location_id);
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
    let (roots, exclusions) = {
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
        // Resolved here, against the location list as it stands, so a stored
        // exclusion can never point at a location that has since moved.
        let exclusions = exclusions_for_build(&session);
        let roots = session
            .locations
            .iter()
            .map(|location| location.root.clone())
            .collect::<Vec<_>>();
        (roots, exclusions)
    };

    let worker_executable = std::env::current_exe()
        .map_err(|_| "Minutes Archive could not bind its document converter.".to_string())?;
    // Kept live: `purge_session` drains it, and a worker that needs scratch
    // space again should register here. Nothing populates it today.
    let _snapshot_registry = Arc::clone(&state.live_snapshots);
    // Reset before the build, so a second build does not continue the first
    // one's numbers.
    let progress = Arc::new(BuildProgress::default());
    if let Ok(mut slot) = state.build_progress.lock() {
        *slot = Arc::clone(&progress);
    }
    let build_result = tauri::async_runtime::spawn_blocking(move || {
        let vault_id = VaultId::parse("local-private-vault")
            .map_err(|_| "Minutes Archive could not establish the private vault.".to_string())?;
        let converter =
            BoundedConverter::bind(&worker_executable).map_err(|error| error.to_string())?;
        // Binding the on-device model must not be able to deny the operator an
        // index. Exact evidence is the product; semantic suggestions are an
        // optional aid the interface already labels review-not-verified. A Mac
        // without Apple's linguistic asset previously got NO search at all.
        let semantic_engine = BoundedSemanticEngine::bind(&worker_executable).ok();
        // Same reasoning for the recogniser: a Mac where Vision cannot start
        // must still index everything that is not a scan. Scans then stay
        // counted as needing OCR, which is what they were before this existed.
        let transcriber = BoundedTranscriber::bind(&worker_executable).ok();
        // Neither worker copies itself any more -- both execute in place from
        // the bundle -- so there is no snapshot directory left to reclaim. The
        // registry stays because it is what `purge_session` drains, and a
        // future worker that does need scratch space should register it here.
        build_authorized_document_vault(
            vault_id,
            &roots,
            DocumentVaultLimits {
                excluded_paths: exclusions,
                ..DocumentVaultLimits::default()
            },
            &cancelled,
            &converter,
            transcriber.as_ref(),
            Some(progress.as_ref()),
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
    /// The opaque id, so a card can ask for itself to be shown in Finder.
    ///
    /// An id, never a path: it means nothing outside this session and resolves
    /// only against sources the vault already holds. The interface still
    /// receives no filename and no location.
    document_id: String,
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
            document_id: card.document_id.as_str().to_string(),
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
    document_id: String,
    document_title: String,
    provision_heading: Option<String>,
    source_anchor: String,
    exact_excerpt: String,
    sentence_count: u32,
    source_converter: String,
    why_suggested: String,
    index_fresh: bool,
}

/// A passage read out of an image, kept apart from every card that quotes a
/// source. The field names differ from `UiEvidenceCard` on purpose: the
/// interface cannot render one as the other by reaching for `exact_excerpt`.
#[derive(serde::Serialize)]
struct UiTranscribedCard {
    document_id: String,
    document_title: String,
    page_anchor: String,
    transcribed_text: String,
    lowest_line_confidence: f32,
    transcriber: String,
    why_transcribed: String,
    index_fresh: bool,
}

impl From<&minutes_archive_core::retrieval::TranscribedCard> for UiTranscribedCard {
    fn from(card: &minutes_archive_core::retrieval::TranscribedCard) -> Self {
        Self {
            document_id: card.document_id.as_str().to_string(),
            document_title: card.document_title.clone(),
            page_anchor: card.page_anchor.clone(),
            transcribed_text: card.transcribed_text.clone(),
            lowest_line_confidence: card.lowest_line_confidence,
            transcriber: card.transcriber.clone(),
            why_transcribed: card.why_transcribed.clone(),
            index_fresh: card.index_fresh,
        }
    }
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
    transcriptions: Vec<UiTranscribedCard>,
    lexical_candidates_considered: usize,
    semantic_candidates_considered: usize,
    semantic_query_applied: bool,
    stale_evidence_withdrawn: u64,
    inferred_boundary_evidence_withdrawn: u64,
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
                    document_id: card.document_id.as_str().to_string(),
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
            transcriptions: response
                .transcriptions
                .iter()
                .map(UiTranscribedCard::from)
                .collect(),
            lexical_candidates_considered: response.lexical_candidates_considered,
            semantic_candidates_considered: response.semantic_candidates_considered,
            semantic_query_applied: response.semantic_query_applied,
            stale_evidence_withdrawn: response.stale_evidence_withdrawn,
            inferred_boundary_evidence_withdrawn: response.inferred_boundary_evidence_withdrawn,
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

/// Bind the converter to this executable and convert one synthetic document.
///
/// Deliberately end to end through the real `BoundedConverter`: the failure
/// this exists to catch was in binding, not in parsing, and it only appears
/// when the running executable carries a bundle-bound signature.
/// The recognizer worker, reached through the same binary as the others.
///
/// Its absence is what made `BoundedTranscriber::bind` fail: the marker fell
/// through to the GUI branch, so binding launched a second copy of the
/// application instead of a worker, the self-test never passed, and every scan
/// was silently skipped as an unsupported format. Nothing reported it, because
/// the engine is optional by design and `.ok()` swallowed the failure.
fn run_ocr_worker(operation: &str) -> i32 {
    if minutes_archive_ocr::install_worker_security_boundary().is_err() {
        return 70;
    }
    if operation == "sandbox-self-test" {
        return minutes_archive_ocr::sandbox_self_test();
    }
    if operation != "recognize" {
        return 64;
    }
    use std::io::{Read, Write};
    let mut image = Vec::new();
    if std::io::stdin().lock().read_to_end(&mut image).is_err() {
        return 65;
    }
    let outcome = std::panic::catch_unwind(|| minutes_archive_ocr::recognize_page(&image));
    let Ok(Ok(page)) = outcome else {
        return 66;
    };
    let Ok(encoded) = serde_json::to_vec(&page) else {
        return 67;
    };
    if std::io::stdout().lock().write_all(&encoded).is_err() {
        return 68;
    }
    0
}

fn run_signed_worker_selftest() -> i32 {
    let executable = match std::env::current_exe() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("signed-worker-selftest: current_exe failed: {error}");
            return 70;
        }
    };
    let converter = match minutes_archive_convert::BoundedConverter::bind(&executable) {
        Ok(converter) => converter,
        Err(error) => {
            eprintln!("signed-worker-selftest: converter bind failed: {error}");
            return 71;
        }
    };
    // A minimal synthetic document, inline so the check needs no fixture on
    // the runner and never touches a real file.
    const SOURCE: &[u8] =
        b"7. CONFIDENTIALITY\nConfidential Information includes affiliate data.\n";
    match converter.convert(minutes_archive_convert::SourceFormat::Docx, SOURCE) {
        Ok(_) => {}
        Err(minutes_archive_convert::WorkerError::SourceRefused) => {
            // Expected: those bytes are not a real DOCX container. What
            // matters is that the worker ran and answered, which it cannot do
            // if the signature check killed it.
        }
        Err(error) => {
            eprintln!("signed-worker-selftest: worker did not run: {error}");
            return 72;
        }
    }
    // The recognizer is bound against THIS binary, not the standalone worker.
    // Exercising the standalone one is what let a missing marker ship: that
    // executable of course understood its own marker, while the application it
    // actually runs inside did not, and binding it launched a second copy of
    // the app instead of a worker.
    if let Err(error) = minutes_archive_ocr::BoundedTranscriber::bind(&executable) {
        eprintln!("signed-worker-selftest: recognizer bind failed: {error}");
        return 73;
    }
    match minutes_archive_semantic::BoundedSemanticEngine::bind(&executable) {
        Ok(_) => println!("signed_worker_selftest=passed converter=bound ocr=bound semantic=bound"),
        // Absent on a runner without Apple's linguistic asset, which is not a
        // signing failure and must not fail the check.
        Err(error) => println!(
            "signed_worker_selftest=passed converter=bound ocr=bound semantic=unavailable ({error})"
        ),
    }
    0
}

fn main() {
    // One descriptor per indexed document, held for the session so the
    // live-source fence can re-read through it. macOS gives a GUI application a
    // soft limit of 256, which stopped a build at 237 of 16,621 and reported
    // the rest as unreadable. The implementation lives in archive-core so a
    // harness can lower the ceiling and prove this still completes; nothing in
    // `main` is reachable from a test.
    minutes_archive_core::vault::raise_open_file_ceiling();
    let mut arguments = std::env::args();
    let _program = arguments.next();
    let marker = arguments.next();
    if matches!(
        marker.as_deref(),
        Some(CONVERT_WORKER_MARKER | SEMANTIC_WORKER_MARKER | OCR_WORKER_MARKER)
    ) {
        let operation = arguments.next().unwrap_or_default();
        if arguments.next().is_some() {
            std::process::exit(64);
        }
        let status = match marker.as_deref() {
            Some(CONVERT_WORKER_MARKER) => run_convert_worker(&operation),
            Some(SEMANTIC_WORKER_MARKER) => run_semantic_worker(&operation),
            Some(OCR_WORKER_MARKER) => run_ocr_worker(&operation),
            _ => unreachable!("worker marker was already validated"),
        };
        std::process::exit(status);
    }
    if marker.as_deref() == Some(SIGNED_WORKER_SELFTEST_MARKER) {
        if arguments.next().is_some() {
            std::process::exit(64);
        }
        std::process::exit(run_signed_worker_selftest());
    }
    // An unrecognised flag must never reach the GUI branch.
    //
    // This is how a missing worker marker became a second copy of the
    // application on the owner's screen: `BoundedTranscriber::bind` launched
    // this binary with the OCR marker, nothing matched, and execution fell
    // through to `tauri::Builder`. The bind then failed, every scan was skipped
    // as unsupported, and the visible symptom was a window nobody asked for
    // stealing focus mid-build. Refusing anything flag-shaped that is not
    // understood turns that whole class of mistake into an immediate exit.
    if marker
        .as_deref()
        .is_some_and(|value| value.starts_with("--") && value != NATIVE_LIFECYCLE_SELFTEST_MARKER)
    {
        eprintln!("Minutes Archive: unrecognised option");
        std::process::exit(64);
    }
    let native_lifecycle_selftest = marker.as_deref() == Some(NATIVE_LIFECYCLE_SELFTEST_MARKER);
    if native_lifecycle_selftest && arguments.next().is_some() {
        std::process::exit(64);
    }

    tauri::Builder::default()
        .manage(ArchiveState::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            // Erase any panel state a PREVIOUS run left behind, before this
            // one can show a window. The erasure after each panel and at
            // graceful exit cannot run under SIGKILL or Force Quit, so a
            // forced termination can leave NSOSPLastRootDirectory on disk. It
            // cannot be prevented -- AppKit writes it, and no hook runs after
            // a kill -- but it can be bounded: with this, residue survives
            // only until the app is next opened, rather than indefinitely.
            native_panel_state::forget();
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
            choose_archive_exclusions,
            clear_archive_exclusions,
            remove_archive_location,
            run_archive_census,
            cancel_archive_census,
            export_archive_census,
            build_archive_text_vault,
            archive_index_progress,
            reveal_archive_document,
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
    use minutes_archive_core::approve_roots;

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
        //
        // `document_id` was on this list and has been deliberately removed from
        // it, so that a card can ask for its own source to be shown in Finder.
        // What that reveals is nothing: the id is a synthetic counter,
        // `document-{n:016x}`, generated during the build and meaningless
        // outside the session. It is not derived from the filename, the path or
        // the contents, and `valid_opaque_id` restricts it to lowercase
        // alphanumerics and hyphens, so a path could not be smuggled through it
        // even by accident. The path itself is resolved behind the boundary and
        // never returned. The assertion below pins that.
        for field in ["sha256", "byte_len", "vault_id", "lexical_rank"] {
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

        // The id crosses, and it is opaque. Nothing about where the document
        // lives goes with it.
        assert!(
            projected.contains("document_id"),
            "cards cannot ask for their source without an id"
        );
        assert!(
            !projected.contains("probe-doc.txt") && !projected.contains('/'),
            "the projection carried a filename or a path: {projected}"
        );
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

    /// A skipped folder must follow its location, not its position.
    ///
    /// Exclusions are stored against the location's stable id and resolved to a
    /// root index only at build time. Storing the index instead would mean that
    /// removing the first location silently moved every exclusion onto whatever
    /// location took its place -- folders the owner never pointed at would stop
    /// being read, with only a count to show for it, which is the one failure
    /// this build cannot have.
    #[test]
    fn a_skipped_folder_follows_its_location_when_the_list_changes() {
        let temp = TempDir::new().expect("temp");
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        fs::create_dir(&first).expect("first");
        fs::create_dir(&second).expect("second");
        let mut roots = approve_roots(&[first, second]).expect("approve synthetic roots");
        let mut session = SessionState {
            locations: vec![
                ApprovedLocation {
                    id: 7,
                    root: roots.remove(0),
                },
                ApprovedLocation {
                    id: 8,
                    root: roots.remove(0),
                },
            ],
            exclusions: vec![SessionExclusion {
                location_id: 8,
                relative_path: std::path::PathBuf::from("attachments"),
            }],
            ..SessionState::default()
        };

        let resolved = exclusions_for_build(&session);
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].root_index, 1, "second location is at index 1");

        // Drop the first location. The exclusion still belongs to location 8,
        // which has moved to index 0.
        session.locations.retain(|location| location.id != 7);
        let resolved = exclusions_for_build(&session);
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].root_index, 0,
            "the exclusion followed its location instead of staying at index 1"
        );

        // Drop the location it belongs to. The exclusion resolves to nothing
        // rather than attaching itself to a location that never had it.
        session.locations.clear();
        assert!(exclusions_for_build(&session).is_empty());
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
