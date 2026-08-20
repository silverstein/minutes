#[cfg(not(windows))]
use cap_std::fs::DirBuilder as CapDirBuilder;
use cap_std::fs::{Dir, OpenOptions as CapOpenOptions, OpenOptionsExt as CapOpenOptionsExt};
use sha2::{Digest, Sha256};
use std::cell::Cell;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

/// Maximum exact text capability retained by native policy surfaces.
///
/// This matches the SDK/MCP per-file authorization ceiling. Callers may apply
/// tighter prompt/history budgets, but no policy read may allocate from an
/// attacker-controlled file length beyond this process-safe bound.
pub const MAX_BOUND_TEXT_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_RESTRICTED_OVERRIDE_AUDIT_RECORD_BYTES: usize = 16 * 1024;
const MAX_RESTRICTED_OVERRIDE_AUDIT_FILE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundFileIdentity {
    first: u64,
    second: u64,
}

/// Stable identity and content proof for a capability-bound file. The fields
/// stay private so callers cannot accidentally turn this proof into
/// user-facing context or event data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryFileProof {
    identity: BoundFileIdentity,
    bytes: u64,
    digest: [u8; 32],
}

/// Stable identity proof for one retained recovery directory capability.
///
/// Like `RecoveryFileProof`, the fields remain private so the proof cannot be
/// serialized into a public event or error by accident. Recovery ledgers may
/// persist its private parts inside owner-only control records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryDirectoryProof {
    identity: BoundFileIdentity,
}

impl RecoveryDirectoryProof {
    /// Re-attest a reopened owner-private directory through the caller's exact
    /// handle. This lets a short-lived Windows rename-capable handle prove
    /// continuity with an atomically protected allocation without retaining a
    /// second no-delete-sharing policy handle beside it.
    pub(crate) fn attest_exact_owner_private_directory_file(
        &self,
        directory: &File,
    ) -> std::io::Result<()> {
        if bound_file_identity(directory)? != self.identity {
            return Err(invalid_recovery_path(
                "reopened private directory does not match its allocation proof",
            ));
        }
        let metadata = directory.metadata()?;
        if !metadata.is_dir() || metadata_is_link_or_reparse(&metadata) {
            return Err(invalid_recovery_path(
                "reopened private directory is not a safe directory",
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o777 != 0o700 {
                return Err(invalid_recovery_path(
                    "reopened private directory is not mode 0700",
                ));
            }
        }
        #[cfg(windows)]
        crate::overlays::attest_owner_only_directory_handle(directory)?;
        Ok(())
    }
}

impl RecoveryFileProof {
    /// Return the exact byte length bound into this opaque proof without
    /// exposing its identity or digest.
    pub fn byte_len(&self) -> u64 {
        self.bytes
    }
}

#[derive(Debug, Clone)]
pub struct BoundTextSnapshot {
    pub canonical_path: PathBuf,
    pub content: String,
    pub identity: BoundFileIdentity,
}

pub fn content_sha256_hex(content: &[u8]) -> String {
    Sha256::digest(content)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Read one already-bound file twice under an exact byte and wall-clock
/// budget, retaining only the first pass. Matching lengths and SHA-256 digests
/// prove both passes observed the same bytes without allocating a second copy.
pub(crate) fn read_bound_file_twice_bounded(
    file: &mut File,
    max_bytes: u64,
    deadline: std::time::Instant,
) -> std::io::Result<Vec<u8>> {
    fn scan(
        file: &mut File,
        max_bytes: u64,
        deadline: std::time::Instant,
        mut retained: Option<&mut Vec<u8>>,
    ) -> std::io::Result<(u64, [u8; 32])> {
        file.seek(SeekFrom::Start(0))?;
        let mut digest = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            if std::time::Instant::now() >= deadline {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "capability-bound text read exceeded its deadline",
                ));
            }
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| std::io::Error::other("bounded text length overflow"))?;
            if total > max_bytes {
                return Err(std::io::Error::other(
                    "capability-bound text exceeded its byte budget",
                ));
            }
            digest.update(&buffer[..read]);
            if let Some(bytes) = retained.as_deref_mut() {
                bytes.extend_from_slice(&buffer[..read]);
            }
        }
        Ok((total, digest.finalize().into()))
    }

    let expected_len = file.metadata()?.len();
    if expected_len > max_bytes {
        return Err(std::io::Error::other(
            "capability-bound text exceeded its byte budget",
        ));
    }
    let capacity = usize::try_from(expected_len)
        .map_err(|_| std::io::Error::other("bounded text length is not addressable"))?;
    let mut bytes = Vec::with_capacity(capacity);
    let first = scan(file, max_bytes, deadline, Some(&mut bytes))?;
    let second = scan(file, max_bytes, deadline, None)?;
    if first != second || first.0 != expected_len {
        return Err(std::io::Error::other(
            "capability-bound text changed while it was read",
        ));
    }
    file.seek(SeekFrom::Start(0))?;
    Ok(bytes)
}

#[cfg(any(windows, test))]
fn normalize_windows_canonical_path_wire(raw: &str) -> Option<String> {
    const EXTENDED_PREFIX: &str = "\\\\?\\";
    const EXTENDED_UNC_PREFIX: &str = "\\\\?\\UNC\\";

    if raw
        .get(..EXTENDED_UNC_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(EXTENDED_UNC_PREFIX))
    {
        return Some(format!("\\\\{}", &raw[EXTENDED_UNC_PREFIX.len()..]));
    }

    let rest = raw.strip_prefix(EXTENDED_PREFIX)?;
    let bytes = rest.as_bytes();
    (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\')
        .then(|| rest.to_string())
}

/// Serialize a canonical path across the Rust/Node JSON boundary.
///
/// Windows `std::fs::canonicalize` returns extended drive/UNC spellings while
/// Node's realpath APIs return the equivalent DOS/UNC spelling. Normalize only
/// those two namespaces; every other path distinction remains exact.
pub fn canonical_path_wire(path: &Path) -> String {
    let raw = path.to_string_lossy();
    #[cfg(windows)]
    if let Some(normalized) = normalize_windows_canonical_path_wire(&raw) {
        return normalized;
    }
    raw.into_owned()
}

fn metadata_identity_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    let stable = left.is_file() == right.is_file()
        && left.is_dir() == right.is_dir()
        && left.len() == right.len()
        && left.modified().ok() == right.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        stable && left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        stable
            && left.creation_time() == right.creation_time()
            && left.last_write_time() == right.last_write_time()
            && left.file_attributes() == right.file_attributes()
    }
    #[cfg(not(any(unix, windows)))]
    {
        stable
    }
}

fn directory_metadata_identity_matches(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    let stable = left.is_dir() && right.is_dir();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        stable && left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        // Directory byte length and last-write time are mutable namespace
        // bookkeeping. Creation time and attributes bind the lexical checks;
        // opened directory pairs additionally compare stable handle identity.
        stable
            && left.creation_time() == right.creation_time()
            && left.file_attributes() == right.file_attributes()
    }
    #[cfg(not(any(unix, windows)))]
    {
        stable
    }
}

#[cfg(windows)]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

pub(crate) fn cap_lexical_regular_file_is_safe(metadata: &cap_std::fs::Metadata) -> bool {
    !metadata.file_type().is_symlink() && metadata.is_file()
}

#[cfg(unix)]
pub(crate) fn opened_regular_file_is_safe(file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata()
        .is_ok_and(|metadata| metadata.is_file() && metadata.nlink() == 1)
}

#[cfg(windows)]
pub(crate) fn opened_regular_file_is_safe(file: &File) -> bool {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    (unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) }) != 0
        && info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) == 0
        && info.nNumberOfLinks == 1
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn opened_regular_file_is_safe(file: &File) -> bool {
    file.metadata().is_ok_and(|metadata| metadata.is_file())
}

#[cfg(unix)]
pub(crate) fn open_file_identity_matches(left: &File, right: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.metadata()
        .ok()
        .zip(right.metadata().ok())
        .is_some_and(|(left, right)| left.dev() == right.dev() && left.ino() == right.ino())
}

#[cfg(unix)]
fn bound_file_identity(file: &File) -> std::io::Result<BoundFileIdentity> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(BoundFileIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    })
}

#[cfg(unix)]
fn bound_file_filesystem_identity(file: &File) -> std::io::Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(file.metadata()?.dev())
}

#[cfg(windows)]
pub(crate) fn open_file_identity_matches(left: &File, right: &File) -> bool {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let identify = |file: &File| -> Option<BY_HANDLE_FILE_INFORMATION> {
        let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        (unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } != 0)
            .then_some(info)
    };
    identify(left)
        .zip(identify(right))
        .is_some_and(|(left, right)| {
            left.dwVolumeSerialNumber == right.dwVolumeSerialNumber
                && left.nFileIndexHigh == right.nFileIndexHigh
                && left.nFileIndexLow == right.nFileIndexLow
        })
}

#[cfg(windows)]
fn bound_file_identity(file: &File) -> std::io::Result<BoundFileIdentity> {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(BoundFileIdentity {
        first: info.dwVolumeSerialNumber as u64,
        second: ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
    })
}

#[cfg(windows)]
fn bound_file_filesystem_identity(file: &File) -> std::io::Result<u64> {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };

    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.dwVolumeSerialNumber as u64)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn open_file_identity_matches(left: &File, right: &File) -> bool {
    left.metadata()
        .ok()
        .zip(right.metadata().ok())
        .is_some_and(|(left, right)| metadata_identity_matches(&left, &right))
}

#[cfg(not(any(unix, windows)))]
fn bound_file_identity(file: &File) -> std::io::Result<BoundFileIdentity> {
    let metadata = file.metadata()?;
    let modified = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    Ok(BoundFileIdentity {
        first: metadata.len(),
        second: modified.as_nanos() as u64,
    })
}

#[cfg(not(any(unix, windows)))]
fn bound_file_filesystem_identity(_file: &File) -> std::io::Result<u64> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "filesystem identity is unavailable on this platform",
    ))
}

/// Create or attest a directory whose contents are visible only to the
/// current OS account. On Windows this applies and verifies a protected DACL;
/// on Unix it rejects symlinks/wrong types and enforces mode 0700.
pub fn ensure_owner_only_directory(path: &Path) -> std::io::Result<()> {
    crate::overlays::secure_private_parent(path)
}

/// Create or attest an owner-only regular file. Call this before writing
/// private bytes when the file was just created beneath an owner-only parent.
pub fn ensure_owner_only_file(path: &Path) -> std::io::Result<()> {
    crate::overlays::secure_private_file(path)
}

/// Create a new owner-only file without ever exposing its bytes under ambient
/// permissions. The destination must not already exist.
pub fn write_owner_only_new_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_owner_only_new_file_with_hook(path, bytes, || {})
}

/// Durably append one privacy-safe restricted-content authorization record.
///
/// The complete audit namespace, serialization lease, and visible JSONL leaf
/// stay capability-bound for the transaction. Authorization succeeds only
/// after the exact appended bytes are readable from the still-visible leaf.
pub fn append_restricted_override_audit(record: &[u8]) -> std::io::Result<()> {
    append_restricted_override_audit_at_with_hook(
        &crate::overlays::correction_state_dir().join("audit"),
        record,
        || {},
    )
}

#[doc(hidden)]
pub fn append_restricted_override_audit_at_with_hook(
    audit_dir: &Path,
    record: &[u8],
    after_data_sync: impl FnOnce(),
) -> std::io::Result<()> {
    if record.is_empty()
        || record.len() > MAX_RESTRICTED_OVERRIDE_AUDIT_RECORD_BYTES
        || !record.ends_with(b"\n")
        || record[..record.len() - 1].contains(&b'\n')
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "restricted override audit record is not one bounded JSONL line",
        ));
    }

    let directory = BoundRecoveryDirectory::prepare_owner_private(audit_dir)?;
    let lease =
        directory.bind_or_create_private_lease_file(OsStr::new("sensitivity-overrides.lock"))?;
    crate::overlays::secure_private_file_handle(&lease.file.file)?;
    lease.attest_visible_identity()?;
    lease.lock_exclusive()?;
    lease.attest_visible_identity()?;

    // Use the non-delete-sharing lease handle contract for the data leaf too.
    // On Windows this makes rename/delete impossible while authorization is
    // pending; on POSIX the final visible-identity proof detects a displaced
    // inode and fails the request closed.
    let audit =
        directory.bind_or_create_private_lease_file(OsStr::new("sensitivity-overrides.jsonl"))?;
    crate::overlays::secure_private_file_handle(&audit.file.file)?;
    audit.attest_visible_identity()?;
    let mut file = audit.file.file.try_clone()?;
    let before = file.metadata()?.len();
    let record_len = u64::try_from(record.len())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let after = before.checked_add(record_len).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "restricted override audit length overflow",
        )
    })?;
    if after > MAX_RESTRICTED_OVERRIDE_AUDIT_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "restricted override audit reached its bounded size",
        ));
    }
    if before > 0 {
        file.seek(SeekFrom::Start(before - 1))?;
        let mut tail = [0u8; 1];
        file.read_exact(&mut tail)?;
        if tail[0] != b'\n' {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "restricted override audit has an incomplete tail",
            ));
        }
    }

    file.seek(SeekFrom::Start(before))?;
    file.write_all(record)?;
    file.flush()?;
    file.sync_all()?;
    after_data_sync();

    // Re-read the exact new tail and require EOF there. This rejects partial,
    // interleaved, appended-after-us, or displaced-leaf outcomes before the
    // caller is allowed to release restricted content.
    file.seek(SeekFrom::Start(before))?;
    let mut observed = vec![0u8; record.len()];
    file.read_exact(&mut observed)?;
    let mut extra = [0u8; 1];
    if observed != record || file.read(&mut extra)? != 0 || file.metadata()?.len() != after {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "restricted override audit append could not be re-attested",
        ));
    }
    audit.attest_visible_identity()?;
    directory.sync()?;
    audit.attest_visible_identity()?;
    lease.attest_visible_identity()?;
    Ok(())
}

fn write_owner_only_new_file_with_hook<F>(
    path: &Path,
    bytes: &[u8],
    after_parent_bound: F,
) -> std::io::Result<()>
where
    F: FnOnce(),
{
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("private file has no parent directory"))?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("private file has no filename"))?;
    // Keep the exact parent capability from authorization through creation and
    // durability. Re-resolving `path` here would let a concurrent parent
    // rename redirect a checkpoint after the owner-private check.
    let directory = BoundRecoveryDirectory::prepare_owner_private(parent)?;
    after_parent_bound();
    let mut file = directory.create_new_exact_file(name)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    directory.sync()?;
    Ok(())
}

/// Atomically move one filesystem entry without replacing an existing
/// destination. Unlike verify-then-unlink, the moved entry is exactly the one
/// that occupied `source` at the linearization point, so a concurrent leaf
/// replacement is preserved rather than deleted.
#[cfg(target_os = "linux")]
pub fn move_entry_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
pub fn move_entry_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), libc::RENAME_EXCL) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub fn move_entry_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};
    let source: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
    let destination: Vec<u16> = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    if unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    } != 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub fn move_entry_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    let _ = (source, destination);
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this platform",
    ))
}

fn validate_entry_name(name: &std::ffi::OsStr) -> std::io::Result<()> {
    let mut components = Path::new(name).components();
    let is_one_normal_component = components
        .next()
        .is_some_and(|component| matches!(component, Component::Normal(found) if found == name))
        && components.next().is_none();
    if is_one_normal_component {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rename entry is not one safe relative filename",
        ))
    }
}

