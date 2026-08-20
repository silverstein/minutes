use crate::diarize::{AttributionSource, Confidence, SpeakerAttribution};
use rusqlite::{params, Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use thiserror::Error;

const MAX_OVERLAY_SOURCE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_OVERLAY_DB_BYTES: u64 = 64 * 1024 * 1024;
const MAX_OVERLAY_ROWS: i64 = 10_000;
const MAX_OVERLAY_FIELD_BYTES: i64 = 64 * 1024;
const MAX_OVERLAY_AGGREGATE_FIELD_BYTES: i64 = 16 * 1024 * 1024;
const OVERLAY_READ_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

#[derive(Debug, Error)]
pub enum OverlayError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct SpeakerConfirmation {
    pub meeting_key: String,
    pub speaker_label: String,
    pub name: String,
    pub confidence: Confidence,
    pub source: AttributionSource,
    pub reversible_to: Option<String>,
    pub note: Option<String>,
    pub created_at: String,
    /// Exact source bytes visible when the user confirmed this attribution.
    /// Legacy unbound rows remain inspectable but cannot authorize graph facts.
    pub source_sha256: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct StableSpeakerOverlaySnapshot {
    by_meeting: HashMap<String, Vec<SpeakerConfirmation>>,
    revision: String,
}

impl StableSpeakerOverlaySnapshot {
    pub(crate) fn empty() -> Self {
        Self {
            by_meeting: HashMap::new(),
            revision: empty_overlay_revision(),
        }
    }

    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn confirmations_for_meeting(&self, meeting_path: &Path) -> &[SpeakerConfirmation] {
        self.by_meeting
            .get(&meeting_key(meeting_path))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub(crate) fn confirmations(&self) -> impl Iterator<Item = &SpeakerConfirmation> {
        self.by_meeting.values().flat_map(|items| items.iter())
    }

    pub(crate) fn confirmations_for_source(
        &self,
        meeting_path: &Path,
        source_sha256: &[u8; 32],
    ) -> Vec<SpeakerConfirmation> {
        let expected: String = source_sha256
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.confirmations_for_meeting(meeting_path)
            .iter()
            .filter(|confirmation| confirmation.source_sha256.as_deref() == Some(&expected))
            .cloned()
            .collect()
    }
}

fn hash_field(hasher: &mut Sha256, value: Option<&str>) {
    match value {
        Some(value) => {
            hasher.update([1]);
            hasher.update((value.len() as u64).to_le_bytes());
            hasher.update(value.as_bytes());
        }
        None => hasher.update([0]),
    }
}

fn finish_revision(hasher: Sha256) -> String {
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn empty_overlay_revision() -> String {
    let mut hasher = Sha256::new();
    hasher.update(OVERLAY_REVISION_DOMAIN);
    finish_revision(hasher)
}

const OVERLAY_REVISION_DOMAIN: &[u8] = b"minutes.speaker-overlays.v1\0";

/// The one state root used by correction readers and writers. `MINUTES_HOME`
/// is an explicit override of the complete `.minutes` directory; otherwise
/// preserve the historical `$HOME/.minutes` location.
fn resolved_home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(dirs::home_dir)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

pub(crate) fn legacy_default_state_dir() -> PathBuf {
    resolved_home_dir().join(".minutes")
}

pub(crate) fn correction_state_dir() -> PathBuf {
    let home = resolved_home_dir();
    let Some(configured_os) = std::env::var_os("MINUTES_HOME") else {
        return home.join(".minutes");
    };
    if configured_os.is_empty() {
        return home.join(".minutes");
    }
    if let Some(configured) = configured_os.to_str() {
        for marker in ["~", "$HOME", "${HOME}"] {
            if configured == marker {
                return home;
            }
            if let Some(suffix) = configured.strip_prefix(marker) {
                if suffix.starts_with('/') || suffix.starts_with('\\') {
                    return lexical_normalize_absolute(
                        home.join(suffix.trim_start_matches(['/', '\\'])),
                    );
                }
            }
        }
    }
    let configured = PathBuf::from(configured_os);
    if configured.is_absolute() {
        return lexical_normalize_absolute(configured);
    }
    // A relative override is deterministic across CLI, desktop, SDK and MCP:
    // it is HOME-relative, never process-cwd-relative.
    lexical_normalize_absolute(home.join(configured))
}

fn lexical_normalize_absolute(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                // An absolute state root must never traverse above its prefix.
                if normalized.file_name().is_some() {
                    normalized.pop();
                }
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    normalized
}

/// Database path: `$MINUTES_HOME/overlays.db` or `~/.minutes/overlays.db`.
///
/// This is additive state layered over immutable meeting markdown. Deleting it
/// removes user-confirmed corrections but never damages raw capture files.
pub fn default_db_path() -> PathBuf {
    correction_state_dir().join("overlays.db")
}

pub fn db_path() -> PathBuf {
    default_db_path()
}

/// Resolve an isolated overlay database beside a test graph projection.
///
/// Production graph queries use [`default_db_path`] for durable corrections
/// and build their graph projection in process-private temporary storage.
pub fn db_path_for_graph_path(graph_path: &Path) -> PathBuf {
    graph_path
        .parent()
        .map(|parent| parent.join("overlays.db"))
        .unwrap_or_else(db_path)
}

#[cfg(unix)]
pub(crate) fn secure_private_parent(parent: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(parent)?;
    let metadata = fs::symlink_metadata(parent)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "correction store parent is not a private directory",
        ));
    }
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
pub(crate) fn secure_private_file(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if path.exists() {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(std::io::Error::other(
                "correction store is not a private regular file",
            ));
        }
    } else {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(unix)]
pub(crate) fn secure_private_file_handle(file: &File) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o777 != 0o600 {
        return Err(std::io::Error::other(
            "private file handle is not an owner-only single-link regular file",
        ));
    }
    Ok(())
}

#[cfg(windows)]
mod windows_private {
    use std::ffi::c_void;
    use std::fs::File;
    use std::io;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS,
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Security::Authorization::{
        GetSecurityInfo, SetEntriesInAclW, SetSecurityInfo, EXPLICIT_ACCESS_W, SET_ACCESS,
        SE_FILE_OBJECT, TRUSTEE_IS_SID, TRUSTEE_IS_USER, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        AclSizeInformation, EqualSid, GetAce, GetAclInformation, GetSecurityDescriptorControl,
        GetTokenInformation, InitializeSecurityDescriptor, IsValidSid,
        SetSecurityDescriptorControl, SetSecurityDescriptorDacl, SetSecurityDescriptorOwner,
        TokenUser, ACL, ACL_SIZE_INFORMATION, CONTAINER_INHERIT_ACE, DACL_SECURITY_INFORMATION,
        NO_INHERITANCE, OBJECT_INHERIT_ACE, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
        SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateDirectoryW, CreateFileW, FileAttributeTagInfo, GetFileInformationByHandleEx,
        CREATE_NEW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_FLAG_WRITE_THROUGH, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_ALWAYS, OPEN_EXISTING, READ_CONTROL, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::SystemServices::{
        ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct TokenHandle(HANDLE);

    impl Drop for TokenHandle {
        fn drop(&mut self) {
            if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
                unsafe { CloseHandle(self.0) };
            }
        }
    }

    struct OwnerOnlySecurity {
        token_user: Vec<usize>,
        acl: *mut ACL,
        descriptor: Box<SECURITY_DESCRIPTOR>,
        directory: bool,
    }

    impl Drop for OwnerOnlySecurity {
        fn drop(&mut self) {
            if !self.acl.is_null() {
                unsafe { LocalFree(self.acl.cast()) };
            }
        }
    }

    impl OwnerOnlySecurity {
        fn new(directory: bool) -> io::Result<Self> {
            let mut token: HANDLE = null_mut();
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(io::Error::last_os_error());
            }
            let token = TokenHandle(token);
            let mut required = 0u32;
            if unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut required) } != 0
                || required == 0
                || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER
            {
                return Err(io::Error::last_os_error());
            }
            let mut token_user = vec![0usize; (required as usize).div_ceil(size_of::<usize>())];
            if unsafe {
                GetTokenInformation(
                    token.0,
                    TokenUser,
                    token_user.as_mut_ptr().cast(),
                    required,
                    &mut required,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
            if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
                return Err(io::Error::other("current Windows user SID is invalid"));
            }
            let access = EXPLICIT_ACCESS_W {
                grfAccessPermissions: FILE_ALL_ACCESS,
                grfAccessMode: SET_ACCESS,
                grfInheritance: if directory {
                    OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE
                } else {
                    NO_INHERITANCE
                },
                Trustee: TRUSTEE_W {
                    pMultipleTrustee: null_mut(),
                    MultipleTrusteeOperation: 0,
                    TrusteeForm: TRUSTEE_IS_SID,
                    TrusteeType: TRUSTEE_IS_USER,
                    ptstrName: sid.cast(),
                },
            };
            let mut acl: *mut ACL = null_mut();
            let status = unsafe { SetEntriesInAclW(1, &access, null(), &mut acl) };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            let mut descriptor = Box::new(unsafe { zeroed::<SECURITY_DESCRIPTOR>() });
            let descriptor_ptr = descriptor.as_mut() as *mut SECURITY_DESCRIPTOR as *mut c_void;
            if unsafe {
                InitializeSecurityDescriptor(descriptor_ptr, SECURITY_DESCRIPTOR_REVISION) != 0
                    && SetSecurityDescriptorOwner(descriptor_ptr, sid, 0) != 0
                    && SetSecurityDescriptorDacl(descriptor_ptr, 1, acl, 0) != 0
                    && SetSecurityDescriptorControl(
                        descriptor_ptr,
                        SE_DACL_PROTECTED,
                        SE_DACL_PROTECTED,
                    ) != 0
            } == false
            {
                unsafe { LocalFree(acl.cast()) };
                return Err(io::Error::last_os_error());
            }
            Ok(Self {
                token_user,
                acl,
                descriptor,
                directory,
            })
        }

        fn sid(&self) -> PSID {
            unsafe { (*(self.token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid }
        }

        fn attributes(&mut self) -> SECURITY_ATTRIBUTES {
            SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: self.descriptor.as_mut() as *mut SECURITY_DESCRIPTOR
                    as *mut c_void,
                bInheritHandle: 0,
            }
        }

        fn tighten_and_verify(&self, file: &File) -> io::Result<()> {
            let status = unsafe {
                SetSecurityInfo(
                    file.as_raw_handle() as HANDLE,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION
                        | DACL_SECURITY_INFORMATION
                        | PROTECTED_DACL_SECURITY_INFORMATION,
                    self.sid(),
                    null_mut(),
                    self.acl,
                    null(),
                )
            };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            self.verify(file)
        }

        fn verify(&self, file: &File) -> io::Result<()> {
            let mut owner: PSID = null_mut();
            let mut dacl: *mut ACL = null_mut();
            let mut descriptor = null_mut();
            let status = unsafe {
                GetSecurityInfo(
                    file.as_raw_handle() as HANDLE,
                    SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner,
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            };
            if status != ERROR_SUCCESS {
                return Err(io::Error::from_raw_os_error(status as i32));
            }
            struct DescriptorGuard(*mut c_void);
            impl Drop for DescriptorGuard {
                fn drop(&mut self) {
                    if !self.0.is_null() {
                        unsafe { LocalFree(self.0) };
                    }
                }
            }
            let _guard = DescriptorGuard(descriptor);
            let mut control = 0u16;
            let mut revision = 0u32;
            let mut info = unsafe { zeroed::<ACL_SIZE_INFORMATION>() };
            if owner.is_null()
                || unsafe { EqualSid(owner, self.sid()) } == 0
                || dacl.is_null()
                || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
                    == 0
                || control & SE_DACL_PROTECTED == 0
                || unsafe {
                    GetAclInformation(
                        dacl,
                        (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                        size_of::<ACL_SIZE_INFORMATION>() as u32,
                        AclSizeInformation,
                    )
                } == 0
                || info.AceCount != 1
            {
                return Err(io::Error::other(
                    "private correction store DACL is not owner-only",
                ));
            }
            let mut ace_ptr: *mut c_void = null_mut();
            if unsafe { GetAce(dacl, 0, &mut ace_ptr) } == 0 || ace_ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let ace =
                unsafe { &*(ace_ptr.cast::<windows_sys::Win32::Security::ACCESS_ALLOWED_ACE>()) };
            let required_flags = if self.directory {
                (OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE) as u8
            } else {
                NO_INHERITANCE as u8
            };
            let ace_sid = (&ace.SidStart as *const u32).cast_mut().cast();
            if ace.Header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
                || ace.Header.AceFlags != required_flags
                || ace.Mask != FILE_ALL_ACCESS
                || unsafe { EqualSid(ace_sid, self.sid()) } == 0
            {
                return Err(io::Error::other(
                    "private correction store grants access beyond its owner",
                ));
            }
            Ok(())
        }
    }

    fn wide(path: &Path) -> io::Result<Vec<u16>> {
        let mut path: Vec<u16> = path.as_os_str().encode_wide().collect();
        if path.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path contains NUL",
            ));
        }
        path.push(0);
        Ok(path)
    }

    fn validate_type(file: &File, directory: bool) -> io::Result<()> {
        let mut attributes = unsafe { zeroed::<FILE_ATTRIBUTE_TAG_INFO>() };
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle() as HANDLE,
                FileAttributeTagInfo,
                (&mut attributes as *mut FILE_ATTRIBUTE_TAG_INFO).cast(),
                size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        if attributes.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (attributes.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
        {
            return Err(io::Error::other(
                "private correction path is a reparse point or wrong type",
            ));
        }
        Ok(())
    }

    pub(super) fn secure(path: &Path, directory: bool) -> io::Result<()> {
        let wide = wide(path)?;
        let mut security = OwnerOnlySecurity::new(directory)?;
        let attributes = security.attributes();
        let handle = if directory && !path.exists() {
            if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
                return Err(io::Error::last_os_error());
            }
            unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    READ_CONTROL | WRITE_DAC | WRITE_OWNER,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    null(),
                    OPEN_EXISTING,
                    FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                    null_mut(),
                )
            }
        } else {
            unsafe {
                CreateFileW(
                    wide.as_ptr(),
                    if directory {
                        READ_CONTROL | WRITE_DAC | WRITE_OWNER
                    } else {
                        GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER
                    },
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    &attributes,
                    if path.exists() {
                        OPEN_EXISTING
                    } else {
                        OPEN_ALWAYS
                    },
                    FILE_FLAG_OPEN_REPARSE_POINT
                        | if directory {
                            FILE_FLAG_BACKUP_SEMANTICS
                        } else {
                            FILE_ATTRIBUTE_NORMAL
                        },
                    null_mut(),
                )
            }
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle as _) };
        validate_type(&file, directory)?;
        security.tighten_and_verify(&file)
    }

    pub(super) fn create_directory(path: &Path) -> io::Result<()> {
        let wide = wide(path)?;
        let mut security = OwnerOnlySecurity::new(true)?;
        let attributes = security.attributes();
        if unsafe { CreateDirectoryW(wide.as_ptr(), &attributes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle as _) };
        validate_type(&file, true)?;
        security.verify(&file)
    }

    /// Create a new retained lease file with its final owner-only DACL attached
    /// before the directory entry becomes visible.
    ///
    /// Applying the DACL after `CREATE_NEW` leaves a window where another
    /// process can open the inherited descriptor and correctly reject it as
    /// non-canonical (#800). The retained parent directory handle prevents the
    /// ambient path from being renamed while this call resolves it.
    pub(super) fn create_lease_file(path: &Path) -> io::Result<File> {
        let wide = wide(path)?;
        let mut security = OwnerOnlySecurity::new(false)?;
        let attributes = security.attributes();
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE | READ_CONTROL | WRITE_DAC | WRITE_OWNER,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                &attributes,
                CREATE_NEW,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_WRITE_THROUGH,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let file = unsafe { File::from_raw_handle(handle as _) };
        validate_type(&file, false)?;
        security.verify(&file)?;
        Ok(file)
    }

    pub(super) fn attest_handle(file: &File, directory: bool) -> io::Result<()> {
        validate_type(file, directory)?;
        OwnerOnlySecurity::new(directory)?.verify(file)
    }

    pub(super) fn secure_file_handle(file: &File) -> io::Result<()> {
        validate_type(file, false)?;
        OwnerOnlySecurity::new(false)?.tighten_and_verify(file)
    }
}

