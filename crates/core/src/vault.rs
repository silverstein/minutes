use crate::config::Config;
use crate::error::VaultError;
use std::fs;
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────────────────────
// Vault sync: link Minutes meetings into an Obsidian/Logseq vault.
//
// Three strategies:
//   symlink — symlink inside vault → ~/meetings/. Zero-copy,
//             local-only vaults not in ~/Documents/.
//   copy    — copy .md files into vault after pipeline write.
//             Works with iCloud, Obsidian Sync, Dropbox.
//   direct  — user sets output_dir to vault path directly.
//             Requires FDA for ~/Documents/.
//   auto    — pick symlink or copy based on path analysis.
//
// Pipeline integration (non-fatal):
//   pipeline.rs calls sync_file() after markdown::write().
//   Failure logs a warning but never fails the pipeline.
// ──────────────────────────────────────────────────────────────

/// Cloud sync provider detected from path heuristics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloudProvider {
    ICloud,
    Dropbox,
    OneDrive,
    GoogleDrive,
}

impl std::fmt::Display for CloudProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloudProvider::ICloud => write!(f, "iCloud"),
            CloudProvider::Dropbox => write!(f, "Dropbox"),
            CloudProvider::OneDrive => write!(f, "OneDrive"),
            CloudProvider::GoogleDrive => write!(f, "Google Drive"),
        }
    }
}

/// Recommended sync strategy with reasoning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VaultStrategy {
    Symlink,
    Copy,
    Direct,
}

impl std::fmt::Display for VaultStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VaultStrategy::Symlink => write!(f, "symlink"),
            VaultStrategy::Copy => write!(f, "copy"),
            VaultStrategy::Direct => write!(f, "direct"),
        }
    }
}

/// Health status of the vault configuration.
#[derive(Debug)]
pub enum VaultStatus {
    NotConfigured,
    Healthy { strategy: String, path: PathBuf },
    BrokenSymlink { link_path: PathBuf, target: PathBuf },
    PermissionDenied { path: PathBuf },
    MissingVaultDir { path: PathBuf },
}

/// A vault detected by scanning the filesystem.
#[derive(Debug, Clone)]
pub struct DetectedVault {
    pub path: PathBuf,
    pub kind: String,
    pub cloud: Option<CloudProvider>,
    pub tcc_protected: bool,
}

// ── Detection ────────────────────────────────────────────────

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Check if a path is inside a macOS TCC-protected directory.
pub fn is_tcc_protected(path: &Path) -> bool {
    let home = home_dir();
    let protected = [
        home.join("Documents"),
        home.join("Desktop"),
        home.join("Downloads"),
    ];
    let canonical = path
        .canonicalize()
        .or_else(|_| {
            // Path may not exist yet — try resolving the parent
            path.parent()
                .and_then(|p| p.canonicalize().ok())
                .map(|p| p.join(path.file_name().unwrap_or_default()))
                .ok_or_else(|| std::io::Error::other("no parent"))
        })
        .unwrap_or_else(|_| path.to_path_buf());

    protected.iter().any(|dir| {
        dir.canonicalize()
            .map(|d| canonical.starts_with(&d))
            .unwrap_or(false)
    })
}

/// Detect if a path is inside a cloud-synced directory.
pub fn is_cloud_synced(path: &Path) -> Option<CloudProvider> {
    let path_str = path.to_string_lossy();

    // iCloud: ~/Library/Mobile Documents/ or com~apple~CloudDocs
    if path_str.contains("Mobile Documents") || path_str.contains("com~apple~CloudDocs") {
        return Some(CloudProvider::ICloud);
    }

    // macOS iCloud also syncs ~/Documents/ and ~/Desktop/ when enabled.
    // Check for the iCloud marker file in ~/Library/Mobile Documents/.
    #[cfg(target_os = "macos")]
    if is_tcc_protected(path) {
        let mobile_docs = home_dir().join("Library/Mobile Documents");
        if mobile_docs.exists() {
            // If the user has iCloud Drive enabled and Documents sync on,
            // ~/Documents/ content lives under Mobile Documents internally.
            // We detect this by checking if the path resolves through iCloud.
            let icloud_docs = mobile_docs.join("com~apple~CloudDocs/Documents");
            if icloud_docs.exists() {
                let home = home_dir();
                if path.starts_with(home.join("Documents"))
                    || path
                        .canonicalize()
                        .ok()
                        .map(|c| c.starts_with(home.join("Documents")))
                        .unwrap_or(false)
                {
                    return Some(CloudProvider::ICloud);
                }
            }
        }
    }

    if path_str.contains("Dropbox") {
        return Some(CloudProvider::Dropbox);
    }
    if path_str.contains("OneDrive") {
        return Some(CloudProvider::OneDrive);
    }
    if path_str.contains("Google Drive") || path_str.contains("GoogleDrive") {
        return Some(CloudProvider::GoogleDrive);
    }

    None
}