/// Atomically move one retained directory entry to another retained directory
/// without replacing the destination.
///
/// Linux and macOS only expose descriptor-relative *name*-based rename
/// primitives. `source_file` is therefore retained for the caller to attest
/// the moved destination immediately after the syscall. Windows can perform
/// the rename on that exact opened handle, eliminating the source-leaf race at
/// the primitive itself.
#[cfg(target_os = "linux")]
pub(crate) fn move_entry_at_no_replace(
    source_dir: &Dir,
    source_name: &std::ffi::OsStr,
    _source_file: &File,
    destination_dir: &Dir,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    validate_entry_name(source_name)?;
    validate_entry_name(destination_name)?;
    let source_name = CString::new(source_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination_name = CString::new(destination_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe {
        libc::renameat2(
            source_dir.as_raw_fd(),
            source_name.as_ptr(),
            destination_dir.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn move_entry_at_no_replace(
    source_dir: &Dir,
    source_name: &std::ffi::OsStr,
    _source_file: &File,
    destination_dir: &Dir,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    validate_entry_name(source_name)?;
    validate_entry_name(destination_name)?;
    let source_name = CString::new(source_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let destination_name = CString::new(destination_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe {
        libc::renameatx_np(
            source_dir.as_raw_fd(),
            source_name.as_ptr(),
            destination_dir.as_raw_fd(),
            destination_name.as_ptr(),
            libc::RENAME_EXCL,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(windows)]
pub(crate) fn move_entry_at_no_replace(
    _source_dir: &Dir,
    source_name: &std::ffi::OsStr,
    source_file: &File,
    destination_dir: &Dir,
    destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    validate_entry_name(source_name)?;
    validate_entry_name(destination_name)?;
    let destination_name = destination_name.encode_wide().collect::<Vec<_>>();
    if destination_name.is_empty()
        || destination_name
            .iter()
            .any(|unit| matches!(*unit, 0 | 47 | 58 | 92))
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "rename entry is not one safe Windows filename",
        ));
    }

    // Microsoft documents a buffer of at least the fixed structure plus the
    // variable filename bytes. The extra nominal one-element FileName field
    // and trailing alignment are harmless and keep this valid for short names.
    let filename_size = destination_name
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let buffer_size = size_of::<FILE_RENAME_INFORMATION>()
        .checked_add(filename_size)
        .filter(|size| *size <= u32::MAX as usize)
        .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let mut buffer = vec![0usize; buffer_size.div_ceil(size_of::<usize>())];
    let information = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    // SAFETY: `buffer` is pointer-aligned and sized for the fixed header plus
    // every UTF-16 code unit. FILE_RENAME_INFO explicitly uses a trailing
    // variable-length FileName array.
    unsafe {
        (*information).Anonymous.ReplaceIfExists = 0;
        (*information).RootDirectory = destination_dir.as_raw_handle() as _;
        (*information).FileNameLength = filename_size as u32;
        std::ptr::copy_nonoverlapping(
            destination_name.as_ptr(),
            std::ptr::addr_of_mut!((*information).FileName).cast::<u16>(),
            destination_name.len(),
        );
    }
    let mut io_status = unsafe { zeroed::<IO_STATUS_BLOCK>() };
    let status = unsafe {
        NtSetInformationFile(
            source_file.as_raw_handle() as _,
            &mut io_status,
            information.cast(),
            buffer_size as u32,
            FileRenameInformation,
        )
    };
    if status >= 0 {
        Ok(())
    } else {
        let win32 = unsafe { RtlNtStatusToDosError(status) };
        Err(std::io::Error::from_raw_os_error(win32 as i32))
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(crate) fn move_entry_at_no_replace(
    _source_dir: &Dir,
    _source_name: &std::ffi::OsStr,
    _source_file: &File,
    _destination_dir: &Dir,
    _destination_name: &std::ffi::OsStr,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "descriptor-relative atomic no-replace rename is unavailable on this platform",
    ))
}

/// Atomically exchange two existing names without deleting either object.
/// This is the POSIX fixed-slot primitive: a disposable public control file
/// can trade places with an exact private zero tombstone, after which both
/// identities are re-opened and attested by the caller.
#[cfg(target_os = "linux")]
fn exchange_entries_at(
    left_dir: &Dir,
    left_name: &OsStr,
    right_dir: &Dir,
    right_name: &OsStr,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    validate_entry_name(left_name)?;
    validate_entry_name(right_name)?;
    let left_name = CString::new(left_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let right_name = CString::new(right_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe {
        libc::renameat2(
            left_dir.as_raw_fd(),
            left_name.as_ptr(),
            right_dir.as_raw_fd(),
            right_name.as_ptr(),
            libc::RENAME_EXCHANGE,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(target_os = "macos")]
fn exchange_entries_at(
    left_dir: &Dir,
    left_name: &OsStr,
    right_dir: &Dir,
    right_name: &OsStr,
) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    validate_entry_name(left_name)?;
    validate_entry_name(right_name)?;
    let left_name = CString::new(left_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    let right_name = CString::new(right_name.as_bytes())
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    if unsafe {
        libc::renameatx_np(
            left_dir.as_raw_fd(),
            left_name.as_ptr(),
            right_dir.as_raw_fd(),
            right_name.as_ptr(),
            libc::RENAME_SWAP,
        )
    } == 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Remove the exact file represented by a retained Windows handle. POSIX
/// disposition semantics unlink its visible name when this deletion handle is
/// closed even if other readers still retain handles to the old file object.
#[cfg(windows)]
pub(crate) fn delete_file_by_handle(file: &File) -> std::io::Result<()> {
    use std::mem::size_of;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfoEx, SetFileInformationByHandle, FILE_DISPOSITION_FLAG_DELETE,
        FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO_EX,
    };

    let disposition = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    if unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as _,
            FileDispositionInfoEx,
            (&disposition as *const FILE_DISPOSITION_INFO_EX).cast(),
            size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } != 0
    {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

fn directory_open_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn cap_directory_open_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

/// Open options used only while atomically moving one exact child directory
/// on Windows. Ordinary directory capabilities intentionally deny delete
/// sharing so their visible names cannot move while they are trusted. A
/// `FileRenameInfo` operation is different: Windows requires DELETE access on
/// the exact source handle, and every simultaneously-open handle to that
/// child must permit delete sharing. Keep that weaker sharing contract local
/// to the short move transaction; the published child is rebound through the
/// ordinary no-delete-sharing path before it is returned.
#[cfg(windows)]
fn cap_directory_rename_open_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    const DELETE: u32 = 0x0001_0000;
    const GENERIC_READ: u32 = 0x8000_0000;
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_WRITE: u32 = 0x0000_0002;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
    options
        .read(true)
        .access_mode(GENERIC_READ | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
        );
    options
}

fn directory_metadata_is_safe(metadata: &fs::Metadata) -> bool {
    metadata.is_dir() && !metadata_is_link_or_reparse(metadata)
}

fn cap_directory_metadata_is_safe(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.is_dir() && !metadata.file_type().is_symlink()
}

fn cap_metadata_identity_matches_opened(metadata: &cap_std::fs::Metadata, opened: &File) -> bool {
    let Ok(opened) = opened.metadata() else {
        return false;
    };
    let stable = metadata.is_file() == opened.is_file()
        && metadata.is_dir() == opened.is_dir()
        && metadata.len() == opened.len();
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::unix::fs::MetadataExt as StdMetadataExt;
        stable
            && CapMetadataExt::dev(metadata) == StdMetadataExt::dev(&opened)
            && CapMetadataExt::ino(metadata) == StdMetadataExt::ino(&opened)
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::windows::fs::MetadataExt as StdMetadataExt;
        stable
            && CapMetadataExt::file_attributes(metadata) == StdMetadataExt::file_attributes(&opened)
            && CapMetadataExt::creation_time(metadata) == StdMetadataExt::creation_time(&opened)
            && CapMetadataExt::last_write_time(metadata) == StdMetadataExt::last_write_time(&opened)
    }
    #[cfg(not(any(unix, windows)))]
    {
        stable
    }
}

/// Compare descriptor-relative directory metadata with an opened directory.
/// Directory byte length is mutable namespace bookkeeping on Darwin/APFS, not
/// an identity attribute, so the regular-file helper above is intentionally
/// not used here.
fn cap_directory_metadata_identity_matches_opened(
    metadata: &cap_std::fs::Metadata,
    opened: &File,
) -> bool {
    let Ok(opened) = opened.metadata() else {
        return false;
    };
    let stable = metadata.is_dir() && opened.is_dir();
    #[cfg(unix)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::unix::fs::MetadataExt as StdMetadataExt;
        stable
            && CapMetadataExt::dev(metadata) == StdMetadataExt::dev(&opened)
            && CapMetadataExt::ino(metadata) == StdMetadataExt::ino(&opened)
    }
    #[cfg(windows)]
    {
        use cap_std::fs::MetadataExt as CapMetadataExt;
        use std::os::windows::fs::MetadataExt as StdMetadataExt;
        stable
            && CapMetadataExt::creation_time(metadata) == StdMetadataExt::creation_time(&opened)
            && CapMetadataExt::file_attributes(metadata) == StdMetadataExt::file_attributes(&opened)
    }
    #[cfg(not(any(unix, windows)))]
    {
        stable
    }
}

#[cfg(unix)]
fn opened_directory_is_safe(file: &File) -> bool {
    file.metadata().is_ok_and(|metadata| metadata.is_dir())
}

#[cfg(windows)]
fn opened_directory_is_safe(file: &File) -> bool {
    use std::mem::zeroed;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT,
    };

    let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
    (unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) }) != 0
        && info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0
        && info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT == 0
}

#[cfg(not(any(unix, windows)))]
fn opened_directory_is_safe(file: &File) -> bool {
    file.metadata().is_ok_and(|metadata| metadata.is_dir())
}

fn validate_opened_directory_pair(opened: &File, confirmed: &File) -> std::io::Result<()> {
    let opened_metadata = opened.metadata()?;
    let confirmed_metadata = confirmed.metadata()?;
    if !opened_directory_is_safe(opened)
        || !opened_directory_is_safe(confirmed)
        || !directory_metadata_identity_matches(&opened_metadata, &confirmed_metadata)
        || !open_file_identity_matches(opened, confirmed)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory changed while it was opened",
        ));
    }
    Ok(())
}

/// Open and retain one ambient directory without following a leaf
/// symlink/reparse point. The lexical entry, first handle, final lexical entry,
/// and confirming handle must all identify the same directory.
pub(crate) fn open_directory_no_follow(path: &Path) -> std::io::Result<Dir> {
    let lexical_before = fs::symlink_metadata(path)?;
    if !directory_metadata_is_safe(&lexical_before) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory is not a safe directory",
        ));
    }
    let opened = directory_open_options().open(path)?;
    let lexical_after = fs::symlink_metadata(path)?;
    let confirmed = directory_open_options().open(path)?;
    if !directory_metadata_is_safe(&lexical_after)
        || !directory_metadata_identity_matches(&lexical_before, &opened.metadata()?)
        || !directory_metadata_identity_matches(&opened.metadata()?, &lexical_after)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory changed while it was opened",
        ));
    }
    validate_opened_directory_pair(&opened, &confirmed)?;
    Ok(Dir::from_std_file(opened))
}

/// Descriptor-relative form used while walking mutation parents and opening
/// archive/staging directories. The hook exists for exact boundary regression
/// tests and runs after an exact no-follow handle has been bound to the lexical
/// entry but before the handle used by the caller is opened.
#[doc(hidden)]
pub(crate) fn open_directory_at_no_follow_with_hook(
    parent: &Dir,
    name: &std::ffi::OsStr,
    before_open: impl FnOnce(),
) -> std::io::Result<Dir> {
    validate_entry_name(name)?;
    let lexical_before = parent.symlink_metadata(name)?;
    if !cap_directory_metadata_is_safe(&lexical_before) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory is not a safe directory",
        ));
    }
    let options = cap_directory_open_options();
    let bound_before = parent.open_with(name, &options)?.into_std();
    let bound_before_confirmed = parent.open_with(name, &options)?.into_std();
    validate_opened_directory_pair(&bound_before, &bound_before_confirmed)?;
    if !cap_directory_metadata_identity_matches_opened(&lexical_before, &bound_before) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory changed while its lexical identity was bound",
        ));
    }

    before_open();
    let opened = parent.open_with(name, &options)?.into_std();
    let lexical_after = parent.symlink_metadata(name)?;
    let confirmed = parent.open_with(name, &options)?.into_std();
    if !cap_directory_metadata_is_safe(&lexical_after)
        || !cap_directory_metadata_identity_matches_opened(&lexical_after, &opened)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory changed while it was opened",
        ));
    }
    validate_opened_directory_pair(&opened, &confirmed)?;
    if !open_file_identity_matches(&bound_before, &opened) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy directory changed after its lexical identity was bound",
        ));
    }
    Ok(Dir::from_std_file(opened))
}

pub(crate) fn open_directory_at_no_follow(
    parent: &Dir,
    name: &std::ffi::OsStr,
) -> std::io::Result<Dir> {
    open_directory_at_no_follow_with_hook(parent, name, || {})
}

/// Bind an exact child directory to a Windows handle that is authorized for
/// `SetFileInformationByHandle(FileRenameInfo)`. Both handles share delete so
/// they can coexist, but lexical metadata and stable file identity must agree
/// before the caller receives either capability. The later exact-handle move
/// remains safe if the ambient name changes after this function returns.
#[cfg(windows)]
fn open_directory_at_for_rename(parent: &Dir, name: &OsStr) -> std::io::Result<Dir> {
    validate_entry_name(name)?;
    let lexical_before = parent.symlink_metadata(name)?;
    if !cap_directory_metadata_is_safe(&lexical_before) {
        return Err(invalid_recovery_path(
            "recovery directory is not a safe directory",
        ));
    }
    let options = cap_directory_rename_open_options();
    let opened = parent.open_with(name, &options)?.into_std();
    let confirmed = parent.open_with(name, &options)?.into_std();
    let lexical_after = parent.symlink_metadata(name)?;
    if !cap_directory_metadata_is_safe(&lexical_after)
        || !cap_directory_metadata_identity_matches_opened(&lexical_before, &opened)
        || !cap_directory_metadata_identity_matches_opened(&lexical_after, &opened)
    {
        return Err(invalid_recovery_path(
            "recovery directory changed while its rename handle was opened",
        ));
    }
    validate_opened_directory_pair(&opened, &confirmed)?;
    Ok(Dir::from_std_file(opened))
}

/// A retained, no-follow file capability used by private policy state.
///
/// Its directory capability, entry name, and exact file handle stay bound for
/// the complete transaction. Callers cannot obtain the raw handles and must
/// use the descriptor-relative publication and quarantine operations below.
pub struct BoundRecoveryFile {
    parent_chain: BoundDirectoryChain,
    name: OsString,
    file: File,
    display_path: PathBuf,
    expected_digest: Cell<Option<(u64, [u8; 32])>>,
}

/// A retained owner-private lease identity whose Windows handle denies delete
/// sharing for its full lifetime. Keeping this distinct from rename-capable
/// control files prevents callers from accidentally weakening the lease-name
/// invariant to support an unrelated publication workflow.
pub struct BoundRecoveryLeaseFile {
    file: BoundRecoveryFile,
}

/// A retained owner-private directory capability for policy state.
pub struct BoundRecoveryDirectory {
    chain: BoundDirectoryChain,
    owner_private_namespace: bool,
}

struct BoundDirectoryChain {
    anchor_path: PathBuf,
    anchor: Dir,
    components: Vec<(OsString, Dir)>,
    display_path: PathBuf,
}

impl BoundDirectoryChain {
    fn leaf(&self) -> &Dir {
        self.components
            .last()
            .map(|(_, directory)| directory)
            .unwrap_or(&self.anchor)
    }

    fn attest(&self) -> std::io::Result<()> {
        let reopened_anchor = open_directory_no_follow(&self.anchor_path)?;
        if !open_file_identity_matches(
            &self.anchor.try_clone()?.into_std_file(),
            &reopened_anchor.try_clone()?.into_std_file(),
        ) {
            return Err(invalid_recovery_path(
                "recovery filesystem anchor changed; no cleanup was attempted",
            ));
        }
        let mut current = reopened_anchor;
        for (name, expected) in &self.components {
            let opened = open_directory_at_no_follow(&current, name)?;
            if !open_file_identity_matches(
                &expected.try_clone()?.into_std_file(),
                &opened.try_clone()?.into_std_file(),
            ) {
                return Err(invalid_recovery_path(
                    "recovery directory ancestor moved or was replaced; no cleanup was attempted",
                ));
            }
            current = opened;
        }
        Ok(())
    }

    fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self {
            anchor_path: self.anchor_path.clone(),
            anchor: self.anchor.try_clone()?,
            components: self
                .components
                .iter()
                .map(|(name, directory)| Ok((name.clone(), directory.try_clone()?)))
                .collect::<std::io::Result<Vec<_>>>()?,
            display_path: self.display_path.clone(),
        })
    }

    fn with_child(&self, name: &OsStr, directory: &Dir) -> std::io::Result<Self> {
        let mut cloned = self.try_clone()?;
        cloned
            .components
            .push((name.to_os_string(), directory.try_clone()?));
        cloned.display_path.push(name);
        Ok(cloned)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamespaceDurabilityPolicy {
    #[cfg(not(windows))]
    DirectoryFsync,
    #[cfg(windows)]
    WriteThroughExactHandles,
}

const fn namespace_durability_policy() -> NamespaceDurabilityPolicy {
    #[cfg(not(windows))]
    {
        NamespaceDurabilityPolicy::DirectoryFsync
    }
    #[cfg(windows)]
    {
        NamespaceDurabilityPolicy::WriteThroughExactHandles
    }
}

/// Complete the platform-specific durability side of a capability-bound
/// namespace mutation without asking a read-only Windows traversal handle to
/// flush. POSIX persists the directory entry with a real directory fsync.
/// Windows exact file handles used by every caller are opened with
/// `FILE_FLAG_WRITE_THROUGH`; after their rename completes, re-attesting this
/// chain is the remaining safe parent operation.
fn sync_bound_namespace_after_write_through_change(
    chain: &BoundDirectoryChain,
) -> std::io::Result<()> {
    chain.attest()?;
    match namespace_durability_policy() {
        #[cfg(not(windows))]
        NamespaceDurabilityPolicy::DirectoryFsync => {
            chain.leaf().try_clone()?.into_std_file().sync_all()
        }
        #[cfg(windows)]
        NamespaceDurabilityPolicy::WriteThroughExactHandles => Ok(()),
    }
}

fn invalid_recovery_path(message: &'static str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::PermissionDenied, message)
}

/// Rewrite only Darwin's fixed root-level compatibility aliases before the
/// no-follow capability walk begins.
///
/// macOS reports ordinary temporary paths as `/var/folders/...`, while `/var`
/// is a system compatibility symlink to `/private/var` (and `/tmp` to
/// `/private/tmp`). Treating that first component like an application-controlled
/// symlink makes every normal Darwin temp path fail closed. Expanding these
/// exact root aliases keeps the subsequent walk fully descriptor-relative and
/// no-follow; no caller-controlled component is resolved or trusted.
#[cfg(any(target_os = "macos", test))]
fn normalize_macos_system_root_alias(path: &Path) -> PathBuf {
    for (alias, target) in [
        (Path::new("/var"), Path::new("/private/var")),
        (Path::new("/tmp"), Path::new("/private/tmp")),
    ] {
        if let Ok(relative) = path.strip_prefix(alias) {
            return target.join(relative);
        }
    }
    path.to_path_buf()
}

/// Normalize only fixed operating-system root aliases that identify the same
/// namespace. This never resolves a caller-controlled component.
pub(crate) fn normalize_platform_path_identity(path: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    return normalize_macos_system_root_alias(path);
    #[cfg(not(target_os = "macos"))]
    path.to_path_buf()
}

/// Turn a path into an absolute lexical path containing no `.` or `..`.
/// Rejecting those components makes every subsequent component walk auditable
/// and prevents a capability from being confused with a different spelling.
fn absolute_normal_path(path: &Path) -> std::io::Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let absolute = normalize_platform_path_identity(&absolute);
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::Normal(name) => normalized.push(name),
            Component::CurDir | Component::ParentDir => {
                return Err(invalid_recovery_path(
                    "recovery path contains a non-canonical component",
                ));
            }
        }
    }
    if !normalized.is_absolute() {
        return Err(invalid_recovery_path("recovery path is not absolute"));
    }
    Ok(normalized)
}

fn absolute_anchor_and_names(path: &Path) -> std::io::Result<(PathBuf, Vec<OsString>, PathBuf)> {
    let normalized = absolute_normal_path(path)?;
    let mut anchor = PathBuf::new();
    let mut names = Vec::new();
    for component in normalized.components() {
        match component {
            Component::Prefix(prefix) => anchor.push(prefix.as_os_str()),
            Component::RootDir => anchor.push(Path::new(std::path::MAIN_SEPARATOR_STR)),
            Component::Normal(name) => names.push(name.to_os_string()),
            Component::CurDir | Component::ParentDir => unreachable!("normalized above"),
        }
    }
    if !anchor.is_absolute() {
        return Err(invalid_recovery_path(
            "recovery path has no absolute filesystem anchor",
        ));
    }
    Ok((anchor, names, normalized))
}

fn walk_absolute_directory(
    path: &Path,
    create_missing: bool,
) -> std::io::Result<BoundDirectoryChain> {
    let (anchor, names, normalized) = absolute_anchor_and_names(path)?;
    let anchor_directory = open_directory_no_follow(&anchor)?;
    let mut directory = anchor_directory.try_clone()?;
    let mut components = Vec::with_capacity(names.len());
    for name in names {
        match open_directory_at_no_follow(&directory, &name) {
            Ok(next) => directory = next,
            Err(error) if create_missing && error.kind() == std::io::ErrorKind::NotFound => {
                match directory.create_dir(&name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
                directory = open_directory_at_no_follow(&directory, &name)?;
            }
            Err(error) => return Err(error),
        }
        components.push((name, directory.try_clone()?));
    }
    Ok(BoundDirectoryChain {
        anchor_path: anchor,
        anchor: anchor_directory,
        components,
        display_path: normalized,
    })
}

#[cfg(any(windows, test))]
const WINDOWS_FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(any(windows, test))]
const WINDOWS_FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;

#[cfg(any(windows, test))]
const fn windows_exact_rename_file_flags() -> u32 {
    WINDOWS_FILE_FLAG_OPEN_REPARSE_POINT | WINDOWS_FILE_FLAG_WRITE_THROUGH
}

fn recovery_file_open_options(create_new: bool) -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true).write(true).create_new(create_new);
    #[cfg(unix)]
    options
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE | DELETE)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(windows_exact_rename_file_flags());
    }
    options
}

/// A private control file uses the same no-follow/single-link contract as a
/// recovery file, plus the Windows rights needed to tighten its DACL through
/// the exact handle after an OS lease is acquired.
fn private_control_file_open_options(create_new: bool) -> CapOpenOptions {
    let options = recovery_file_open_options(create_new);
    #[cfg(windows)]
    {
        let mut options = options;
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        const READ_CONTROL: u32 = 0x0002_0000;
        const WRITE_DAC: u32 = 0x0004_0000;
        const WRITE_OWNER: u32 = 0x0008_0000;
        options
            .access_mode(
                GENERIC_READ | GENERIC_WRITE | DELETE | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
            )
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH);
        options
    }
    #[cfg(not(windows))]
    options
}

/// Open options reserved for a retained cross-process lease file.
///
/// Ordinary private control files permit delete sharing on Windows because
/// several publication protocols rename them after binding. A lease is the
/// opposite: its visible name must remain attached to the exact locked file
/// for the full lock lifetime. Denying only delete sharing preserves fs2's
/// ability to open a second read/write handle and contend on the advisory lock
/// while making rename/delete-and-recreate split brain impossible.
fn private_lease_file_open_options(create_new: bool) -> CapOpenOptions {
    let options = private_control_file_open_options(create_new);
    #[cfg(windows)]
    {
        let mut options = options;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_FLAG_WRITE_THROUGH: u32 = 0x8000_0000;
        const READ_CONTROL: u32 = 0x0002_0000;
        const WRITE_DAC: u32 = 0x0004_0000;
        const WRITE_OWNER: u32 = 0x0008_0000;
        options
            .access_mode(GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH);
        options
    }
    #[cfg(not(windows))]
    options
}

fn open_private_lease_file_at(parent: &Dir, name: &OsStr) -> std::io::Result<File> {
    validate_entry_name(name)?;
    let lexical_before = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_before) {
        return Err(invalid_recovery_path(
            "private lease file is not a no-follow regular file",
        ));
    }
    let options = private_lease_file_open_options(false);
    let opened = parent.open_with(name, &options)?.into_std();
    let lexical_after = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_after)
        || !cap_metadata_identity_matches_opened(&lexical_before, &opened)
        || !cap_metadata_identity_matches_opened(&lexical_after, &opened)
        || !opened_regular_file_is_safe(&opened)
    {
        return Err(invalid_recovery_path(
            "private lease file changed while it was being bound",
        ));
    }
    Ok(opened)
}

fn open_recovery_file_at(parent: &Dir, name: &OsStr) -> std::io::Result<File> {
    validate_entry_name(name)?;
    let lexical_before = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_before) {
        return Err(invalid_recovery_path(
            "recovery source is not a no-follow regular file",
        ));
    }
    let options = recovery_file_open_options(false);
    let opened = parent.open_with(name, &options)?.into_std();
    let confirmed = parent.open_with(name, &options)?.into_std();
    let lexical_after = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_after)
        || !cap_metadata_identity_matches_opened(&lexical_before, &opened)
        || !cap_metadata_identity_matches_opened(&lexical_after, &opened)
        || !opened_regular_file_is_safe(&opened)
        || !opened_regular_file_is_safe(&confirmed)
        || !open_file_identity_matches(&opened, &confirmed)
    {
        return Err(invalid_recovery_path(
            "recovery source changed or has multiple links",
        ));
    }
    Ok(opened)
}

fn open_regular_file_at_allow_links(
    parent: &Dir,
    name: &OsStr,
    writable: bool,
) -> std::io::Result<File> {
    validate_entry_name(name)?;
    let lexical_before = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_before) {
        return Err(invalid_recovery_path(
            "capability-bound file is not a no-follow regular file",
        ));
    }
    let mut options = CapOpenOptions::new();
    options.read(true).write(writable);
    #[cfg(unix)]
    options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const DELETE: u32 = 0x0001_0000;
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        let access = GENERIC_READ | DELETE | if writable { GENERIC_WRITE } else { 0 };
        options
            .access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .custom_flags(windows_exact_rename_file_flags());
    }
    let opened = parent.open_with(name, &options)?.into_std();
    let confirmed = parent.open_with(name, &options)?.into_std();
    let lexical_after = parent.symlink_metadata(name)?;
    if !cap_lexical_regular_file_is_safe(&lexical_after)
        || !opened.metadata()?.is_file()
        || !confirmed.metadata()?.is_file()
        || !open_file_identity_matches(&opened, &confirmed)
    {
        return Err(invalid_recovery_path(
            "capability-bound file changed while it was opened",
        ));
    }
    Ok(opened)
}

fn random_recovery_name(prefix: &str) -> std::io::Result<OsString> {
    let mut nonce = [0u8; 16];
    getrandom::fill(&mut nonce).map_err(|error| {
        std::io::Error::other(format!("could not generate recovery nonce: {error}"))
    })?;
    let suffix = nonce
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(OsString::from(format!(".{prefix}-{suffix}")))
}

fn hash_exact_recovery_file(file: &File) -> std::io::Result<(u64, [u8; 32])> {
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("recovery hash length overflow"))?;
    }
    Ok((bytes, digest.finalize().into()))
}