#[cfg(windows)]
pub(crate) fn secure_private_parent(parent: &Path) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut cursor = parent;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(std::io::Error::other(
                        "correction store ancestor is a reparse point or wrong type",
                    ));
                }
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                cursor = cursor.parent().ok_or_else(|| {
                    std::io::Error::other("correction store has no existing ancestor")
                })?;
            }
            Err(error) => return Err(error),
        }
    }
    if missing.is_empty() {
        return windows_private::secure(parent, true);
    }
    for directory in missing.iter().rev() {
        windows_private::secure(directory, true)?;
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn secure_private_file(path: &Path) -> std::io::Result<()> {
    windows_private::secure(path, false)
}

#[cfg(windows)]
pub(crate) fn secure_private_file_handle(file: &File) -> std::io::Result<()> {
    windows_private::secure_file_handle(file)
}

#[cfg(windows)]
pub(crate) fn create_owner_only_directory(path: &Path) -> std::io::Result<()> {
    windows_private::create_directory(path)
}

#[cfg(windows)]
pub(crate) fn create_owner_only_lease_file(path: &Path) -> std::io::Result<File> {
    windows_private::create_lease_file(path)
}

#[cfg(windows)]
pub(crate) fn attest_owner_only_directory_handle(file: &File) -> std::io::Result<()> {
    windows_private::attest_handle(file, true)
}

#[cfg(windows)]
pub(crate) fn attest_owner_only_file_handle(file: &File) -> std::io::Result<()> {
    windows_private::attest_handle(file, false)
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn secure_private_parent(_parent: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "private correction stores are unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn secure_private_file(_path: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "private correction stores are unsupported on this platform",
    ))
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn secure_private_file_handle(_file: &File) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "private correction stores are unsupported on this platform",
    ))
}