/// Recommend the best vault strategy based on path characteristics.
pub fn recommend_strategy(vault_path: &Path) -> VaultStrategy {
    // Cloud-synced vaults must use copy (symlinks break in cloud sync)
    if is_cloud_synced(vault_path).is_some() {
        return VaultStrategy::Copy;
    }

    // TCC-protected paths: prefer copy (symlink creation also needs TCC)
    if is_tcc_protected(vault_path) {
        return VaultStrategy::Copy;
    }

    // Local, non-TCC path: symlink is ideal
    VaultStrategy::Symlink
}

/// Scan common locations for markdown vaults.
/// Returns vaults found, with metadata about cloud sync and TCC status.
pub fn detect_vaults() -> Vec<DetectedVault> {
    let mut vaults = Vec::new();
    let home = home_dir();

    // Directories to scan (1 level deep for vault markers)
    let scan_dirs: Vec<PathBuf> = vec![
        home.join("Documents"),
        home.join("Obsidian"),
        home.join("notes"),
        home.join("vault"),
        home.join("vaults"),
    ];

    // Also scan direct children of home
    let home_children: Vec<PathBuf> = fs::read_dir(&home)
        .ok()
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();

    let all_dirs: Vec<PathBuf> = scan_dirs
        .into_iter()
        .chain(home_children)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for dir in &all_dirs {
        check_vault_at(dir, &mut vaults);

        // Scan 1 level of subdirectories
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    check_vault_at(&entry.path(), &mut vaults);
                }
            }
        }
    }

    // Deduplicate by canonical path
    vaults.sort_by(|a, b| a.path.cmp(&b.path));
    vaults.dedup_by(|a, b| a.path == b.path);
    vaults
}

fn check_vault_at(dir: &Path, vaults: &mut Vec<DetectedVault>) {
    let markers = [
        (".obsidian", "obsidian"),
        (".logseq", "logseq"),
        (".foam", "foam"),
    ];

    for (marker, kind) in &markers {
        if dir.join(marker).is_dir() {
            vaults.push(DetectedVault {
                path: dir.to_path_buf(),
                kind: kind.to_string(),
                cloud: is_cloud_synced(dir),
                tcc_protected: is_tcc_protected(dir),
            });
            return; // One vault per directory
        }
    }
}

// ── Symlink ──────────────────────────────────────────────────

/// Create a symlink from `link_path` (inside vault) pointing to `target` (~/meetings/).
/// The link_path's parent directory must exist and be writable.
pub fn create_symlink(link_path: &Path, target: &Path) -> Result<(), VaultError> {
    // Validate target exists
    if !target.exists() {
        return Err(VaultError::VaultPathNotFound(format!(
            "meetings directory not found: {}",
            target.display()
        )));
    }

    // Check if link_path already exists
    if link_path.exists() || link_path.symlink_metadata().is_ok() {
        // It's a symlink — check if it points to the right place
        if link_path
            .symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            let current_target = fs::read_link(link_path).map_err(VaultError::Io)?;
            if current_target == target {
                tracing::info!("symlink already exists and is correct");
                return Ok(());
            }
            // Wrong target — remove and recreate
            fs::remove_file(link_path).map_err(VaultError::Io)?;
        } else if link_path.is_dir() {
            return Err(VaultError::ExistingDirectory(format!(
                "{} already exists as a directory. Move or rename it before setting up vault sync.",
                link_path.display()
            )));
        }
    }

    // Ensure parent directory exists
    if let Some(parent) = link_path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                VaultError::PermissionDenied(parent.display().to_string())
            } else {
                VaultError::Io(e)
            }
        })?;
    }

    // Create symlink (Unix)
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, link_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                VaultError::PermissionDenied(link_path.display().to_string())
            } else {
                VaultError::SymlinkFailed(format!("{}: {}", link_path.display(), e))
            }
        })?;
    }

    // Windows: symlink_dir (requires Developer Mode or admin)
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(target, link_path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                VaultError::PermissionDenied(link_path.display().to_string())
            } else {
                VaultError::SymlinkFailed(format!(
                    "{}: {} — try: mklink /J \"{}\" \"{}\"",
                    link_path.display(),
                    e,
                    link_path.display(),
                    target.display()
                ))
            }
        })?;
    }

    tracing::info!(
        link = %link_path.display(),
        target = %target.display(),
        "vault symlink created"
    );
    Ok(())
}