fn hash_exact_recovery_file_bounded(
    file: &File,
    max_bytes: u64,
    deadline: std::time::Instant,
) -> std::io::Result<(u64, [u8; 32])> {
    if file.metadata()?.len() > max_bytes {
        return Err(invalid_recovery_path(
            "capability-bound file exceeds its attestation byte budget",
        ));
    }
    let mut reader = file.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut digest = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 256 * 1024];
    loop {
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "capability-bound file exceeded its attestation deadline",
            ));
        }
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("recovery hash length overflow"))?;
        if bytes > max_bytes {
            return Err(invalid_recovery_path(
                "capability-bound file exceeds its attestation byte budget",
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok((bytes, digest.finalize().into()))
}

fn create_private_directory_at(
    parent: &Dir,
    name: &OsStr,
    _display_path: &Path,
) -> std::io::Result<Dir> {
    validate_entry_name(name)?;
    #[cfg(unix)]
    let builder = {
        use cap_std::fs::DirBuilderExt;
        let mut builder = CapDirBuilder::new();
        builder.mode(0o700);
        builder
    };
    #[cfg(not(any(unix, windows)))]
    let builder = CapDirBuilder::new();
    #[cfg(not(windows))]
    parent.create_dir_with(name, &builder)?;
    #[cfg(windows)]
    crate::overlays::create_owner_only_directory(_display_path)?;
    let directory = open_directory_at_no_follow(parent, name)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let exact = directory.try_clone()?.into_std_file();
        exact.set_permissions(fs::Permissions::from_mode(0o700))?;
        let metadata = exact.metadata()?;
        if !metadata.is_dir() || metadata.mode() & 0o777 != 0o700 {
            return Err(invalid_recovery_path(
                "recovery directory is not owner-private",
            ));
        }
    }
    #[cfg(windows)]
    {
        // The parent and exact child handles both deny FILE_SHARE_DELETE. The
        // protected DACL was attached atomically in CreateDirectoryW, before
        // the child became visible; verify it through the retained capability
        // without rewriting an already-observable security descriptor.
        let exact = directory.try_clone()?.into_std_file();
        crate::overlays::attest_owner_only_directory_handle(&exact)?;
        let confirmed = open_directory_at_no_follow(parent, name)?;
        if !open_file_identity_matches(&exact, &confirmed.into_std_file()) {
            return Err(invalid_recovery_path(
                "private recovery directory changed during DACL attestation",
            ));
        }
    }
    Ok(directory)
}

impl BoundRecoveryDirectory {
    fn attest_location(&self) -> std::io::Result<()> {
        self.chain.attest()
    }

    /// Bind an existing directory without following any component. This does
    /// not change its permissions; it is used for user-selected directories
    /// where Minutes must retain an exact capability without rewriting mode.
    pub fn bind_existing(path: &Path) -> std::io::Result<Self> {
        let chain = walk_absolute_directory(path, false)?;
        chain.attest()?;
        Ok(Self {
            chain,
            owner_private_namespace: false,
        })
    }

    /// Create or bind an owner-private application directory and retain the
    /// complete no-follow capability chain. This is the queue boundary for
    /// recovery inputs materialized beneath Minutes' private jobs namespace.
    pub fn prepare_owner_private(path: &Path) -> std::io::Result<Self> {
        let normalized = absolute_normal_path(path)?;
        let parent = normalized
            .parent()
            .ok_or_else(|| invalid_recovery_path("private recovery directory has no parent"))?;
        let name = normalized
            .file_name()
            .ok_or_else(|| invalid_recovery_path("private recovery directory has no name"))?;
        let parent = walk_absolute_directory(parent, true)?;
        let directory = match open_directory_at_no_follow(parent.leaf(), name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let display_path = parent.display_path.join(name);
                match create_private_directory_at(parent.leaf(), name, &display_path) {
                    Ok(directory) => directory,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        open_directory_at_no_follow(parent.leaf(), name)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let exact = directory.try_clone()?.into_std_file();
            exact.set_permissions(fs::Permissions::from_mode(0o700))?;
            let metadata = exact.metadata()?;
            if !metadata.is_dir() || metadata.mode() & 0o777 != 0o700 {
                return Err(invalid_recovery_path(
                    "private recovery directory is not mode 0700",
                ));
            }
        }
        #[cfg(windows)]
        crate::overlays::attest_owner_only_directory_handle(
            &directory.try_clone()?.into_std_file(),
        )
        .map_err(|error| {
            std::io::Error::new(
                error.kind(),
                format!(
                    "private recovery directory DACL attestation failed for {}: {error}",
                    normalized.display()
                ),
            )
        })?;
        let chain = parent.with_child(name, &directory)?;
        chain.attest()?;
        Ok(Self {
            chain,
            owner_private_namespace: true,
        })
    }

    /// Create or bind one owner-private child beneath this exact retained
    /// owner-private capability. The child is derived descriptor-relative;
    /// callers never re-resolve the parent through an ambient pathname.
    pub fn prepare_owner_private_child(&self, name: &OsStr) -> std::io::Result<Self> {
        validate_entry_name(name)?;
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private child requires an owner-private parent capability",
            ));
        }
        self.attest_location()?;
        let display_path = self.chain.display_path.join(name);
        let directory = match open_directory_at_no_follow(self.chain.leaf(), name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match create_private_directory_at(self.chain.leaf(), name, &display_path) {
                    Ok(directory) => directory,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        open_directory_at_no_follow(self.chain.leaf(), name)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::{MetadataExt, PermissionsExt};
            let exact = directory.try_clone()?.into_std_file();
            exact.set_permissions(fs::Permissions::from_mode(0o700))?;
            let metadata = exact.metadata()?;
            if !metadata.is_dir() || metadata.mode() & 0o777 != 0o700 {
                return Err(invalid_recovery_path(
                    "private child directory is not mode 0700",
                ));
            }
        }
        #[cfg(windows)]
        crate::overlays::attest_owner_only_directory_handle(
            &directory.try_clone()?.into_std_file(),
        )?;
        self.attest_location()?;
        let chain = self.chain.with_child(name, &directory)?;
        chain.attest()?;
        Ok(Self {
            chain,
            owner_private_namespace: true,
        })
    }

    /// Create one new owner-private child beneath this exact retained
    /// owner-private capability. Unlike `prepare_owner_private_child`, this
    /// never adopts an existing pathname: callers allocating a fresh private
    /// generation receive `AlreadyExists` and must choose another name.
    pub(crate) fn create_new_owner_private_child(&self, name: &OsStr) -> std::io::Result<Self> {
        validate_entry_name(name)?;
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private child requires an owner-private parent capability",
            ));
        }
        self.attest_location()?;
        let display_path = self.chain.display_path.join(name);
        let directory = create_private_directory_at(self.chain.leaf(), name, &display_path)?;
        self.attest_location()?;
        let chain = self.chain.with_child(name, &directory)?;
        chain.attest()?;
        Ok(Self {
            chain,
            owner_private_namespace: true,
        })
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn bind_existing_owner_private_child(&self, name: &OsStr) -> std::io::Result<Self> {
        validate_entry_name(name)?;
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private child requires an owner-private parent capability",
            ));
        }
        self.attest_location()?;
        let directory = open_directory_at_no_follow(self.chain.leaf(), name)?;
        self.attest_location()?;
        let chain = self.chain.with_child(name, &directory)?;
        chain.attest()?;
        Ok(Self {
            chain,
            owner_private_namespace: true,
        })
    }

    pub fn display_path(&self) -> &Path {
        &self.chain.display_path
    }

    /// Return the stable identity of the exact retained directory handle.
    /// Callers must compare this proof instead of taking path metadata before
    /// binding, which leaves a same-UID rename window between the two steps.
    pub fn recovery_directory_proof(&self) -> std::io::Result<RecoveryDirectoryProof> {
        self.attest_location()?;
        Ok(RecoveryDirectoryProof {
            identity: bound_file_identity(&self.chain.leaf().try_clone()?.into_std_file())?,
        })
    }

    /// Re-attest both the complete path chain and the exact retained directory
    /// identity against a durable private proof.
    pub fn attest_recovery_directory_proof(
        &self,
        expected: &RecoveryDirectoryProof,
    ) -> std::io::Result<()> {
        let current = self.recovery_directory_proof()?;
        if &current != expected {
            return Err(invalid_recovery_path(
                "recovery destination directory changed after it was bound",
            ));
        }
        Ok(())
    }

    /// Re-attest the complete anchor-to-destination chain before callers
    /// sanitize any original source handles.
    pub fn attest_for_source_cleanup(&self) -> std::io::Result<()> {
        self.attest_location()
    }

    pub fn entry_exists(&self, name: &OsStr) -> std::io::Result<bool> {
        validate_entry_name(name)?;
        match self.chain.leaf().symlink_metadata(name) {
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Bind one exact single-link regular child without resolving its parent
    /// through an ambient pathname.
    pub fn bind_exact_file(&self, name: &OsStr) -> std::io::Result<BoundRecoveryFile> {
        validate_entry_name(name)?;
        self.attest_location()?;
        let file = open_recovery_file_at(self.chain.leaf(), name)?;
        let bound = BoundRecoveryFile {
            parent_chain: self.chain.try_clone()?,
            name: name.to_os_string(),
            file,
            display_path: self.chain.display_path.join(name),
            expected_digest: Cell::new(None),
        };
        bound.attest_visible_identity()?;
        Ok(bound)
    }

    /// Bind a deterministic private control leaf that may receive private
    /// bytes. Unlike generic recovery binding, an existing Windows leaf is
    /// accepted only when its exact handle already carries the canonical
    /// protected owner-only descriptor; it is never "repaired" after a
    /// possibly hostile process retained access.
    pub(crate) fn bind_owner_private_exact_file(
        &self,
        name: &OsStr,
    ) -> std::io::Result<BoundRecoveryFile> {
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private control file requires an owner-private namespace",
            ));
        }
        let bound = self.bind_exact_file(name)?;
        #[cfg(windows)]
        crate::overlays::attest_owner_only_file_handle(&bound.file)?;
        Ok(bound)
    }

    /// Bind one exact regular child through this retained directory while
    /// permitting pre-existing hard links. This is used only for job records
    /// whose crash-recovery protocol must move the exact observed inode into
    /// private quarantine before deciding whether an alias can be retired.
    #[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
    pub(crate) fn bind_file_allow_links(&self, name: &OsStr) -> std::io::Result<BoundRecoveryFile> {
        validate_entry_name(name)?;
        self.attest_location()?;
        let file = open_regular_file_at_allow_links(self.chain.leaf(), name, false)?;
        let confirmed = open_regular_file_at_allow_links(self.chain.leaf(), name, false)?;
        if !open_file_identity_matches(&file, &confirmed) {
            return Err(invalid_recovery_path(
                "capability-bound linked file changed while it was being bound",
            ));
        }
        let bound = BoundRecoveryFile {
            parent_chain: self.chain.try_clone()?,
            name: name.to_os_string(),
            file,
            display_path: self.chain.display_path.join(name),
            expected_digest: Cell::new(None),
        };
        Ok(bound)
    }

    /// Create one single-link regular file beneath this exact retained
    /// directory without following or re-resolving any ambient parent path.
    /// The directory chain and created leaf are both re-attested before the
    /// exact writable handle is returned.
    pub fn create_new_exact_file(&self, name: &OsStr) -> std::io::Result<File> {
        validate_entry_name(name)?;
        self.attest_location()?;
        let options = if self.owner_private_namespace {
            private_control_file_open_options(true)
        } else {
            recovery_file_open_options(true)
        };
        let file = self.chain.leaf().open_with(name, &options)?.into_std();
        if !opened_regular_file_is_safe(&file) {
            return Err(invalid_recovery_path(
                "new capability-bound file is not a single-link regular file",
            ));
        }
        #[cfg(windows)]
        if self.owner_private_namespace {
            // The directory was created atomically owner-only and its exact
            // retained handle has just been re-attested. No other principal
            // can open this newly created child before its exact handle is
            // given the canonical protected file DACL.
            crate::overlays::secure_private_file_handle(&file)?;
        }
        self.attest_location()?;
        let confirmed = open_recovery_file_at(self.chain.leaf(), name)?;
        if !open_file_identity_matches(&file, &confirmed) {
            return Err(invalid_recovery_path(
                "new capability-bound file changed during creation",
            ));
        }
        Ok(file)
    }

    /// Create one uniquely named, bounded control file beneath this retained
    /// owner-private directory. The file is written and synced through the
    /// exact handle returned by the no-follow create operation; callers retain
    /// the resulting capability for exact retirement after a watcher observes
    /// its pathname.
    pub fn create_random_private_control_file(
        &self,
        prefix: &str,
        bytes: &[u8],
    ) -> std::io::Result<BoundRecoveryFile> {
        self.create_random_private_control_file_with_hook(prefix, bytes, |_| Ok(()))
    }

    /// Variant used by watcher protocols that must register the unpredictable
    /// exact path before the create event can be delivered. The hook runs
    /// after the random name is selected but before any filesystem mutation.
    #[doc(hidden)]
    pub fn create_random_private_control_file_with_hook(
        &self,
        prefix: &str,
        bytes: &[u8],
        before_create: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> std::io::Result<BoundRecoveryFile> {
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private control files require an owner-private directory",
            ));
        }
        if bytes.len() > 4 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "private control file exceeds its bounded size",
            ));
        }
        let name = random_recovery_name(prefix)?;
        let display_path = self.chain.display_path.join(&name);
        before_create(&display_path)?;
        let parent_chain = self.chain.try_clone()?;
        let file = self.create_new_exact_file(&name)?;
        let mut bound = BoundRecoveryFile {
            parent_chain,
            name: name.clone(),
            file,
            display_path,
            expected_digest: Cell::new(None),
        };
        if let Err(error) = bound.fill_exact_empty_visible(bytes) {
            let _ = self.remove_owned_private_file(bound);
            return Err(error);
        }
        if let Err(error) = self.sync() {
            let _ = self.remove_owned_private_file(bound);
            return Err(error);
        }
        Ok(bound)
    }

    /// Prove that two retained directory capabilities live on the same
    /// filesystem. Policy-watcher control traffic uses this before relying on
    /// one native event stream to order a protected corpus and its sibling
    /// control directory.
    pub fn is_same_filesystem(&self, other: &Self) -> std::io::Result<bool> {
        self.attest_location()?;
        other.attest_location()?;
        let this = bound_file_filesystem_identity(&self.chain.leaf().try_clone()?.into_std_file())?;
        let that =
            bound_file_filesystem_identity(&other.chain.leaf().try_clone()?.into_std_file())?;
        Ok(this == that)
    }

    /// concurrent hard link outside this namespace survives unchanged.
    pub fn remove_owned_private_file(&self, file: BoundRecoveryFile) -> std::io::Result<()> {
        self.remove_owned_private_file_with_hook(file, || {})
    }

    #[doc(hidden)]
    pub(crate) fn remove_owned_private_file_with_hook(
        &self,
        file: BoundRecoveryFile,
        after_final_identity_check: impl FnOnce(),
    ) -> std::io::Result<()> {
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "exact retirement requires an owner-private namespace",
            ));
        }
        self.attest_location()?;
        file.parent_chain.attest()?;
        let this_parent = self.chain.leaf().try_clone()?.into_std_file();
        let file_parent = file.parent_chain.leaf().try_clone()?.into_std_file();
        if !open_file_identity_matches(&this_parent, &file_parent) {
            return Err(invalid_recovery_path(
                "private retirement file belongs to a different directory capability",
            ));
        }
        file.attest_visible_identity_allow_links()?;
        let visible = open_regular_file_at_allow_links(self.chain.leaf(), &file.name, false)?;
        if !open_file_identity_matches(&file.file, &visible) {
            return Err(invalid_recovery_path(
                "private retirement name changed before removal",
            ));
        }
        let name = file.name.clone();
        after_final_identity_check();

        #[cfg(windows)]
        {
            // DuplicateHandle clones share one kernel file object, so closing
            // only a Rust `File::try_clone` does not close the POSIX-delete
            // file object while the original capability is retained. Exact
            // retirement therefore consumes the capability: close every
            // confirming pathname handle first, set disposition on the sole
            // retained exact file object, then close that object.
            drop(visible);
            delete_file_by_handle(&file.file)?;
            drop(file);

            match self.chain.leaf().symlink_metadata(&name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(invalid_recovery_path(
                        "private retirement name was repopulated during removal",
                    ))
                }
                Err(error) => return Err(error),
            }
            self.sync()
        }

        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            // POSIX exposes no unlink-by-handle primitive. This final
            // descriptor-relative unlink is therefore protected by the
            // caller's retained owner-private transaction lease. We defend
            // every pre-existing and hook-injected identity change by this
            // immediately adjacent re-attestation. A non-cooperating process
            // with the same UID racing after this proof is explicitly outside
            // the internal-state threat boundary; user-selected/public
            // namespaces never use this API.
            file.attest_visible_identity_allow_links()?;
            let final_visible = open_regular_file_at_allow_links(self.chain.leaf(), &name, false)?;
            if !open_file_identity_matches(&file.file, &final_visible) {
                return Err(invalid_recovery_path(
                    "private retirement name changed at the POSIX unlink boundary",
                ));
            }
            self.chain.leaf().remove_file(&name)?;
            drop(final_visible);
            drop(visible);
            self.sync()
        }

        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (visible, name);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "exact private file retirement is unavailable on this platform",
            ));
        }
    }

    /// it, and acquire an independent lock on a different file identity.
    pub fn bind_or_create_private_lease_file(
        &self,
        name: &OsStr,
    ) -> std::io::Result<BoundRecoveryLeaseFile> {
        validate_entry_name(name)?;
        if !self.owner_private_namespace {
            return Err(invalid_recovery_path(
                "private lease requires an owner-private directory capability",
            ));
        }
        self.attest_location()?;
        #[cfg(windows)]
        let create_result =
            crate::overlays::create_owner_only_lease_file(&self.chain.display_path.join(name));
        #[cfg(not(windows))]
        let create_result = self
            .chain
            .leaf()
            .open_with(name, &private_lease_file_open_options(true))
            .map(|file| file.into_std());
        let file = match create_result {
            Ok(file) => file,
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    || cfg!(windows) && error.raw_os_error() == Some(32) =>
            {
                // CREATE_NEW can report ERROR_SHARING_VIOLATION before
                // ERROR_FILE_EXISTS when a retained Windows lease identity
                // deliberately denies delete sharing. Re-open the existing
                // read/write identity and prove it below; the retained
                // no-delete-sharing peer prevents a name swap meanwhile.
                open_private_lease_file_at(self.chain.leaf(), name)?
            }
            Err(error) => return Err(error),
        };
        #[cfg(windows)]
        crate::overlays::attest_owner_only_file_handle(&file)?;
        let lexical = self.chain.leaf().symlink_metadata(name)?;
        if !cap_lexical_regular_file_is_safe(&lexical)
            || !cap_metadata_identity_matches_opened(&lexical, &file)
            || !opened_regular_file_is_safe(&file)
        {
            return Err(invalid_recovery_path(
                "private lease file changed while it was being retained",
            ));
        }
        self.attest_location()?;
        Ok(BoundRecoveryLeaseFile {
            file: BoundRecoveryFile {
                parent_chain: self.chain.try_clone()?,
                name: name.to_os_string(),
                file,
                display_path: self.chain.display_path.join(name),
                expected_digest: Cell::new(None),
            },
        })
    }

    /// Atomically exchange two exact owner-private child directories while
    /// preserving both identities. This is the directory-generation analogue
    /// of the fixed file-slot exchange: callers can build a complete intended
    /// generation in one retained slot, then linearize publication without a
    /// pathname-selected delete. Both names are re-opened and identity-proved
    /// before and after the exchange. A hook may inject a hostile post-proof
    /// swap; every observed directory survives and the post-proof fails.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[doc(hidden)]
    pub fn exchange_exact_private_children_with_hook(
        &self,
        left_name: &OsStr,
        right_parent: &BoundRecoveryDirectory,
        right_name: &OsStr,
        after_pre_exchange_proof: impl FnOnce(),
    ) -> std::io::Result<(BoundRecoveryDirectory, BoundRecoveryDirectory)> {
        validate_entry_name(left_name)?;
        validate_entry_name(right_name)?;
        if !right_parent.owner_private_namespace {
            return Err(invalid_recovery_path(
                "directory-generation exchange requires an owner-private generation-slot parent",
            ));
        }
        self.attest_location()?;
        right_parent.attest_location()?;
        let left = open_directory_at_no_follow(self.chain.leaf(), left_name)?;
        let right = open_directory_at_no_follow(right_parent.chain.leaf(), right_name)?;
        let left_file = left.try_clone()?.into_std_file();
        let right_file = right.try_clone()?.into_std_file();
        let left_identity = bound_file_identity(&left_file)?;
        let right_identity = bound_file_identity(&right_file)?;
        self.attest_location()?;
        right_parent.attest_location()?;
        let left_confirmed = open_directory_at_no_follow(self.chain.leaf(), left_name)?;
        let right_confirmed = open_directory_at_no_follow(right_parent.chain.leaf(), right_name)?;
        if !open_file_identity_matches(&left_file, &left_confirmed.try_clone()?.into_std_file())
            || !open_file_identity_matches(
                &right_file,
                &right_confirmed.try_clone()?.into_std_file(),
            )
        {
            return Err(invalid_recovery_path(
                "directory-generation name changed before atomic exchange",
            ));
        }
        after_pre_exchange_proof();

        exchange_entries_at(
            self.chain.leaf(),
            left_name,
            right_parent.chain.leaf(),
            right_name,
        )?;
        let left_after = open_directory_at_no_follow(self.chain.leaf(), left_name)?;
        let right_after = open_directory_at_no_follow(right_parent.chain.leaf(), right_name)?;
        let left_after_file = left_after.try_clone()?.into_std_file();
        let right_after_file = right_after.try_clone()?.into_std_file();
        if bound_file_identity(&left_after_file)? != right_identity
            || bound_file_identity(&right_after_file)? != left_identity
            || !open_file_identity_matches(&right_file, &left_after_file)
            || !open_file_identity_matches(&left_file, &right_after_file)
        {
            return Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "directory-generation exchange selected a different identity; every observed directory was preserved",
            ));
        }
        self.sync()?;
        right_parent.sync()?;
        let left_chain = self.chain.with_child(left_name, &left_after)?;
        let right_chain = right_parent.chain.with_child(right_name, &right_after)?;
        left_chain.attest()?;
        right_chain.attest()?;
        Ok((
            BoundRecoveryDirectory {
                chain: left_chain,
                owner_private_namespace: self.owner_private_namespace,
            },
            BoundRecoveryDirectory {
                chain: right_chain,
                owner_private_namespace: true,
            },
        ))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    #[doc(hidden)]
    pub fn exchange_exact_private_children_with_hook(
        &self,
        left_name: &OsStr,
        right_parent: &BoundRecoveryDirectory,
        right_name: &OsStr,
        _after_pre_exchange_proof: impl FnOnce(),
    ) -> std::io::Result<(BoundRecoveryDirectory, BoundRecoveryDirectory)> {
        validate_entry_name(left_name)?;
        validate_entry_name(right_name)?;
        self.attest_location()?;
        right_parent.attest_location()?;
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic private directory-generation exchange is unavailable on this platform",
        ))
    }

    /// an immediate descriptor-relative identity recheck before `rmdir`.
    pub fn remove_owned_private_empty_child(
        &self,
        child: BoundRecoveryDirectory,
    ) -> std::io::Result<()> {
        self.remove_owned_private_empty_child_with_hook(child, || {})
    }

    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn attest_exact_directory_file(&self, expected: &File) -> std::io::Result<()> {
        self.attest_location()?;
        let retained = self.chain.leaf().try_clone()?.into_std_file();
        if !open_file_identity_matches(expected, &retained) {
            return Err(invalid_recovery_path(
                "retained recovery directory does not match the expected exact handle",
            ));
        }
        Ok(())
    }

    #[doc(hidden)]
    pub fn remove_owned_private_empty_child_with_hook(
        &self,
        child: BoundRecoveryDirectory,
        before_final_identity_check: impl FnOnce(),
    ) -> std::io::Result<()> {
        if !self.owner_private_namespace || !child.owner_private_namespace {
            return Err(invalid_recovery_path(
                "exact directory retirement requires owner-private capabilities",
            ));
        }
        self.attest_location()?;
        child.attest_location()?;
        if child.chain.display_path.parent() != Some(self.chain.display_path.as_path()) {
            return Err(invalid_recovery_path(
                "private directory retirement requires an immediate child",
            ));
        }
        let (child_name, _) = child.chain.components.last().ok_or_else(|| {
            invalid_recovery_path("private directory retirement child has no retained name")
        })?;
        let child_parent = if child.chain.components.len() == 1 {
            &child.chain.anchor
        } else {
            &child.chain.components[child.chain.components.len() - 2].1
        };
        if !open_file_identity_matches(
            &self.chain.leaf().try_clone()?.into_std_file(),
            &child_parent.try_clone()?.into_std_file(),
        ) {
            return Err(invalid_recovery_path(
                "private directory retirement child belongs to a different parent capability",
            ));
        }
        if child.chain.leaf().entries()?.next().transpose()?.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::DirectoryNotEmpty,
                "private directory retirement requires an empty exact child",
            ));
        }
        let expected_identity =
            bound_file_identity(&child.chain.leaf().try_clone()?.into_std_file())?;
        let child_name = child_name.clone();
        before_final_identity_check();

        #[cfg(windows)]
        {
            // Ordinary child capabilities deny delete sharing to keep their
            // names stable. Drop those clones, then bind a short-lived
            // DELETE-capable handle through the still-retained exact parent.
            drop(child);
            let visible = open_directory_at_for_rename(self.chain.leaf(), &child_name)?;
            let visible_file = visible.try_clone()?.into_std_file();
            if bound_file_identity(&visible_file)? != expected_identity {
                return Err(invalid_recovery_path(
                    "private directory retirement name selected a different identity",
                ));
            }
            if visible.entries()?.next().transpose()?.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "private directory retirement child became non-empty",
                ));
            }
            delete_file_by_handle(&visible_file)?;
            drop(visible_file);
            drop(visible);
        }

        #[cfg(unix)]
        {
            let visible = open_directory_at_no_follow(self.chain.leaf(), &child_name)?;
            let visible_file = visible.try_clone()?.into_std_file();
            if bound_file_identity(&visible_file)? != expected_identity
                || !open_file_identity_matches(
                    &child.chain.leaf().try_clone()?.into_std_file(),
                    &visible_file,
                )
            {
                return Err(invalid_recovery_path(
                    "private directory retirement name selected a different identity",
                ));
            }
            if visible.entries()?.next().transpose()?.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "private directory retirement child became non-empty",
                ));
            }
            // Same internal owner-private boundary as file retirement above:
            // legitimate writers serialize on the retained transaction
            // lease, and this exact proof is immediately adjacent to rmdir.
            // A hostile non-cooperating same-UID mutation after the proof is
            // not representable safely by POSIX and is out of scope.
            let final_visible = open_directory_at_no_follow(self.chain.leaf(), &child_name)?;
            let final_file = final_visible.try_clone()?.into_std_file();
            if bound_file_identity(&final_file)? != expected_identity
                || !open_file_identity_matches(
                    &child.chain.leaf().try_clone()?.into_std_file(),
                    &final_file,
                )
            {
                return Err(invalid_recovery_path(
                    "private directory retirement changed at the POSIX rmdir boundary",
                ));
            }
            if final_visible.entries()?.next().transpose()?.is_some() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::DirectoryNotEmpty,
                    "private directory retirement child became non-empty",
                ));
            }
            self.chain.leaf().remove_dir(&child_name)?;
            self.sync()
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = (child, expected_identity);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "exact private directory retirement is unavailable on this platform",
            ));
        }

        #[cfg(windows)]
        {
            match self.chain.leaf().symlink_metadata(&child_name) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Ok(_) => {
                    return Err(invalid_recovery_path(
                        "private directory retirement name was repopulated",
                    ))
                }
                Err(error) => return Err(error),
            }
            self.sync()
        }
    }

    /// Atomically publish an already-bound staging member under a final name
    /// without replacement. Both names are resolved relative to the retained
    /// directory capability and the returned handle must identify the exact
    /// staging inode after the move.
    pub fn rename_bound_no_replace(
        &self,
        source: BoundRecoveryFile,
        destination_name: &OsStr,
    ) -> std::io::Result<BoundRecoveryFile> {
        self.rename_bound_no_replace_with_hook(source, destination_name, || Ok(()))
    }

    #[doc(hidden)]
    pub(crate) fn rename_bound_no_replace_with_hook(
        &self,
        source: BoundRecoveryFile,
        destination_name: &OsStr,
        after_rename_before_directory_sync: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<BoundRecoveryFile> {
        validate_entry_name(destination_name)?;
        self.attest_location()?;
        if source.display_path.parent() != Some(self.chain.display_path.as_path()) {
            return Err(invalid_recovery_path(
                "recovery staging member is outside its destination capability",
            ));
        }
        source.attest_visible_identity()?;
        move_entry_at_no_replace(
            self.chain.leaf(),
            &source.name,
            &source.file,
            self.chain.leaf(),
            destination_name,
        )?;
        let reopened = open_recovery_file_at(self.chain.leaf(), destination_name)?;
        if !open_file_identity_matches(&source.file, &reopened) {
            return Err(invalid_recovery_path(
                "recovery staging identity changed during final publication",
            ));
        }
        after_rename_before_directory_sync()?;
        self.sync()?;
        Ok(BoundRecoveryFile {
            parent_chain: self.chain.try_clone()?,
            name: destination_name.to_os_string(),
            file: source.file,
            display_path: self.chain.display_path.join(destination_name),
            expected_digest: source.expected_digest,
        })
    }

    pub fn sync(&self) -> std::io::Result<()> {
        sync_bound_namespace_after_write_through_change(&self.chain)
    }
}