fn secure_sidecars(path: &Path) -> Result<(), OverlayError> {
    secure_private_file(path)?;
    for suffix in ["-journal", "-wal", "-shm"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.exists() {
            secure_private_file(&sidecar)?;
        }
    }
    Ok(())
}

fn canonical_private_store_path(path: &Path) -> std::io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("correction store has no parent"))?;
    secure_private_parent(parent)?;
    let canonical_parent = parent.canonicalize()?;
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("correction store has no filename"))?;
    Ok(canonical_parent.join(name))
}

fn open_db(path: &Path) -> Result<Connection, OverlayError> {
    let path = canonical_private_store_path(path)?;
    secure_private_file(&path)?;
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )?;
    // DELETE mode deliberately avoids durable WAL/SHM correction sidecars.
    conn.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;")?;
    create_schema(&conn)?;
    secure_sidecars(&path)?;
    Ok(conn)
}

fn create_schema(conn: &Connection) -> Result<(), OverlayError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS overlays (
            id INTEGER PRIMARY KEY,
            entity_key TEXT NOT NULL,
            overlay_type TEXT NOT NULL,
            value TEXT NOT NULL,
            confidence TEXT NOT NULL,
            source TEXT NOT NULL,
            reversible_to TEXT,
            note TEXT,
            created_at TEXT NOT NULL,
            source_sha256 TEXT,
            UNIQUE(entity_key, overlay_type)
        );
        CREATE INDEX IF NOT EXISTS idx_overlays_entity_key ON overlays(entity_key);
        CREATE INDEX IF NOT EXISTS idx_overlays_type ON overlays(overlay_type);",
    )?;
    let has_source_revision = {
        let mut stmt = conn.prepare("PRAGMA table_info(overlays)")?;
        let has_source_revision = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == "source_sha256");
        has_source_revision
    };
    if !has_source_revision {
        conn.execute("ALTER TABLE overlays ADD COLUMN source_sha256 TEXT", [])?;
    }
    Ok(())
}