// ── Copy sync ────────────────────────────────────────────────

/// Resolve the effective strategy from config (handling "auto").
fn effective_strategy(config: &Config) -> VaultStrategy {
    match config.vault.strategy.as_str() {
        "symlink" => VaultStrategy::Symlink,
        "copy" => VaultStrategy::Copy,
        "direct" => VaultStrategy::Direct,
        _ => {
            // "auto" — decide based on vault path
            if config.vault.path.as_os_str().is_empty() {
                VaultStrategy::Copy
            } else {
                recommend_strategy(&config.vault.path)
            }
        }
    }
}

/// Compute the vault meetings directory from config.
pub fn vault_meetings_dir(config: &Config) -> PathBuf {
    config.vault.path.join(&config.vault.meetings_subdir)
}

/// Sync a single file to the vault after it's been written to output_dir.
/// Returns the vault path if a copy was made, or None if no action was needed.
/// Non-fatal: callers should log errors but not fail the pipeline.
pub fn sync_file(source: &Path, config: &Config) -> Result<Option<PathBuf>, VaultError> {
    if !config.vault.enabled {
        return Ok(None);
    }

    let strategy = effective_strategy(config);

    match strategy {
        VaultStrategy::Direct | VaultStrategy::Symlink => {
            // No post-write action needed:
            // - direct: file already written to vault via output_dir
            // - symlink: vault symlink points to output_dir, files visible automatically
            Ok(None)
        }
        VaultStrategy::Copy => {
            let vault_dir = vault_meetings_dir(config);

            // Determine subdirectory: memos go in memos/ subfolder
            let filename = source.file_name().ok_or_else(|| {
                VaultError::CopyFailed("no filename".into(), std::io::Error::other("no filename"))
            })?;

            let dest_dir = if source
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n == "memos")
                .unwrap_or(false)
            {
                vault_dir.join("memos")
            } else {
                vault_dir.clone()
            };

            fs::create_dir_all(&dest_dir).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    VaultError::PermissionDenied(dest_dir.display().to_string())
                } else {
                    VaultError::CopyFailed(dest_dir.display().to_string(), e)
                }
            })?;

            let dest = dest_dir.join(filename);
            fs::copy(source, &dest)
                .map_err(|e| VaultError::CopyFailed(dest.display().to_string(), e))?;

            tracing::info!(
                source = %source.display(),
                dest = %dest.display(),
                "copied meeting to vault"
            );

            Ok(Some(dest))
        }
    }
}

/// Sync all existing meetings from output_dir to the vault (catch-up).
pub fn sync_all(config: &Config) -> Result<Vec<PathBuf>, VaultError> {
    if !config.vault.enabled {
        return Err(VaultError::NotConfigured);
    }

    let strategy = effective_strategy(config);
    if strategy != VaultStrategy::Copy {
        // Only copy strategy needs bulk sync
        return Ok(vec![]);
    }

    let mut synced = Vec::new();
    let output_dir = &config.output_dir;

    // Walk the output directory and copy all .md files
    for entry in walkdir::WalkDir::new(output_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.extension().map(|e| e == "md").unwrap_or(false) {
            match sync_file(path, config) {
                Ok(Some(dest)) => synced.push(dest),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        file = %path.display(),
                        error = %e,
                        "failed to sync file to vault"
                    );
                }
            }
        }
    }

    Ok(synced)
}

// ── Health check ─────────────────────────────────────────────