impl BoundRecoveryLeaseFile {
    /// Re-open a retained lease with the same no-delete-sharing contract.
    ///
    /// `BoundRecoveryFile::attest_visible_identity` intentionally re-opens
    /// ordinary mutation-capable files with DELETE access and delete sharing.
    /// That is incompatible with the lease invariant on Windows and produces
    /// ERROR_SHARING_VIOLATION against our own retained handle.
    fn attest_visible_identity(&self) -> std::io::Result<()> {
        self.file.parent_chain.attest()?;
        let current = open_private_lease_file_at(self.file.parent_chain.leaf(), &self.file.name)?;
        if !open_file_identity_matches(&self.file.file, &current)
            || !opened_regular_file_is_safe(&self.file.file)
        {
            return Err(invalid_recovery_path(
                "private lease name was replaced while retained",
            ));
        }
        Ok(())
    }

    /// Try to acquire the advisory lock on the exact retained lease identity.
    pub fn try_lock_exclusive(&self) -> std::io::Result<bool> {
        match fs2::FileExt::try_lock_exclusive(&self.file.file) {
            Ok(()) => Ok(true),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || cfg!(windows) && error.raw_os_error() == Some(33) =>
            {
                // LockFileEx reports ERROR_LOCK_VIOLATION on Windows, which
                // Rust currently categorizes as Uncategorized.
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Block until the exact retained lease identity is exclusively locked.
    pub fn lock_exclusive(&self) -> std::io::Result<()> {
        fs2::FileExt::lock_exclusive(&self.file.file)
    }
}

impl BoundRecoveryFile {
    /// Fill an exact, still-visible empty single-link file without ever
    /// selecting or truncating a later pathname winner. This is used for
    /// recyclable public control tombstones whose replacement must preserve
    /// arbitrary same-path user bytes.
    pub fn fill_exact_empty_visible(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.attest_visible_identity()?;
        if !opened_regular_file_is_safe(&self.file) || self.file.metadata()?.len() != 0 {
            return Err(invalid_recovery_path(
                "exact control tombstone is not an empty single-link file",
            ));
        }
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(bytes)?;
        self.file.sync_all()?;
        self.attest_visible_identity()?;
        let proof = self.recovery_proof_for_exact_bytes_bounded_with_hook(
            bytes,
            u64::try_from(bytes.len())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            || Ok(()),
        )?;
        self.attest_recovery_proof_bounded(
            &proof,
            u64::try_from(bytes.len())
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
        )
    }

    /// Clone the already-bound exact file capability without reopening its
    /// ambient pathname. Callers must retain this `BoundRecoveryFile` and
    /// re-attest visible identity when pathname coherence also matters.
    pub(crate) fn try_clone_exact_file(&self) -> std::io::Result<File> {
        self.file.try_clone()
    }

    /// Hold a process-scoped advisory lease on this exact retained file.
    /// The lock is released automatically with the capability.
    pub fn try_lock_exclusive(&self) -> std::io::Result<bool> {
        match fs2::FileExt::try_lock_exclusive(&self.file) {
            Ok(()) => Ok(true),
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || cfg!(windows) && error.raw_os_error() == Some(33) =>
            {
                Ok(false)
            }
            Err(error) => Err(error),
        }
    }

    /// Block until this exact owner-private control file holds the exclusive
    /// cross-thread/process lease. The lease is released when this retained
    /// capability is dropped.
    pub fn lock_exclusive(&self) -> std::io::Result<()> {
        fs2::FileExt::lock_exclusive(&self.file)
    }

    /// Bind every parent component and the final file no-follow. Unix files
    /// with more than one hard link and Windows reparse/multi-link files are
    /// rejected before a destination can be created.
    pub fn bind(path: &Path) -> std::io::Result<Self> {
        let normalized = absolute_normal_path(path)?;
        let parent_path = normalized
            .parent()
            .ok_or_else(|| invalid_recovery_path("recovery source has no parent"))?;
        let name = normalized
            .file_name()
            .ok_or_else(|| invalid_recovery_path("recovery source has no filename"))?
            .to_os_string();
        let parent_chain = walk_absolute_directory(parent_path, false)?;
        let file = open_recovery_file_at(parent_chain.leaf(), &name)?;
        let display_path = parent_chain.display_path.join(&name);
        Ok(Self {
            parent_chain,
            name,
            file,
            display_path,
            expected_digest: Cell::new(None),
        })
    }

    pub fn len(&self) -> std::io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    pub fn is_empty(&self) -> std::io::Result<bool> {
        Ok(self.len()? == 0)
    }

    pub fn display_path(&self) -> &Path {
        &self.display_path
    }

    /// Capture the exact named identity and bytes currently held by this
    /// capability. This proof is intentionally opaque outside core.
    /// Capture the exact visible identity and content through a streaming
    /// byte/deadline ceiling. This is the proof-creation counterpart to
    /// `attest_recovery_proof_bounded`: callers can safely charge the opaque
    /// proof's `byte_len` before carrying it across a mutation boundary.
    pub fn recovery_proof_bounded(
        &self,
        max_bytes: u64,
        deadline: std::time::Instant,
    ) -> std::io::Result<RecoveryFileProof> {
        self.recovery_proof_bounded_with_hook(max_bytes, deadline, || Ok(()))
    }

    /// Capture a bounded proof for an existing public generation without
    /// requiring `nlink == 1`. Job records may legitimately have a retained
    /// hard-link alias; immutable exchange and pathname retirement never
    /// rewrite that inode, so preserving the alias is safe and intentional.
    #[doc(hidden)]
    pub fn recovery_proof_bounded_with_hook(
        &self,
        max_bytes: u64,
        deadline: std::time::Instant,
        after_initial_attestation: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<RecoveryFileProof> {
        self.attest_visible_identity_without_digest()?;
        if self.file.metadata()?.len() > max_bytes {
            return Err(invalid_recovery_path(
                "recovery file exceeds its proof byte budget",
            ));
        }
        after_initial_attestation()?;
        let (bytes, digest) = hash_exact_recovery_file_bounded(&self.file, max_bytes, deadline)?;
        let proof = RecoveryFileProof {
            identity: bound_file_identity(&self.file)?,
            bytes,
            digest,
        };
        self.attest_recovery_proof_bounded(&proof, max_bytes, deadline)?;
        Ok(proof)
    }

    /// Bind this exact visible file to caller-authorized bytes. The returned
    /// proof remains opaque outside core, but can be carried across an exact
    /// descriptor-relative move and re-attested before destructive retirement.
    /// Bounded form of `recovery_proof_for_exact_bytes`. No unbounded digest
    /// is computed before the caller's byte and deadline ceilings apply.
    pub fn recovery_proof_for_exact_bytes_bounded(
        &self,
        expected: &[u8],
        max_bytes: u64,
        deadline: std::time::Instant,
    ) -> std::io::Result<RecoveryFileProof> {
        self.recovery_proof_for_exact_bytes_bounded_with_hook(expected, max_bytes, deadline, || {
            Ok(())
        })
    }

    /// Test seam for changing an exact-byte candidate after its initial
    /// bounded identity/length check and before its streaming proof.
    #[doc(hidden)]
    pub fn recovery_proof_for_exact_bytes_bounded_with_hook(
        &self,
        expected: &[u8],
        max_bytes: u64,
        deadline: std::time::Instant,
        after_initial_attestation: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<RecoveryFileProof> {
        let expected_len = u64::try_from(expected.len())
            .map_err(|_| invalid_recovery_path("expected recovery bytes are too large"))?;
        if expected_len > max_bytes {
            return Err(invalid_recovery_path(
                "expected recovery bytes exceed the proof byte budget",
            ));
        }
        let proof =
            self.recovery_proof_bounded_with_hook(max_bytes, deadline, after_initial_attestation)?;
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "capability-bound file exceeded its attestation deadline",
            ));
        }
        let expected_digest: [u8; 32] = Sha256::digest(expected).into();
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "capability-bound file exceeded its attestation deadline",
            ));
        }
        if proof.bytes != expected_len || proof.digest != expected_digest {
            return Err(invalid_recovery_path(
                "recovery file does not match the authorized exact bytes",
            ));
        }
        self.attest_recovery_proof_bounded(&proof, max_bytes, deadline)?;
        Ok(proof)
    }

    /// Zero the exact app-created file and return the still-retained
    /// capability with its prior content expectation cleared. This permits a
    /// subsequent descriptor-relative move into a retired namespace without
    /// reopening or deleting an ambient pathname.
    pub(crate) fn zero_exact_for_retirement(self) -> std::io::Result<Self> {
        self.attest_visible_identity()?;
        self.file.set_len(0)?;
        self.file.sync_all()?;
        self.expected_digest.set(None);
        Ok(self)
    }

    /// Budgeted form used by policy-state scans. The caller precharges the
    /// stored length against one aggregate budget; this method enforces the
    /// per-file limit and checks the deadline inside the streaming hash loop.
    pub fn attest_recovery_proof_bounded(
        &self,
        expected: &RecoveryFileProof,
        max_bytes: u64,
        deadline: std::time::Instant,
    ) -> std::io::Result<()> {
        self.attest_recovery_proof_bounded_with_hook(expected, max_bytes, deadline, || Ok(()))
    }

    /// Test seam for changing the visible leaf after its bounded content proof
    /// but before the final no-follow identity, link-count, and length proof.
    #[doc(hidden)]
    pub fn attest_recovery_proof_bounded_with_hook(
        &self,
        expected: &RecoveryFileProof,
        max_bytes: u64,
        deadline: std::time::Instant,
        after_content_proof: impl FnOnce() -> std::io::Result<()>,
    ) -> std::io::Result<()> {
        self.attest_visible_identity_without_digest()?;
        let metadata = self.file.metadata()?;
        if metadata.len() != expected.bytes || metadata.len() > max_bytes {
            return Err(invalid_recovery_path(
                "recovery file length exceeds or differs from its proof budget",
            ));
        }
        let (bytes, digest) = hash_exact_recovery_file_bounded(&self.file, max_bytes, deadline)?;
        let current = RecoveryFileProof {
            identity: bound_file_identity(&self.file)?,
            bytes,
            digest,
        };
        if &current != expected {
            return Err(invalid_recovery_path(
                "recovery file no longer matches its bounded ledger proof",
            ));
        }
        after_content_proof()?;
        self.attest_visible_identity_without_digest()?;
        if !opened_regular_file_is_safe(&self.file) {
            return Err(invalid_recovery_path(
                "recovery file is no longer a single-link regular file",
            ));
        }
        let final_metadata = self.file.metadata()?;
        if final_metadata.len() != expected.bytes || final_metadata.len() > max_bytes {
            return Err(invalid_recovery_path(
                "recovery file length exceeds or differs from its proof budget",
            ));
        }
        let (final_bytes, final_digest) =
            hash_exact_recovery_file_bounded(&self.file, max_bytes, deadline)?;
        let final_proof = RecoveryFileProof {
            identity: bound_file_identity(&self.file)?,
            bytes: final_bytes,
            digest: final_digest,
        };
        if &final_proof != expected {
            return Err(invalid_recovery_path(
                "recovery file content changed after its bounded ledger proof",
            ));
        }
        self.attest_visible_identity_without_digest()?;
        if !opened_regular_file_is_safe(&self.file) {
            return Err(invalid_recovery_path(
                "recovery file is no longer a single-link regular file",
            ));
        }
        if self.file.metadata()?.len() != expected.bytes {
            return Err(invalid_recovery_path(
                "recovery file length changed after its final bounded content proof",
            ));
        }
        if std::time::Instant::now() >= deadline {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "capability-bound file exceeded its attestation deadline",
            ));
        }
        Ok(())
    }

    /// Re-open the retained leaf through its complete bound parent chain and
    /// prove the visible name still identifies this exact single-link file.
    fn attest_visible_identity_without_digest(&self) -> std::io::Result<()> {
        self.parent_chain.attest()?;
        let current = open_recovery_file_at(self.parent_chain.leaf(), &self.name)?;
        if !open_file_identity_matches(&self.file, &current)
            || !opened_regular_file_is_safe(&self.file)
        {
            return Err(invalid_recovery_path(
                "recovery file name was replaced; no cleanup was attempted",
            ));
        }
        Ok(())
    }

    pub(crate) fn attest_visible_identity_allow_links(&self) -> std::io::Result<()> {
        self.parent_chain.attest()?;
        let current =
            open_regular_file_at_allow_links(self.parent_chain.leaf(), &self.name, false)?;
        if !open_file_identity_matches(&self.file, &current) {
            return Err(invalid_recovery_path(
                "recovery file name was replaced; no mutation was attempted",
            ));
        }
        Ok(())
    }

    /// Re-open the retained leaf through its complete bound parent chain and
    /// prove the visible name still identifies this exact single-link file.
    pub fn attest_visible_identity(&self) -> std::io::Result<()> {
        self.attest_visible_identity_without_digest()?;
        if let Some(expected) = self.expected_digest.get() {
            let current = open_recovery_file_at(self.parent_chain.leaf(), &self.name)?;
            if !open_file_identity_matches(&self.file, &current) {
                return Err(invalid_recovery_path(
                    "recovery file name was replaced; no cleanup was attempted",
                ));
            }
            if hash_exact_recovery_file(&current)? != expected {
                return Err(invalid_recovery_path(
                    "recovery file bytes changed; no cleanup was attempted",
                ));
            }
        }
        Ok(())
    }
}