fn meeting_key(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn speaker_entity_key(meeting_path: &Path, speaker_label: &str) -> String {
    format!(
        "meeting:{}#speaker:{}",
        meeting_key(meeting_path),
        speaker_label
    )
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(unix)]
fn stable_source_sha256(path: &Path) -> std::io::Result<String> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(std::io::Error::other(
            "speaker correction source is not a regular file",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let before = file.metadata()?;
    let first = crate::policy_fs::read_bound_file_twice_bounded(
        &mut file,
        MAX_OVERLAY_SOURCE_BYTES,
        std::time::Instant::now() + OVERLAY_READ_DEADLINE,
    )?;
    let after = file.metadata()?;
    let path_after = fs::metadata(path)?;
    let same = |left: &fs::Metadata, right: &fs::Metadata| {
        left.dev() == right.dev() && left.ino() == right.ino()
    };
    if !same(&before, &after) || !same(&before, &path_after) || before.len() != after.len() {
        return Err(std::io::Error::other(
            "speaker correction source changed while binding",
        ));
    }
    Ok(sha256_hex(&first))
}

#[cfg(windows)]
fn stable_source_sha256(path: &Path) -> std::io::Result<String> {
    use std::mem::zeroed;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
    };

    let open = || {
        OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    };
    let mut file = open()?;
    let identify = |file: &File| -> std::io::Result<BY_HANDLE_FILE_INFORMATION> {
        let mut info = unsafe { zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &mut info) } == 0
            || info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            return Err(std::io::Error::other(
                "speaker correction source identity is invalid",
            ));
        }
        Ok(info)
    };
    let before = identify(&file)?;
    let first = crate::policy_fs::read_bound_file_twice_bounded(
        &mut file,
        MAX_OVERLAY_SOURCE_BYTES,
        std::time::Instant::now() + OVERLAY_READ_DEADLINE,
    )?;
    let after = identify(&file)?;
    let path_after = identify(&open()?)?;
    let same = |left: &BY_HANDLE_FILE_INFORMATION, right: &BY_HANDLE_FILE_INFORMATION| {
        left.dwVolumeSerialNumber == right.dwVolumeSerialNumber
            && left.nFileIndexHigh == right.nFileIndexHigh
            && left.nFileIndexLow == right.nFileIndexLow
    };
    if !same(&before, &after) || !same(&before, &path_after) {
        return Err(std::io::Error::other(
            "speaker correction source changed while binding",
        ));
    }
    Ok(sha256_hex(&first))
}