/// Check the health of the vault configuration.
pub fn check_health(config: &Config) -> VaultStatus {
    if !config.vault.enabled {
        return VaultStatus::NotConfigured;
    }

    let strategy = effective_strategy(config);
    let vault_meetings = vault_meetings_dir(config);

    match strategy {
        VaultStrategy::Symlink => {
            // Check if the symlink exists and points to the right place
            match vault_meetings.symlink_metadata() {
                Ok(meta) if meta.file_type().is_symlink() => match fs::read_link(&vault_meetings) {
                    Ok(target) => {
                        if target.exists() {
                            VaultStatus::Healthy {
                                strategy: "symlink".into(),
                                path: vault_meetings,
                            }
                        } else {
                            VaultStatus::BrokenSymlink {
                                link_path: vault_meetings,
                                target,
                            }
                        }
                    }
                    Err(_) => VaultStatus::BrokenSymlink {
                        link_path: vault_meetings.clone(),
                        target: config.output_dir.clone(),
                    },
                },
                Ok(_) => {
                    // Exists but not a symlink — could be a real directory
                    VaultStatus::Healthy {
                        strategy: "symlink (directory)".into(),
                        path: vault_meetings,
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    VaultStatus::PermissionDenied {
                        path: vault_meetings,
                    }
                }
                Err(_) => VaultStatus::MissingVaultDir {
                    path: vault_meetings,
                },
            }
        }
        VaultStrategy::Copy => {
            if vault_meetings.is_dir() {
                VaultStatus::Healthy {
                    strategy: "copy".into(),
                    path: vault_meetings,
                }
            } else if config.vault.path.exists() {
                // Vault exists but meetings subdir doesn't yet — will be created on first sync
                VaultStatus::Healthy {
                    strategy: "copy (pending first sync)".into(),
                    path: vault_meetings,
                }
            } else {
                VaultStatus::MissingVaultDir {
                    path: config.vault.path.clone(),
                }
            }
        }
        VaultStrategy::Direct => {
            if config.output_dir.is_dir() {
                VaultStatus::Healthy {
                    strategy: "direct".into(),
                    path: config.output_dir.clone(),
                }
            } else {
                VaultStatus::MissingVaultDir {
                    path: config.output_dir.clone(),
                }
            }
        }
    }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn tcc_protected_documents() {
        let home = home_dir();
        assert!(is_tcc_protected(&home.join("Documents")));
        assert!(is_tcc_protected(&home.join("Documents/life")));
        assert!(is_tcc_protected(
            &home.join("Documents/life/areas/meetings")
        ));
        assert!(is_tcc_protected(&home.join("Desktop")));
        assert!(is_tcc_protected(&home.join("Downloads")));
    }

    #[test]
    fn tcc_not_protected_other_dirs() {
        let home = home_dir();
        assert!(!is_tcc_protected(&home.join("meetings")));
        assert!(!is_tcc_protected(&home.join("notes")));
        assert!(!is_tcc_protected(&home.join(".minutes")));
        assert!(!is_tcc_protected(&PathBuf::from("/tmp/vault")));
    }

    #[test]
    fn cloud_detection_icloud() {
        let path = PathBuf::from("/Users/test/Library/Mobile Documents/com~apple~CloudDocs/vault");
        assert_eq!(is_cloud_synced(&path), Some(CloudProvider::ICloud));
    }

    #[test]
    fn cloud_detection_dropbox() {
        let path = PathBuf::from("/Users/test/Dropbox/notes");
        assert_eq!(is_cloud_synced(&path), Some(CloudProvider::Dropbox));
    }

    #[test]
    fn cloud_detection_onedrive() {
        let path = PathBuf::from("/Users/test/OneDrive/vault");
        assert_eq!(is_cloud_synced(&path), Some(CloudProvider::OneDrive));
    }

    #[test]
    fn cloud_detection_google_drive() {
        let path = PathBuf::from("/Users/test/Google Drive/vault");
        assert_eq!(is_cloud_synced(&path), Some(CloudProvider::GoogleDrive));
    }

    #[test]
    fn cloud_detection_none_for_local() {
        let path = PathBuf::from("/tmp/vault");
        assert_eq!(is_cloud_synced(&path), None);
    }

    #[test]
    fn strategy_recommends_copy_for_cloud() {
        let path = PathBuf::from("/Users/test/Dropbox/vault");
        assert_eq!(recommend_strategy(&path), VaultStrategy::Copy);
    }

    #[test]
    fn strategy_recommends_symlink_for_local() {
        let path = PathBuf::from("/tmp/vault");
        assert_eq!(recommend_strategy(&path), VaultStrategy::Symlink);
    }

    #[test]
    fn sync_file_noop_when_disabled() {
        let config = Config::default();
        assert!(!config.vault.enabled);
        let result = sync_file(Path::new("/tmp/test.md"), &config).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn sync_file_copies_to_vault() {
        let tmp = TempDir::new().unwrap();
        let meetings_dir = tmp.path().join("meetings");
        let vault_dir = tmp.path().join("vault");
        fs::create_dir_all(&meetings_dir).unwrap();
        fs::create_dir_all(&vault_dir).unwrap();

        // Write a fake meeting file
        let source = meetings_dir.join("2026-03-17-test.md");
        fs::write(&source, "# Test Meeting\n\nHello world").unwrap();

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.path = vault_dir.clone();
        config.vault.meetings_subdir = "meetings".into();
        config.vault.strategy = "copy".into();
        config.output_dir = meetings_dir;

        let result = sync_file(&source, &config).unwrap();
        assert!(result.is_some());
        let dest = result.unwrap();
        assert!(dest.exists());
        assert_eq!(
            fs::read_to_string(&dest).unwrap(),
            "# Test Meeting\n\nHello world"
        );
    }

    #[test]
    fn sync_file_copies_memo_to_memos_subdir() {
        let tmp = TempDir::new().unwrap();
        let memos_dir = tmp.path().join("meetings/memos");
        let vault_dir = tmp.path().join("vault");
        fs::create_dir_all(&memos_dir).unwrap();
        fs::create_dir_all(&vault_dir).unwrap();

        let source = memos_dir.join("2026-03-17-idea.md");
        fs::write(&source, "# Quick thought").unwrap();

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.path = vault_dir.clone();
        config.vault.meetings_subdir = "meetings".into();
        config.vault.strategy = "copy".into();
        config.output_dir = tmp.path().join("meetings");

        let result = sync_file(&source, &config).unwrap();
        let dest = result.unwrap();
        assert!(dest.to_string_lossy().contains("memos"));
        assert!(dest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn create_symlink_works() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("meetings");
        let link = tmp.path().join("vault/areas/meetings");
        fs::create_dir_all(&target).unwrap();

        create_symlink(&link, &target).unwrap();

        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(fs::read_link(&link).unwrap(), target);
    }

    #[cfg(unix)]
    #[test]
    fn create_symlink_idempotent() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("meetings");
        let link = tmp.path().join("vault/meetings");
        fs::create_dir_all(&target).unwrap();

        create_symlink(&link, &target).unwrap();
        // Second call should succeed (same target)
        create_symlink(&link, &target).unwrap();
    }

    #[test]
    fn create_symlink_rejects_existing_directory() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("meetings");
        let link = tmp.path().join("vault/meetings");
        fs::create_dir_all(&target).unwrap();
        fs::create_dir_all(&link).unwrap(); // Real directory

        let result = create_symlink(&link, &target);
        assert!(matches!(result, Err(VaultError::ExistingDirectory(_))));
    }

    #[test]
    fn check_health_not_configured() {
        let config = Config::default();
        assert!(matches!(check_health(&config), VaultStatus::NotConfigured));
    }

    #[test]
    fn check_health_copy_strategy_pending() {
        let tmp = TempDir::new().unwrap();
        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.path = tmp.path().to_path_buf();
        config.vault.strategy = "copy".into();

        match check_health(&config) {
            VaultStatus::Healthy { strategy, .. } => {
                assert!(strategy.contains("pending") || strategy == "copy");
            }
            other => panic!("expected Healthy, got {:?}", other),
        }
    }

    #[test]
    fn sync_all_copies_existing_meetings() {
        let tmp = TempDir::new().unwrap();
        let meetings_dir = tmp.path().join("meetings");
        let vault_dir = tmp.path().join("vault");
        fs::create_dir_all(&meetings_dir).unwrap();
        fs::create_dir_all(&vault_dir).unwrap();

        // Create some meeting files
        fs::write(meetings_dir.join("meeting1.md"), "# Meeting 1").unwrap();
        fs::write(meetings_dir.join("meeting2.md"), "# Meeting 2").unwrap();
        fs::write(meetings_dir.join("not-a-meeting.txt"), "skip me").unwrap();

        let mut config = Config::default();
        config.vault.enabled = true;
        config.vault.path = vault_dir.clone();
        config.vault.meetings_subdir = "meetings".into();
        config.vault.strategy = "copy".into();
        config.output_dir = meetings_dir;

        let synced = sync_all(&config).unwrap();
        assert_eq!(synced.len(), 2);

        // Verify files exist in vault
        assert!(vault_dir.join("meetings/meeting1.md").exists());
        assert!(vault_dir.join("meetings/meeting2.md").exists());
        assert!(!vault_dir.join("meetings/not-a-meeting.txt").exists());
    }
}