fn cap_read_options() -> CapOpenOptions {
    let mut options = CapOpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    options
}

fn normal_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn read_bounded_bytes_twice(
    file: &mut File,
    expected_len: u64,
    after_first_read: impl FnOnce(),
) -> std::io::Result<Vec<u8>> {
    if expected_len > MAX_BOUND_TEXT_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "policy source exceeded its byte ceiling",
        ));
    }
    let capacity = usize::try_from(expected_len).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "policy source length cannot be represented safely",
        )
    })?;
    let mut bytes = Vec::with_capacity(capacity);
    Read::by_ref(file)
        .take(MAX_BOUND_TEXT_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_BOUND_TEXT_FILE_BYTES || bytes.len() as u64 != expected_len {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source changed while it was read",
        ));
    }

    after_first_read();
    file.seek(SeekFrom::Start(0))?;
    let mut offset = 0usize;
    let mut chunk = [0u8; 16 * 1024];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        let end = offset.checked_add(read).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "policy source exceeded its byte ceiling",
            )
        })?;
        if end > bytes.len() || bytes[offset..end] != chunk[..read] {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "policy source changed while it was read",
            ));
        }
        offset = end;
    }
    if offset != bytes.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source changed while it was read",
        ));
    }
    Ok(bytes)
}

pub fn read_bound_utf8_file(root: &Path, path: &Path) -> std::io::Result<BoundTextSnapshot> {
    read_bound_utf8_file_with_hooks(root, path, || {}, || {})
}

#[doc(hidden)]
pub fn read_bound_utf8_file_with_hooks<BeforeOpen, AfterFirstRead>(
    root: &Path,
    path: &Path,
    before_open: BeforeOpen,
    after_first_read: AfterFirstRead,
) -> std::io::Result<BoundTextSnapshot>
where
    BeforeOpen: FnOnce(),
    AfterFirstRead: FnOnce(),
{
    let canonical_root = root.canonicalize()?;
    let canonical_path = path.canonicalize()?;
    let relative = canonical_path.strip_prefix(&canonical_root).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source escaped its root",
        )
    })?;
    if relative.as_os_str().is_empty() || !normal_relative_path(relative) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source path is invalid",
        ));
    }

    // Keep the entire canonical root chain retained, not merely a directory
    // fd detached from its live pathname. A same-UID root rename/replacement
    // must invalidate the read instead of returning attacker-selected bytes
    // under the original root label.
    let bound_root = BoundRecoveryDirectory::bind_existing(&canonical_root)?;
    let directory = bound_root.chain.leaf().try_clone()?;
    before_open();

    let lexical = directory.symlink_metadata(relative)?;
    if !cap_lexical_regular_file_is_safe(&lexical) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source is not a single-link regular file",
        ));
    }
    let options = cap_read_options();
    let mut file = directory.open_with(relative, &options)?.into_std();
    let opened_before = file.metadata()?;
    if !opened_regular_file_is_safe(&file) || opened_before.len() > MAX_BOUND_TEXT_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source is not a bounded single-link regular file",
        ));
    }

    let bytes = read_bounded_bytes_twice(&mut file, opened_before.len(), after_first_read)?;
    let opened_after = file.metadata()?;
    if !opened_regular_file_is_safe(&file)
        || !metadata_identity_matches(&opened_before, &opened_after)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source changed while it was read",
        ));
    }

    let confirmed_file = directory.open_with(relative, &options)?.into_std();
    let confirmed_metadata = confirmed_file.metadata()?;
    let confirmed_lexical = directory.symlink_metadata(relative)?;
    if !cap_lexical_regular_file_is_safe(&confirmed_lexical)
        || !opened_regular_file_is_safe(&confirmed_file)
        || !metadata_identity_matches(&opened_after, &confirmed_metadata)
        || !open_file_identity_matches(&file, &confirmed_file)
        || directory.canonicalize(relative)? != relative
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "policy source changed while it was read",
        ));
    }

    bound_root.attest_location()?;

    let content = String::from_utf8(bytes).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "policy source is not valid UTF-8",
        )
    })?;

    Ok(BoundTextSnapshot {
        canonical_path,
        content,
        identity: bound_file_identity(&file)?,
    })
}

const LEGACY_POLICY_CACHE_NAMES: &[&str] = &[
    "search.db",
    "search.db-wal",
    "search.db-shm",
    "search.db-journal",
    "graph.db",
    "graph.db-wal",
    "graph.db-shm",
    "graph.db-journal",
];

/// Remove durable search/graph projections created by Minutes versions that
/// predate process-private policy projections.
///
/// Cleanup is capability-bound, serialized across current processes, and
/// nonblocking. Startup callers may warn and continue when another process owns
/// the cleanup lease; answer-time callers fail closed instead of waiting.
/// Every exact single-link file is truncated before unlink so an
/// already-open legacy descriptor cannot retain meeting metadata. Symlinks,
/// hard links, directories, replacement races, and unsafe state roots are
/// refused; callers must not publish a graph/search answer after refusal.
#[doc(hidden)]
pub fn retire_legacy_policy_caches() -> std::io::Result<()> {
    let configured = crate::overlays::correction_state_dir();
    retire_legacy_policy_caches_at(&configured)?;
    let historical = crate::overlays::legacy_default_state_dir();
    if historical != configured {
        retire_legacy_policy_caches_at(&historical)?;
    }
    Ok(())
}

/// Hold the single private SQLite projection resource reservation.
///
/// The lease is owner-private, capability-bound, and nonblocking in
/// production so parallel CLI/MCP processes cannot each amplify the same
/// corpus into independent SQLite/FTS heaps. Its retained handle releases the
/// reservation automatically on drop.
pub(crate) fn acquire_private_projection_lease(
    state_root: &Path,
    wait_for_test_peer: bool,
) -> std::io::Result<BoundRecoveryLeaseFile> {
    let state = BoundRecoveryDirectory::prepare_owner_private(state_root)?;
    let lease =
        state.bind_or_create_private_lease_file(OsStr::new("private-policy-projection.lock"))?;
    if wait_for_test_peer {
        lease.lock_exclusive()?;
    } else if !lease.try_lock_exclusive()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another bounded private policy projection is active",
        ));
    }
    Ok(lease)
}

/// Acquire the one graph/search heap reservation for a canonical corpus.
/// Keeping derivation here prevents configurable correction roots from
/// splitting what must be a single cross-process memory boundary.
pub(crate) fn acquire_private_corpus_projection_lease(
    canonical_corpus_root: &Path,
    wait_for_test_peer: bool,
) -> std::io::Result<BoundRecoveryLeaseFile> {
    acquire_private_projection_lease(
        &canonical_corpus_root.join(".minutes-private-projection"),
        wait_for_test_peer,
    )
}