#[cfg(not(any(unix, windows)))]
fn stable_source_sha256(_path: &Path) -> std::io::Result<String> {
    Err(std::io::Error::other(
        "speaker correction binding is unsupported on this platform",
    ))
}

pub fn write_speaker_confirmation(
    meeting_path: &Path,
    speaker_label: &str,
    name: &str,
    reversible_to: Option<&str>,
    note: Option<&str>,
) -> Result<SpeakerConfirmation, OverlayError> {
    write_speaker_confirmation_at(
        &db_path(),
        meeting_path,
        speaker_label,
        name,
        reversible_to,
        note,
    )
}

pub fn write_speaker_confirmation_at(
    db_path: &Path,
    meeting_path: &Path,
    speaker_label: &str,
    name: &str,
    reversible_to: Option<&str>,
    note: Option<&str>,
) -> Result<SpeakerConfirmation, OverlayError> {
    let source_sha256 = stable_source_sha256(meeting_path)?;
    let conn = open_db(db_path)?;
    let created_at = chrono::Utc::now().to_rfc3339();
    let entity_key = speaker_entity_key(meeting_path, speaker_label);

    conn.execute(
        "INSERT INTO overlays
            (entity_key, overlay_type, value, confidence, source, reversible_to, note, created_at, source_sha256)
         VALUES (?1, 'speaker', ?2, 'high', 'manual', ?3, ?4, ?5, ?6)
         ON CONFLICT(entity_key, overlay_type) DO UPDATE SET
            value = excluded.value,
            confidence = excluded.confidence,
            source = excluded.source,
            reversible_to = excluded.reversible_to,
            note = excluded.note,
            created_at = excluded.created_at,
            source_sha256 = excluded.source_sha256",
        params![entity_key, name, reversible_to, note, created_at, source_sha256],
    )?;

    let confirmation = SpeakerConfirmation {
        meeting_key: meeting_key(meeting_path),
        speaker_label: speaker_label.to_string(),
        name: name.to_string(),
        confidence: Confidence::High,
        source: AttributionSource::Manual,
        reversible_to: reversible_to.map(str::to_string),
        note: note.map(str::to_string),
        created_at,
        source_sha256: Some(source_sha256),
    };
    drop(conn);
    secure_sidecars(db_path)?;
    Ok(confirmation)
}

/// Load one transactionally stable, process-memory snapshot of every manual
/// speaker confirmation. SQLite owns descriptor binding for the read
/// transaction; `SQLITE_OPEN_NOFOLLOW` rejects a replaced symlink, DELETE
/// journaling avoids ambient WAL/SHM state, and callers re-read the logical
/// revision after materialization before exposing correction-derived facts.
pub(crate) fn stable_speaker_overlay_snapshot_at(
    db_path: &Path,
) -> Result<StableSpeakerOverlaySnapshot, OverlayError> {
    stable_speaker_overlay_snapshot_at_until(
        db_path,
        std::time::Instant::now() + OVERLAY_READ_DEADLINE,
    )
}

pub(crate) fn stable_speaker_overlay_snapshot_at_until(
    db_path: &Path,
    operation_deadline: std::time::Instant,
) -> Result<StableSpeakerOverlaySnapshot, OverlayError> {
    let deadline = operation_deadline.min(std::time::Instant::now() + OVERLAY_READ_DEADLINE);
    let check_deadline = || -> Result<(), OverlayError> {
        if std::time::Instant::now() >= deadline {
            Err(OverlayError::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "speaker correction store exceeded its materialization deadline",
            )))
        } else {
            Ok(())
        }
    };
    check_deadline()?;
    if let Some(parent) = db_path.parent() {
        match fs::symlink_metadata(parent) {
            Ok(_) => secure_private_parent(parent)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    if !db_path.exists() {
        if let Ok(metadata) = fs::symlink_metadata(db_path) {
            if metadata.file_type().is_symlink() {
                return Err(std::io::Error::other(
                    "speaker correction store is not a regular file",
                )
                .into());
            }
        }
        return Ok(StableSpeakerOverlaySnapshot::empty());
    }

    let canonical_db_path = canonical_private_store_path(db_path)?;
    secure_private_file(&canonical_db_path)?;
    if fs::metadata(&canonical_db_path)?.len() > MAX_OVERLAY_DB_BYTES {
        return Err(
            std::io::Error::other("speaker correction store exceeded its byte budget").into(),
        );
    }
    let conn = Connection::open_with_flags(
        &canonical_db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE,
    )?;
    conn.progress_handler(1_000, Some(move || std::time::Instant::now() >= deadline));
    conn.execute_batch("PRAGMA temp_store=MEMORY; PRAGMA query_only=ON; BEGIN")?;
    let temp_mode: i64 = conn.query_row("PRAGMA temp_store", [], |row| row.get(0))?;
    if temp_mode != 2 {
        return Err(std::io::Error::other(
            "SQLite refused the memory-only temporary-storage policy",
        )
        .into());
    }
    check_deadline()?;
    let (row_count, aggregate_bytes, largest_field): (i64, i64, i64) = conn.query_row(
        "SELECT COUNT(*),
                COALESCE(SUM(length(CAST(entity_key AS BLOB)) +
                             length(CAST(value AS BLOB)) +
                             length(CAST(confidence AS BLOB)) +
                             length(CAST(source AS BLOB)) +
                             COALESCE(length(CAST(reversible_to AS BLOB)), 0) +
                             COALESCE(length(CAST(note AS BLOB)), 0) +
                             length(CAST(created_at AS BLOB)) +
                             COALESCE(length(CAST(source_sha256 AS BLOB)), 0)), 0),
                COALESCE(MAX(MAX(length(CAST(entity_key AS BLOB)),
                                 length(CAST(value AS BLOB)),
                                 length(CAST(confidence AS BLOB)),
                                 length(CAST(source AS BLOB)),
                                 COALESCE(length(CAST(reversible_to AS BLOB)), 0),
                                 COALESCE(length(CAST(note AS BLOB)), 0),
                                 length(CAST(created_at AS BLOB)),
                                 COALESCE(length(CAST(source_sha256 AS BLOB)), 0))), 0)
         FROM overlays WHERE overlay_type = 'speaker'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    check_deadline()?;
    if row_count > MAX_OVERLAY_ROWS
        || aggregate_bytes > MAX_OVERLAY_AGGREGATE_FIELD_BYTES
        || largest_field > MAX_OVERLAY_FIELD_BYTES
    {
        return Err(std::io::Error::other(
            "speaker correction store exceeded its materialization budget",
        )
        .into());
    }
    let has_source_revision = {
        let mut columns = conn.prepare("PRAGMA table_info(overlays)")?;
        let has_source_revision = columns
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|column| column == "source_sha256");
        has_source_revision
    };
    check_deadline()?;
    let query = if has_source_revision {
        "SELECT entity_key, value, confidence, source, reversible_to, note, created_at, source_sha256
         FROM overlays
         WHERE overlay_type = 'speaker'
         ORDER BY entity_key ASC, created_at ASC, id ASC"
    } else {
        "SELECT entity_key, value, confidence, source, reversible_to, note, created_at, NULL
         FROM overlays
         WHERE overlay_type = 'speaker'
         ORDER BY entity_key ASC, created_at ASC, id ASC"
    };
    let mut stmt = conn.prepare(query)?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<String>>(7)?,
        ))
    })?;

    let mut hasher = Sha256::new();
    hasher.update(OVERLAY_REVISION_DOMAIN);
    let mut by_meeting: HashMap<String, Vec<SpeakerConfirmation>> = HashMap::new();
    for row in rows {
        check_deadline()?;
        let (entity_key, name, confidence, source, reversible_to, note, created_at, source_sha256) =
            row?;
        if confidence != "high" || source != "manual" {
            return Err(std::io::Error::other(
                "speaker correction store contains an invalid confirmation",
            )
            .into());
        }
        let Some((meeting_key, speaker_label)) = entity_key
            .strip_prefix("meeting:")
            .and_then(|key| key.rsplit_once("#speaker:"))
        else {
            return Err(std::io::Error::other(
                "speaker correction store contains an invalid entity key",
            )
            .into());
        };
        if meeting_key.is_empty() || speaker_label.is_empty() {
            return Err(std::io::Error::other(
                "speaker correction store contains an empty identity",
            )
            .into());
        }
        if source_sha256.as_ref().is_some_and(|revision| {
            revision.len() != 64
                || !revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(std::io::Error::other(
                "speaker correction store contains an invalid source revision",
            )
            .into());
        }

        for value in [
            Some(entity_key.as_str()),
            Some(name.as_str()),
            Some(confidence.as_str()),
            Some(source.as_str()),
            reversible_to.as_deref(),
            note.as_deref(),
            Some(created_at.as_str()),
            source_sha256.as_deref(),
        ] {
            hash_field(&mut hasher, value);
        }

        by_meeting
            .entry(meeting_key.to_string())
            .or_default()
            .push(SpeakerConfirmation {
                meeting_key: meeting_key.to_string(),
                speaker_label: speaker_label.to_string(),
                name,
                confidence: Confidence::High,
                source: AttributionSource::Manual,
                reversible_to,
                note,
                created_at,
                source_sha256,
            });
    }
    drop(stmt);
    check_deadline()?;
    conn.execute_batch("COMMIT")?;
    drop(conn);
    secure_sidecars(&canonical_db_path)?;
    Ok(StableSpeakerOverlaySnapshot {
        by_meeting,
        revision: finish_revision(hasher),
    })
}