fn retire_legacy_policy_caches_at(state_root: &Path) -> std::io::Result<()> {
    match fs::symlink_metadata(state_root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }

    let state = BoundRecoveryDirectory::prepare_owner_private(state_root)?;
    let lease =
        state.bind_or_create_private_lease_file(OsStr::new("policy-cache-retirement.lock"))?;
    if !lease.try_lock_exclusive()? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WouldBlock,
            "another process is retiring legacy policy caches",
        ));
    }

    for name in LEGACY_POLICY_CACHE_NAMES {
        let name = OsStr::new(name);
        if !state.entry_exists(name)? {
            continue;
        }
        let file = state.bind_exact_file(name)?.zero_exact_for_retirement()?;
        state.remove_owned_private_file(file)?;
    }
    let legacy_graph_name = OsStr::new("graph");
    if state.entry_exists(legacy_graph_name)? {
        let legacy_graph = state.bind_existing_owner_private_child(legacy_graph_name)?;
        for name in ["index.json", "index.tmp"] {
            let name = OsStr::new(name);
            if !legacy_graph.entry_exists(name)? {
                continue;
            }
            let file = legacy_graph
                .bind_exact_file(name)?
                .zero_exact_for_retirement()?;
            legacy_graph.remove_owned_private_file(file)?;
        }
        state.remove_owned_private_empty_child(legacy_graph)?;
    }
    state.sync()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_policy_caches_are_zeroed_for_open_holders_then_retired() {
        let root = tempfile::TempDir::new().unwrap();
        let state = root.path().join("state");
        drop(BoundRecoveryDirectory::prepare_owner_private(&state).unwrap());
        let search = state.join("search.db");
        let graph_wal = state.join("graph.db-wal");
        let graph_dir = state.join("graph");
        fs::create_dir(&graph_dir).unwrap();
        let graph_index = graph_dir.join("index.json");
        fs::write(&search, b"PRIVATE-LEGACY-SEARCH-CANARY").unwrap();
        fs::write(&graph_wal, b"PRIVATE-LEGACY-GRAPH-CANARY").unwrap();
        fs::write(&graph_index, b"PRIVATE-LEGACY-JSON-GRAPH-CANARY").unwrap();
        let holder = File::open(&search).unwrap();
        let graph_holder = File::open(&graph_index).unwrap();

        retire_legacy_policy_caches_at(&state).unwrap();

        assert!(!search.exists());
        assert!(!graph_wal.exists());
        assert!(!graph_dir.exists());
        assert_eq!(holder.metadata().unwrap().len(), 0);
        assert_eq!(graph_holder.metadata().unwrap().len(), 0);
        assert!(state.join("policy-cache-retirement.lock").exists());
    }

    #[test]
    fn legacy_policy_cache_retirement_never_waits_on_a_peer_lease() {
        let root = tempfile::TempDir::new().unwrap();
        let state_path = root.path().join("state");
        let state = BoundRecoveryDirectory::prepare_owner_private(&state_path).unwrap();
        let lease = state
            .bind_or_create_private_lease_file(OsStr::new("policy-cache-retirement.lock"))
            .unwrap();
        assert!(lease.try_lock_exclusive().unwrap());

        let started = std::time::Instant::now();
        let error = retire_legacy_policy_caches_at(&state_path).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn private_projection_admission_is_nonblocking_and_released_on_drop() {
        let root = tempfile::TempDir::new().unwrap();
        let state = root.path().join("state");
        let first = acquire_private_projection_lease(&state, false).unwrap();

        let started = std::time::Instant::now();
        let error = match acquire_private_projection_lease(&state, false) {
            Ok(_) => panic!("second projection unexpectedly acquired the live lease"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));

        drop(first);
        acquire_private_projection_lease(&state, false).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn legacy_policy_cache_hard_link_refuses_retirement() {
        let root = tempfile::TempDir::new().unwrap();
        let state = root.path().join("state");
        fs::create_dir(&state).unwrap();
        let search = state.join("search.db");
        let alias = root.path().join("outside-alias.db");
        fs::write(&search, b"PRIVATE-LEGACY-CANARY").unwrap();
        fs::hard_link(&search, &alias).unwrap();

        assert!(retire_legacy_policy_caches_at(&state).is_err());
        assert_eq!(fs::read(&alias).unwrap(), b"PRIVATE-LEGACY-CANARY");
        assert!(search.exists());
    }

    #[test]
    fn restricted_override_audit_default_root_child_entry() {
        let Some(record) = std::env::var_os("MINUTES_TEST_DEFAULT_AUDIT_RECORD") else {
            return;
        };
        append_restricted_override_audit(record.to_str().unwrap().as_bytes()).unwrap();
    }

    #[test]
    fn restricted_override_audit_honors_the_shared_minutes_home_root() {
        let root = tempfile::TempDir::new().unwrap();
        let home = root.path().join("home");
        let minutes_home = root.path().join("custom-minutes-home");
        fs::create_dir(&home).unwrap();
        let run_child = |home: &Path, minutes_home: &std::ffi::OsStr, record: &str| {
            let status = crate::engine_process::command(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "policy_fs::tests::restricted_override_audit_default_root_child_entry",
                    "--nocapture",
                ])
                .env("HOME", home)
                .env("MINUTES_HOME", minutes_home)
                .env("MINUTES_TEST_DEFAULT_AUDIT_RECORD", record)
                .status()
                .unwrap();
            assert!(status.success());
        };
        run_child(
            &home,
            minutes_home.as_os_str(),
            "{\"v\":1,\"custom_root\":true}\n",
        );

        assert_eq!(
            fs::read(
                minutes_home
                    .join("audit")
                    .join("sensitivity-overrides.jsonl")
            )
            .unwrap(),
            b"{\"v\":1,\"custom_root\":true}\n"
        );
        assert!(!home
            .join(".minutes")
            .join("audit")
            .join("sensitivity-overrides.jsonl")
            .exists());

        let packaged_home = root.path().join("packaged-home");
        fs::create_dir(&packaged_home).unwrap();
        run_child(
            &packaged_home,
            std::ffi::OsStr::new("${HOME}/.minutes"),
            "{\"v\":1,\"packaged_root\":true}\n",
        );
        assert_eq!(
            fs::read(packaged_home.join(".minutes/audit/sensitivity-overrides.jsonl")).unwrap(),
            b"{\"v\":1,\"packaged_root\":true}\n"
        );
        assert!(!packaged_home.join("${HOME}").exists());

        let empty_home = root.path().join("empty-home");
        fs::create_dir(&empty_home).unwrap();
        run_child(
            &empty_home,
            std::ffi::OsStr::new(""),
            "{\"v\":1,\"empty_root\":true}\n",
        );
        assert_eq!(
            fs::read(empty_home.join(".minutes/audit/sensitivity-overrides.jsonl")).unwrap(),
            b"{\"v\":1,\"empty_root\":true}\n"
        );
        assert!(!empty_home.join("audit").exists());
    }

    #[test]
    fn restricted_override_audit_serializes_complete_jsonl_records() {
        let root = tempfile::TempDir::new().unwrap();
        let audit_dir = root.path().join("audit");
        let threads = 12;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(threads));
        let mut handles = Vec::new();
        for index in 0..threads {
            let audit_dir = audit_dir.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let record = format!("{{\"v\":1,\"index\":{index}}}\n");
                append_restricted_override_audit_at_with_hook(&audit_dir, record.as_bytes(), || {})
                    .unwrap();
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }

        let bytes = fs::read(audit_dir.join("sensitivity-overrides.jsonl")).unwrap();
        let lines = bytes.split(|byte| *byte == b'\n').collect::<Vec<_>>();
        assert_eq!(lines.len(), threads + 1);
        assert!(lines.last().unwrap().is_empty());
        for line in &lines[..threads] {
            let value: serde_json::Value = serde_json::from_slice(line).unwrap();
            assert_eq!(value["v"], 1);
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(audit_dir.join("sensitivity-overrides.jsonl"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn restricted_override_audit_child_process_entry() {
        let Some(audit_dir) = std::env::var_os("MINUTES_TEST_AUDIT_DIR") else {
            return;
        };
        let record = std::env::var("MINUTES_TEST_AUDIT_RECORD").unwrap();
        append_restricted_override_audit_at_with_hook(
            Path::new(&audit_dir),
            record.as_bytes(),
            || {},
        )
        .unwrap();
    }

    #[test]
    fn restricted_override_audit_serializes_across_processes() {
        let root = tempfile::TempDir::new().unwrap();
        let audit_dir = root.path().join("audit");
        let current_test = std::env::current_exe().unwrap();
        let mut children = Vec::new();
        for index in 0..8 {
            children.push(
                crate::engine_process::command(&current_test)
                    .args([
                        "--exact",
                        "policy_fs::tests::restricted_override_audit_child_process_entry",
                        "--nocapture",
                    ])
                    .env("MINUTES_TEST_AUDIT_DIR", &audit_dir)
                    .env(
                        "MINUTES_TEST_AUDIT_RECORD",
                        format!("{{\"v\":1,\"process\":{index}}}\n"),
                    )
                    .spawn()
                    .unwrap(),
            );
        }
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        let content = fs::read_to_string(audit_dir.join("sensitivity-overrides.jsonl")).unwrap();
        let lines = content.lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 8);
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(value["v"], 1);
        }
    }

    #[test]
    fn restricted_override_audit_rejects_an_incomplete_existing_tail() {
        let root = tempfile::TempDir::new().unwrap();
        let audit_dir = root.path().join("audit");
        let directory = BoundRecoveryDirectory::prepare_owner_private(&audit_dir).unwrap();
        let mut file = directory
            .create_new_exact_file(OsStr::new("sensitivity-overrides.jsonl"))
            .unwrap();
        file.write_all(b"{\"partial\":true}").unwrap();
        file.sync_all().unwrap();
        drop(file);

        let error =
            append_restricted_override_audit_at_with_hook(&audit_dir, b"{\"v\":1}\n", || {})
                .expect_err("a partial peer record must poison authorization");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            fs::read(audit_dir.join("sensitivity-overrides.jsonl")).unwrap(),
            b"{\"partial\":true}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn restricted_override_audit_denies_a_displaced_visible_leaf() {
        let root = tempfile::TempDir::new().unwrap();
        let audit_dir = root.path().join("audit");
        let visible = audit_dir.join("sensitivity-overrides.jsonl");
        let displaced = audit_dir.join("displaced.jsonl");

        let error =
            append_restricted_override_audit_at_with_hook(&audit_dir, b"{\"v\":1}\n", || {
                fs::rename(&visible, &displaced).unwrap();
                fs::write(&visible, b"{\"attacker\":true}\n").unwrap();
            })
            .expect_err("authorization must fail if the durable record is no longer reachable");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(displaced).unwrap(), b"{\"v\":1}\n");
        assert_eq!(fs::read(visible).unwrap(), b"{\"attacker\":true}\n");
    }

    #[test]
    fn namespace_durability_policy_matches_host_and_exact_windows_moves_are_write_through() {
        #[cfg(not(windows))]
        assert_eq!(
            namespace_durability_policy(),
            NamespaceDurabilityPolicy::DirectoryFsync
        );
        #[cfg(windows)]
        assert_eq!(
            namespace_durability_policy(),
            NamespaceDurabilityPolicy::WriteThroughExactHandles
        );
        assert_ne!(
            windows_exact_rename_file_flags() & WINDOWS_FILE_FLAG_WRITE_THROUGH,
            0,
            "every exact file handle used by Windows rename must request write-through metadata"
        );

        let root = tempfile::TempDir::new().unwrap();
        let directory =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        directory
            .sync()
            .expect("the host durability helper must accept an attested directory capability");
    }

    #[test]
    fn bounded_recovery_proof_reports_exact_length_and_rejects_growth() {
        let root = tempfile::TempDir::new().unwrap();
        let stable_path = root.path().join("stable-context");
        fs::write(&stable_path, b"STABLE").unwrap();
        let stable = BoundRecoveryFile::bind(&stable_path).unwrap();
        let proof = stable
            .recovery_proof_bounded(
                16,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();
        assert_eq!(proof.byte_len(), 6);
        stable
            .attest_recovery_proof_bounded(
                &proof,
                16,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();

        let growing_path = root.path().join("growing-context");
        fs::write(&growing_path, b"SMALL").unwrap();
        let growing = BoundRecoveryFile::bind(&growing_path).unwrap();
        let error = growing
            .recovery_proof_bounded_with_hook(
                16,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                || {
                    OpenOptions::new()
                        .write(true)
                        .open(&growing_path)?
                        .set_len(17)
                },
            )
            .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::metadata(&growing_path).unwrap().len(), 17);
    }

    #[test]
    fn bounded_recovery_attestation_rejects_post_content_leaf_replacement() {
        let root = tempfile::TempDir::new().unwrap();
        let source_path = root.path().join("bounded-source");
        let displaced_path = root.path().join("bounded-source-displaced");
        fs::write(&source_path, b"AUTHORIZED").unwrap();
        let source = BoundRecoveryFile::bind(&source_path).unwrap();
        let proof = source
            .recovery_proof_bounded(
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();

        let error = source
            .attest_recovery_proof_bounded_with_hook(
                &proof,
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                || {
                    fs::rename(&source_path, &displaced_path)?;
                    fs::write(&source_path, b"REPLACEMENT")
                },
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&source_path).unwrap(), b"REPLACEMENT");
        assert_eq!(fs::read(&displaced_path).unwrap(), b"AUTHORIZED");
    }

    #[test]
    fn bounded_recovery_attestation_rejects_post_content_hard_link() {
        let root = tempfile::TempDir::new().unwrap();
        let source_path = root.path().join("bounded-source");
        let alias_path = root.path().join("bounded-source-alias");
        fs::write(&source_path, b"AUTHORIZED").unwrap();
        let source = BoundRecoveryFile::bind(&source_path).unwrap();
        let proof = source
            .recovery_proof_bounded(
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();

        let error = source
            .attest_recovery_proof_bounded_with_hook(
                &proof,
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                || fs::hard_link(&source_path, &alias_path),
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&source_path).unwrap(), b"AUTHORIZED");
        assert_eq!(fs::read(&alias_path).unwrap(), b"AUTHORIZED");
    }

    #[test]
    fn bounded_recovery_attestation_rejects_post_content_length_change() {
        let root = tempfile::TempDir::new().unwrap();
        let source_path = root.path().join("bounded-source");
        fs::write(&source_path, b"AUTHORIZED").unwrap();
        let source = BoundRecoveryFile::bind(&source_path).unwrap();
        let proof = source
            .recovery_proof_bounded(
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();

        let error = source
            .attest_recovery_proof_bounded_with_hook(
                &proof,
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                || {
                    OpenOptions::new()
                        .write(true)
                        .open(&source_path)?
                        .set_len(11)
                },
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::metadata(&source_path).unwrap().len(), 11);
    }

    #[test]
    fn bounded_recovery_attestation_rejects_post_content_same_length_rewrite() {
        let root = tempfile::TempDir::new().unwrap();
        let source_path = root.path().join("bounded-source");
        fs::write(&source_path, b"AUTHORIZED").unwrap();
        let source = BoundRecoveryFile::bind(&source_path).unwrap();
        let proof = source
            .recovery_proof_bounded(
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
            )
            .unwrap();

        let error = source
            .attest_recovery_proof_bounded_with_hook(
                &proof,
                32,
                std::time::Instant::now() + std::time::Duration::from_secs(1),
                || fs::write(&source_path, b"UNEXPECTED"),
            )
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(fs::read(&source_path).unwrap(), b"UNEXPECTED");
    }

    #[test]
    fn windows_canonical_wire_normalizes_only_extended_drive_and_unc_paths() {
        assert_eq!(
            normalize_windows_canonical_path_wire(r"\\?\C:\Users\test\meeting.md"),
            Some(r"C:\Users\test\meeting.md".to_string())
        );
        assert_eq!(
            normalize_windows_canonical_path_wire(r"\\?\UNC\server\share\meeting.md"),
            Some(r"\\server\share\meeting.md".to_string())
        );
        assert_eq!(
            normalize_windows_canonical_path_wire(r"\\?\GLOBALROOT\Device\x"),
            None
        );
        assert_eq!(normalize_windows_canonical_path_wire("meeting.md"), None);
    }

    #[cfg(windows)]
    #[test]
    fn actual_windows_canonical_path_uses_node_compatible_wire_spelling() {
        let root = tempfile::TempDir::new().unwrap();
        let source = root.path().join("meeting.md");
        fs::write(&source, "WINDOWS_WIRE_CANARY").unwrap();
        let canonical = source.canonicalize().unwrap();
        let wire = canonical_path_wire(&canonical);

        assert!(!wire.starts_with(r"\\?\"));
        assert!(Path::new(&wire).is_absolute());
        assert_eq!(fs::read_to_string(&wire).unwrap(), "WINDOWS_WIRE_CANARY");
    }

    #[test]
    fn atomic_move_never_clobbers_an_existing_private_destination() {
        let root = tempfile::TempDir::new().unwrap();
        let source = root.path().join("source.md");
        let destination = root.path().join("destination.md");
        fs::write(&source, "SOURCE_CANARY").unwrap();
        fs::write(&destination, "DESTINATION_CANARY").unwrap();

        assert!(move_entry_no_replace(&source, &destination).is_err());
        assert_eq!(fs::read_to_string(&source).unwrap(), "SOURCE_CANARY");
        assert_eq!(
            fs::read_to_string(&destination).unwrap(),
            "DESTINATION_CANARY"
        );

        fs::remove_file(&destination).unwrap();
        move_entry_no_replace(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read_to_string(destination).unwrap(), "SOURCE_CANARY");
    }

    #[test]
    fn bound_text_read_rejects_external_hard_link_capabilities() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("meetings");
        let outside = temp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let meeting = root.join("meeting.md");
        let alias = outside.join("meeting-alias.md");
        fs::write(&meeting, "SAFE_BYTES").unwrap();
        fs::hard_link(&meeting, &alias).unwrap();

        let error = read_bound_utf8_file(&root, &meeting)
            .expect_err("a source writable through an unwatched alias must be denied");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        fs::write(&alias, "OUTSIDE_ALIAS_CANARY").unwrap();
        assert_eq!(
            fs::read_to_string(&meeting).unwrap(),
            "OUTSIDE_ALIAS_CANARY"
        );
    }

    #[test]
    fn bound_text_read_rejects_oversized_source_before_allocating_content() {
        let root = tempfile::TempDir::new().unwrap();
        let meeting = root.path().join("oversized.md");
        let file = File::create(&meeting).unwrap();
        file.set_len(MAX_BOUND_TEXT_FILE_BYTES + 1).unwrap();

        assert!(read_bound_utf8_file(root.path(), &meeting).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_relative_directory_open_rejects_internal_symlink_swap() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let outside = temp.path().join("outside");
        let nested = root.join("nested");
        let displaced = root.join("displaced");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let root_dir = open_directory_no_follow(&root).unwrap();

        let result = open_directory_at_no_follow_with_hook(
            &root_dir,
            std::ffi::OsStr::new("nested"),
            || {
                fs::rename(&nested, &displaced).unwrap();
                symlink(&outside, &nested).unwrap();
            },
        );

        assert!(result.is_err());
        assert!(nested.symlink_metadata().unwrap().file_type().is_symlink());
        assert!(displaced.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_relative_directory_open_rejects_real_directory_replacement() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        let replacement = root.join("replacement");
        let displaced = root.join("displaced");
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(&replacement).unwrap();
        fs::write(nested.join("identity"), "ORIGINAL_DIRECTORY").unwrap();
        fs::write(replacement.join("identity"), "REPLACEMENT_DIRECTORY").unwrap();
        let root_dir = open_directory_no_follow(&root).unwrap();

        let result = open_directory_at_no_follow_with_hook(
            &root_dir,
            std::ffi::OsStr::new("nested"),
            || {
                fs::rename(&nested, &displaced).unwrap();
                fs::rename(&replacement, &nested).unwrap();
            },
        );

        assert!(result.is_err());
        assert_eq!(
            fs::read_to_string(displaced.join("identity")).unwrap(),
            "ORIGINAL_DIRECTORY"
        );
        assert_eq!(
            fs::read_to_string(nested.join("identity")).unwrap(),
            "REPLACEMENT_DIRECTORY"
        );
    }

    #[cfg(unix)]
    #[test]
    fn descriptor_relative_directory_open_allows_same_inode_namespace_bookkeeping_change() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let nested = root.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let root_dir = open_directory_no_follow(&root).unwrap();

        let opened = open_directory_at_no_follow_with_hook(
            &root_dir,
            std::ffi::OsStr::new("nested"),
            || fs::write(nested.join("new-child"), b"namespace bookkeeping").unwrap(),
        )
        .expect("a child entry must not change the retained directory identity");

        assert!(opened.symlink_metadata("new-child").unwrap().is_file());
    }

    #[cfg(unix)]
    #[test]
    fn parent_swap_before_open_cannot_escape_bound_root() {
        use std::os::unix::fs::symlink;

        let root = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let parent = root.path().join("nested");
        let displaced = root.path().join("displaced");
        fs::create_dir(&parent).unwrap();
        let meeting = parent.join("meeting.md");
        fs::write(&meeting, "SAFE_BYTES").unwrap();
        fs::write(outside.path().join("meeting.md"), "OUTSIDE_CANARY").unwrap();

        let result = read_bound_utf8_file_with_hooks(
            root.path(),
            &meeting,
            || {
                fs::rename(&parent, &displaced).unwrap();
                symlink(outside.path(), &parent).unwrap();
            },
            || {},
        );
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn parent_swap_and_restore_after_open_keeps_outside_bytes_inert() {
        use std::os::unix::fs::symlink;

        let root = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        let parent = root.path().join("nested");
        let displaced = root.path().join("displaced");
        fs::create_dir(&parent).unwrap();
        let meeting = parent.join("meeting.md");
        fs::write(&meeting, "SAFE_BYTES").unwrap();
        fs::write(outside.path().join("meeting.md"), "OUTSIDE_CANARY").unwrap();

        let snapshot = read_bound_utf8_file_with_hooks(
            root.path(),
            &meeting,
            || {},
            || {
                fs::rename(&parent, &displaced).unwrap();
                symlink(outside.path(), &parent).unwrap();
                fs::remove_file(&parent).unwrap();
                fs::rename(&displaced, &parent).unwrap();
            },
        )
        .unwrap();
        assert_eq!(snapshot.content, "SAFE_BYTES");
        assert!(!snapshot.content.contains("OUTSIDE_CANARY"));
    }

    #[cfg(unix)]
    #[test]
    fn root_replacement_after_read_invalidates_bound_text_snapshot() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("root");
        let displaced = temp.path().join("displaced");
        let replacement = temp.path().join("replacement");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&replacement).unwrap();
        let meeting = root.join("meeting.md");
        fs::write(&meeting, "SAFE_BYTES").unwrap();
        fs::write(replacement.join("meeting.md"), "ATTACKER_BYTES").unwrap();

        let error = read_bound_utf8_file_with_hooks(
            &root,
            &meeting,
            || {},
            || {
                fs::rename(&root, &displaced).unwrap();
                fs::rename(&replacement, &root).unwrap();
            },
        )
        .expect_err("a displaced root capability must invalidate the snapshot");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(
            fs::read_to_string(displaced.join("meeting.md")).unwrap(),
            "SAFE_BYTES"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_parent_reparse_swap_never_returns_outside_bytes() {
        use std::sync::{Arc, Mutex};

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        enum SwapOutcome {
            NotAttempted,
            RenameBlocked,
            JunctionInstalled,
        }

        fn create_junction(link: &Path, target: &Path) -> std::io::Result<()> {
            let output = crate::engine_process::command("cmd")
                .args(["/D", "/C", "mklink", "/J"])
                .arg(link)
                .arg(target)
                .output()?;
            if output.status.success() {
                Ok(())
            } else {
                Err(std::io::Error::other(format!(
                    "mklink /J failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                )))
            }
        }

        let root = tempfile::TempDir::new().unwrap();
        let outside = tempfile::TempDir::new().unwrap();
        // Prove this host can create a real directory junction before the
        // adversarial read. A setup/privilege failure must fail the test; it
        // cannot masquerade as successful policy denial.
        let preflight = root.path().join("junction-preflight");
        create_junction(&preflight, outside.path()).expect("create preflight directory junction");
        assert_eq!(
            preflight
                .canonicalize()
                .expect("canonical preflight junction"),
            outside
                .path()
                .canonicalize()
                .expect("canonical junction target")
        );
        fs::remove_dir(&preflight).expect("remove preflight directory junction");

        let parent = root.path().join("nested");
        let displaced = root.path().join("displaced");
        fs::create_dir(&parent).unwrap();
        let meeting = parent.join("meeting.md");
        fs::write(&meeting, "SAFE_BYTES").unwrap();
        fs::write(outside.path().join("meeting.md"), "WINDOWS_OUTSIDE_CANARY").unwrap();

        let baseline = read_bound_utf8_file(root.path(), &meeting).expect("baseline bound read");
        assert_eq!(baseline.content, "SAFE_BYTES");

        let swap_outcome = Arc::new(Mutex::new(SwapOutcome::NotAttempted));
        let hook_outcome = swap_outcome.clone();

        let result = read_bound_utf8_file_with_hooks(
            root.path(),
            &meeting,
            || {},
            || {
                // Windows commonly refuses the rename because the capability
                // keeps this parent open. That is a distinct accepted branch,
                // and the read still has to complete with the exact safe bytes.
                if fs::rename(&parent, &displaced).is_err() {
                    *hook_outcome.lock().unwrap() = SwapOutcome::RenameBlocked;
                    return;
                }
                create_junction(&parent, outside.path())
                    .expect("install adversarial directory junction after successful rename");
                assert_eq!(
                    parent
                        .canonicalize()
                        .expect("canonical adversarial junction"),
                    outside
                        .path()
                        .canonicalize()
                        .expect("canonical outside target")
                );
                *hook_outcome.lock().unwrap() = SwapOutcome::JunctionInstalled;
                fs::remove_dir(&parent).expect("remove adversarial directory junction");
                fs::rename(&displaced, &parent).expect("restore original parent directory");
            },
        );

        match *swap_outcome.lock().unwrap() {
            SwapOutcome::NotAttempted => panic!("adversarial swap hook did not execute"),
            SwapOutcome::RenameBlocked => {
                let snapshot = result.expect("rename-blocked read must remain available");
                assert_eq!(snapshot.content, "SAFE_BYTES");
            }
            SwapOutcome::JunctionInstalled => match result {
                Ok(snapshot) => assert_eq!(snapshot.content, "SAFE_BYTES"),
                Err(error) => assert_eq!(
                    error.kind(),
                    std::io::ErrorKind::PermissionDenied,
                    "only an explicit policy denial may satisfy the swap branch"
                ),
            },
        };
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_artifact_wrappers_apply_and_reattest_owner_only_dacls() {
        let root = tempfile::TempDir::new().unwrap();
        let private_dir = root.path().join("restricted-artifacts");
        ensure_owner_only_directory(&private_dir)
            .expect("create and attest owner-only private directory");

        let private_file = private_dir.join("artifact.md");
        write_owner_only_new_file(&private_file, b"PRIVATE_DACL_CANARY")
            .expect("create owner-only private file before writing bytes");
        ensure_owner_only_directory(&private_dir).expect("reattest owner-only private directory");
        ensure_owner_only_file(&private_file).expect("reattest owner-only private file");
        assert_eq!(fs::read(&private_file).unwrap(), b"PRIVATE_DACL_CANARY");

        let duplicate = write_owner_only_new_file(&private_file, b"CLOBBER_CANARY")
            .expect_err("private publication must never replace an existing file");
        assert_eq!(duplicate.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&private_file).unwrap(), b"PRIVATE_DACL_CANARY");
    }

    #[cfg(windows)]
    #[test]
    fn windows_owner_private_boundary_rejects_preexisting_unprotected_objects() {
        let root = tempfile::TempDir::new().unwrap();
        let planted_directory = root.path().join("planted-directory");
        fs::create_dir(&planted_directory).unwrap();
        let directory_error =
            match BoundRecoveryDirectory::prepare_owner_private(&planted_directory) {
                Ok(_) => panic!("an existing directory must already have the exact private DACL"),
                Err(error) => error,
            };
        assert!(
            directory_error.to_string().contains("DACL"),
            "unexpected directory denial: {directory_error}"
        );

        let private_directory = root.path().join("private-directory");
        let boundary = BoundRecoveryDirectory::prepare_owner_private(&private_directory)
            .expect("create an atomically owner-private directory");
        let planted_file = private_directory.join("planted-control.json");
        File::create(&planted_file).unwrap();
        let file_error = match boundary
            .bind_owner_private_exact_file(OsStr::new("planted-control.json"))
        {
            Ok(_) => panic!("an existing control leaf must already have the exact private DACL"),
            Err(error) => error,
        };
        assert!(
            file_error.to_string().contains("DACL"),
            "unexpected file denial: {file_error}"
        );
        assert_eq!(fs::metadata(&planted_file).unwrap().len(), 0);

        fs::remove_file(&planted_file).unwrap();
        let created = boundary
            .create_new_exact_file(OsStr::new("created-control.json"))
            .expect("new control leaves receive the canonical private DACL");
        drop(created);
        boundary
            .bind_owner_private_exact_file(OsStr::new("created-control.json"))
            .expect("the canonical private leaf remains bindable");
    }

    #[cfg(unix)]
    #[test]
    fn owner_only_new_file_never_reopens_a_swapped_parent_path() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::TempDir::new().unwrap();
        let parent = root.path().join("checkpoints");
        let displaced = root.path().join("checkpoints-displaced");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let destination = parent.join("state.json");

        let result =
            write_owner_only_new_file_with_hook(&destination, b"PRIVATE_CHECKPOINT_CANARY", || {
                fs::rename(&parent, &displaced).unwrap();
                fs::create_dir(&parent).unwrap();
                fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
            });

        assert!(
            !destination.exists(),
            "the rebound ambient parent must never receive checkpoint bytes"
        );
        match result {
            Ok(()) => assert_eq!(
                fs::read(displaced.join("state.json")).unwrap(),
                b"PRIVATE_CHECKPOINT_CANARY"
            ),
            Err(_) => assert!(
                !displaced.join("state.json").exists()
                    || fs::read(displaced.join("state.json")).unwrap()
                        == b"PRIVATE_CHECKPOINT_CANARY",
                "a failed capability-bound write may not publish different bytes"
            ),
        }
    }

    #[test]
    fn macos_system_root_aliases_expand_without_resolving_later_components() {
        assert_eq!(
            normalize_macos_system_root_alias(Path::new("/var/folders/example/.tmp")),
            PathBuf::from("/private/var/folders/example/.tmp")
        );
        assert_eq!(
            normalize_macos_system_root_alias(Path::new("/tmp/minutes")),
            PathBuf::from("/private/tmp/minutes")
        );
        assert_eq!(
            normalize_macos_system_root_alias(Path::new("/Users/example/var/file")),
            PathBuf::from("/Users/example/var/file")
        );
        assert_eq!(
            normalize_macos_system_root_alias(Path::new("/variable/example")),
            PathBuf::from("/variable/example")
        );
        assert_eq!(
            normalize_macos_system_root_alias(Path::new("/private/var/folders/example")),
            PathBuf::from("/private/var/folders/example")
        );
    }

    #[test]
    fn owner_private_empty_child_is_retired_through_its_exact_parent() {
        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        let child = parent
            .prepare_owner_private_child(OsStr::new("completed"))
            .unwrap();
        let child_path = child.display_path().to_path_buf();

        parent.remove_owned_private_empty_child(child).unwrap();
        assert!(!child_path.exists());
    }

    #[test]
    fn new_owner_private_child_allocation_never_adopts_an_existing_name() {
        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        let child = parent
            .create_new_owner_private_child(OsStr::new("generation"))
            .unwrap();
        let child_path = child.display_path().to_path_buf();

        let error = match parent.create_new_owner_private_child(OsStr::new("generation")) {
            Ok(_) => panic!("fresh private allocation must reject an existing pathname"),
            Err(error) => error,
        };

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(child.display_path(), child_path);
    }

    #[test]
    fn owner_private_random_control_file_registers_before_create_and_retires_exactly() {
        let root = tempfile::TempDir::new().unwrap();
        let directory =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("control")).unwrap();
        let sibling =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("sibling")).unwrap();
        assert!(directory.is_same_filesystem(&sibling).unwrap());

        let registered = std::cell::RefCell::new(None::<PathBuf>);
        let control = directory
            .create_random_private_control_file_with_hook(
                "policy-fence",
                b"EXACT_CONTROL_CANARY",
                |path| {
                    assert!(
                        !path.exists(),
                        "registration must precede filesystem mutation"
                    );
                    *registered.borrow_mut() = Some(path.to_path_buf());
                    Ok(())
                },
            )
            .unwrap();
        let path = registered.into_inner().unwrap();
        assert_eq!(control.display_path(), path);
        assert_eq!(fs::read(&path).unwrap(), b"EXACT_CONTROL_CANARY");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(directory.display_path())
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }

        let other = directory
            .create_random_private_control_file("policy-fence", b"OTHER_CONTROL_CANARY")
            .unwrap();
        assert_ne!(control.display_path(), other.display_path());
        directory.remove_owned_private_file(control).unwrap();
        assert!(!path.exists());
        assert_eq!(
            fs::read(other.display_path()).unwrap(),
            b"OTHER_CONTROL_CANARY"
        );
        directory.remove_owned_private_file(other).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn owner_private_file_exchange_winner_and_original_both_survive_retirement_refusal() {
        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        let target = parent.display_path().join("obsolete.json");
        let sibling = parent.display_path().join("legitimate-sibling.json");
        let displaced = parent.display_path().join("displaced-obsolete.json");
        fs::write(&target, b"ORIGINAL_PRIVATE_GENERATION").unwrap();
        fs::write(&sibling, b"LEGITIMATE_PRIVATE_SIBLING").unwrap();
        let bound = parent.bind_exact_file(OsStr::new("obsolete.json")).unwrap();

        let error = parent
            .remove_owned_private_file_with_hook(bound, || {
                fs::rename(&target, &displaced).unwrap();
                fs::rename(&sibling, &target).unwrap();
            })
            .expect_err("a post-proof exchange winner must be preserved, never unlinked");

        assert!(matches!(
            error.kind(),
            std::io::ErrorKind::Unsupported
                | std::io::ErrorKind::StorageFull
                | std::io::ErrorKind::PermissionDenied
        ));
        assert_eq!(
            fs::read(&displaced).unwrap(),
            b"ORIGINAL_PRIVATE_GENERATION"
        );
        let preserved = fs::read_dir(parent.display_path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read(entry.path()).ok())
            .collect::<Vec<_>>();
        assert!(
            preserved
                .iter()
                .any(|bytes| bytes == b"LEGITIMATE_PRIVATE_SIBLING"),
            "the racing winner must remain named inside the private namespace"
        );
    }

    #[cfg(unix)]
    #[test]
    fn private_directory_generation_exchange_preserves_post_proof_swap_winner() {
        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        for (name, marker) in [
            ("live", b"LIVE".as_slice()),
            ("intended", b"INTENDED".as_slice()),
            ("winner", b"WINNER".as_slice()),
        ] {
            let child = parent
                .prepare_owner_private_child(OsStr::new(name))
                .unwrap();
            fs::write(child.display_path().join("marker"), marker).unwrap();
        }
        let live = parent.display_path().join("live");
        let displaced = parent.display_path().join("displaced-live");
        let winner = parent.display_path().join("winner");

        let error = parent
            .exchange_exact_private_children_with_hook(
                OsStr::new("live"),
                &parent,
                OsStr::new("intended"),
                || {
                    fs::rename(&live, &displaced).unwrap();
                    fs::rename(&winner, &live).unwrap();
                },
            )
            .err()
            .expect("a post-proof name winner must fail the exchange proof");

        assert_eq!(error.kind(), std::io::ErrorKind::StorageFull);
        let markers = fs::read_dir(parent.display_path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| fs::read(entry.path().join("marker")).ok())
            .collect::<Vec<_>>();
        for expected in [
            b"LIVE".as_slice(),
            b"INTENDED".as_slice(),
            b"WINNER".as_slice(),
        ] {
            assert!(markers.iter().any(|marker| marker == expected));
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_private_lease_handle_blocks_rename_and_recreate_split_brain() {
        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        let lease = parent
            .bind_or_create_private_lease_file(OsStr::new("worker.lock"))
            .unwrap();
        lease.lock_exclusive().unwrap();
        let lock_path = parent.display_path().join("worker.lock");
        let displaced = parent.display_path().join("displaced.lock");

        assert!(
            fs::rename(&lock_path, &displaced).is_err(),
            "the retained lease handle must deny Windows delete sharing"
        );
        assert!(lock_path.is_file());
        assert!(!displaced.exists());

        drop(lease);
        fs::rename(&lock_path, &displaced).unwrap();
        assert!(displaced.is_file());
    }

    #[cfg(unix)]
    #[test]
    fn owner_private_empty_child_replacement_survives_retirement_refusal() {
        use std::os::unix::fs::MetadataExt;

        let root = tempfile::TempDir::new().unwrap();
        let parent =
            BoundRecoveryDirectory::prepare_owner_private(&root.path().join("private")).unwrap();
        let child = parent
            .prepare_owner_private_child(OsStr::new("completed"))
            .unwrap();
        let winner = parent
            .prepare_owner_private_child(OsStr::new("winner"))
            .unwrap();
        drop(winner);
        let completed = parent.display_path().join("completed");
        let displaced = parent.display_path().join("displaced");
        let winner = parent.display_path().join("winner");
        let original_inode = fs::metadata(&completed).unwrap().ino();
        let winner_inode = fs::metadata(&winner).unwrap().ino();

        let error = parent
            .remove_owned_private_empty_child_with_hook(child, || {
                fs::rename(&completed, &displaced).unwrap();
                fs::rename(&winner, &completed).unwrap();
            })
            .expect_err("a replacement directory must never be removed");

        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(
            displaced.is_dir(),
            "the originally bound child is preserved"
        );
        assert_eq!(fs::metadata(&displaced).unwrap().ino(), original_inode);
        let remaining_inodes = fs::read_dir(parent.display_path())
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.metadata().ok())
            .filter(|metadata| metadata.is_dir())
            .map(|metadata| metadata.ino())
            .collect::<Vec<_>>();
        assert!(
            remaining_inodes.contains(&winner_inode),
            "the racing directory winner must survive under a retained private name"
        );
    }
}