pub fn load_speaker_confirmations_for_meeting_at(
    db_path: &Path,
    meeting_path: &Path,
) -> Result<Vec<SpeakerConfirmation>, OverlayError> {
    Ok(stable_speaker_overlay_snapshot_at(db_path)?
        .confirmations_for_meeting(meeting_path)
        .to_vec())
}

/// Load confirmations only when they were recorded against these exact
/// meeting bytes. A path is an identity hint, not sufficient authority: the
/// same path may now contain a different (or newly restricted) meeting.
pub fn load_speaker_confirmations_for_source_at(
    db_path: &Path,
    meeting_path: &Path,
    source_content: &[u8],
) -> Result<Vec<SpeakerConfirmation>, OverlayError> {
    let source_sha256: [u8; 32] = Sha256::digest(source_content).into();
    Ok(stable_speaker_overlay_snapshot_at(db_path)?
        .confirmations_for_source(meeting_path, &source_sha256))
}

/// Stable digest surfaced by structured CLI responses so out-of-process
/// consumers can prove an overlay was derived from the bytes they authorized.
pub fn speaker_overlay_source_sha256(source_content: &[u8]) -> String {
    sha256_hex(source_content)
}

pub fn apply_speaker_confirmations(
    speaker_map: &mut Vec<SpeakerAttribution>,
    confirmations: &[SpeakerConfirmation],
) {
    for confirmation in confirmations {
        if let Some(existing) = speaker_map
            .iter_mut()
            .find(|attr| attr.speaker_label == confirmation.speaker_label)
        {
            existing.name = confirmation.name.clone();
            existing.confidence = Confidence::High;
            existing.source = AttributionSource::Manual;
        } else {
            speaker_map.push(SpeakerAttribution {
                speaker_label: confirmation.speaker_label.clone(),
                name: confirmation.name.clone(),
                confidence: Confidence::High,
                source: AttributionSource::Manual,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn speaker_confirmation_roundtrips() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("overlays.db");
        let meeting = tmp.path().join("meeting.md");
        std::fs::write(&meeting, "---\ntitle: Test\n---\n").unwrap();

        write_speaker_confirmation_at(
            &db,
            &meeting,
            "SPEAKER_0",
            "Alex Kim",
            Some("Speaker 0"),
            Some("confirmed in test"),
        )
        .unwrap();

        let confirmations = load_speaker_confirmations_for_meeting_at(&db, &meeting).unwrap();
        assert_eq!(confirmations.len(), 1);
        assert_eq!(confirmations[0].speaker_label, "SPEAKER_0");
        assert_eq!(confirmations[0].name, "Alex Kim");
        assert_eq!(confirmations[0].reversible_to.as_deref(), Some("Speaker 0"));
        assert_eq!(
            confirmations[0].source_sha256.as_deref().map(str::len),
            Some(64)
        );
        for suffix in ["-wal", "-shm", "-journal"] {
            assert!(!PathBuf::from(format!("{}{suffix}", db.display())).exists());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&db).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(tmp.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn exact_source_loader_does_not_replay_overlay_after_same_path_replacement() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("overlays.db");
        let meeting = tmp.path().join("meeting.md");
        let original = b"---\ntitle: Original\n---\nSPEAKER_0: private canary\n";
        let replacement = b"---\ntitle: Replacement\n---\nSPEAKER_0: ordinary text\n";
        std::fs::write(&meeting, original).unwrap();
        write_speaker_confirmation_at(&db, &meeting, "SPEAKER_0", "Restricted Alice", None, None)
            .unwrap();

        assert_eq!(
            load_speaker_confirmations_for_source_at(&db, &meeting, original)
                .unwrap()
                .len(),
            1
        );
        std::fs::write(&meeting, replacement).unwrap();
        assert!(
            load_speaker_confirmations_for_source_at(&db, &meeting, replacement)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn confirmation_rejects_a_source_above_the_exact_byte_budget() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("overlays.db");
        let meeting = tmp.path().join("oversized.md");
        let file = std::fs::File::create(&meeting).unwrap();
        file.set_len(MAX_OVERLAY_SOURCE_BYTES + 1).unwrap();
        drop(file);
        let error = write_speaker_confirmation_at(
            &db,
            &meeting,
            "SPEAKER_0",
            "Synthetic Person",
            None,
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("byte budget"));
        assert!(!db.exists());
    }

    #[test]
    fn overlay_snapshot_rejects_an_oversized_field_before_materialization() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("overlays.db");
        let conn = open_db(&db).unwrap();
        conn.execute(
            "INSERT INTO overlays
             (entity_key, overlay_type, value, confidence, source, created_at, source_sha256)
             VALUES (?1, 'speaker', ?2, 'high', 'manual', ?3, ?4)",
            params![
                "meeting:/synthetic/meeting.md#speaker:SPEAKER_0",
                "x".repeat((MAX_OVERLAY_FIELD_BYTES + 1) as usize),
                "2026-07-21T00:00:00Z",
                "0".repeat(64),
            ],
        )
        .unwrap();
        drop(conn);
        let error = stable_speaker_overlay_snapshot_at(&db).unwrap_err();
        assert!(error.to_string().contains("materialization budget"));
    }

    #[test]
    fn correction_root_matches_packaged_home_placeholders_and_relative_overrides() {
        let _guard = crate::test_home_env_lock();
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        let old_home = std::env::var_os("HOME");
        let old_minutes_home = std::env::var_os("MINUTES_HOME");
        std::env::set_var("HOME", &home);

        std::env::set_var("MINUTES_HOME", "");
        assert_eq!(correction_state_dir(), home.join(".minutes"));
        std::env::set_var("MINUTES_HOME", "${HOME}/.minutes");
        assert_eq!(correction_state_dir(), home.join(".minutes"));
        std::env::set_var("MINUTES_HOME", "$HOME/.minutes-placeholder");
        assert_eq!(correction_state_dir(), home.join(".minutes-placeholder"));
        std::env::set_var("MINUTES_HOME", "~/.minutes-dev/../private-minutes");
        assert_eq!(correction_state_dir(), home.join("private-minutes"));
        assert_eq!(default_db_path(), home.join("private-minutes/overlays.db"));
        assert_eq!(
            crate::vocabulary::default_path(),
            home.join("private-minutes/vocabulary.toml")
        );

        std::env::set_var("MINUTES_HOME", "relative-minutes/./state");
        assert_eq!(correction_state_dir(), home.join("relative-minutes/state"));
        let meeting = home.join("meeting.md");
        fs::write(&meeting, "---\ntitle: Nested root proof\n---\n").unwrap();
        write_speaker_confirmation(&meeting, "SPEAKER_0", "Nested Alex", None, None).unwrap();
        let vocabulary = crate::vocabulary::VocabularyStore {
            entries: vec![crate::vocabulary::VocabularyEntry::new(
                crate::vocabulary::VocabularyKind::Person,
                "Nested Alex",
            )],
        };
        crate::vocabulary::save_at(&crate::vocabulary::default_path(), &vocabulary).unwrap();
        assert_eq!(
            load_speaker_confirmations_for_meeting_at(&default_db_path(), &meeting)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            crate::vocabulary::load_at(&crate::vocabulary::default_path())
                .unwrap()
                .entries[0]
                .canonical,
            "Nested Alex"
        );

        match old_home {
            Some(value) => std::env::set_var("HOME", value),
            None => std::env::remove_var("HOME"),
        }
        match old_minutes_home {
            Some(value) => std::env::set_var("MINUTES_HOME", value),
            None => std::env::remove_var("MINUTES_HOME"),
        }
    }
}
