//! Bounded parent-memory, finite-deadline supervision for audio engine children.
//!
//! Callers construct the command (including the executable allow-list and
//! environment policy); this module owns only process-tree lifetime and pipe
//! budgets. A timeout or budget failure terminates the supervised Unix process
//! group or Windows Job. Callers that opt into `address_space_limit` also get
//! an OS-enforced RLIMIT_AS / Job Object memory ceiling. This is not a sandbox:
//! a deliberately daemonizing user-configured Unix executable can call
//! `setsid` after it has already received the user's authority and leave that
//! group.

use std::ffi::{OsStr, OsString};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStderr, ChildStdin, ChildStdout, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use zeroize::Zeroizing;

pub(crate) struct BoundExecutable {
    /// An immutable private snapshot on Unix and a non-write/non-delete-shared
    /// exact source handle on Windows. Worker execution never trusts a later
    /// reopen of the caller-provided source bytes.
    file: std::fs::File,
    source_path: PathBuf,
    #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
    bytes: u64,
    #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
    digest: [u8; 32],
    #[cfg(windows)]
    snapshot_owner: Arc<WindowsExecutableSnapshotOwner>,
    #[cfg(all(unix, not(target_os = "linux")))]
    snapshot_owner: Arc<UnixExecutableSnapshotOwner>,
}

#[cfg(windows)]
struct WindowsExecutableSnapshotOwner {
    // Drop the no-delete chain before TempDir attempts recursive cleanup.
    _directory_guard: crate::policy_fs::BoundRecoveryDirectory,
    _temp_dir: tempfile::TempDir,
}

#[cfg(all(unix, not(target_os = "linux")))]
struct UnixExecutableSnapshotOwner {
    // Drop the no-delete chain before TempDir attempts recursive cleanup.
    _directory_guard: crate::policy_fs::BoundRecoveryDirectory,
    _temp_dir: tempfile::TempDir,
}

#[cfg(all(unix, not(target_os = "linux")))]
type ImmutableUnixExecutableSnapshot = (
    std::fs::File,
    PathBuf,
    Arc<UnixExecutableSnapshotOwner>,
    u64,
    [u8; 32],
);

impl BoundExecutable {
    #[cfg(target_os = "linux")]
    fn verify(&self) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let metadata = self.file.metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o222 != 0 {
            Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "worker executable snapshot was not immutable",
            ))
        } else {
            Ok(())
        }
    }

    /// Re-verify the snapshot immediately before launch.
    ///
    /// Unlike Linux, this snapshot is reachable through a pathname, so mode
    /// and digest are both re-checked here rather than relying on the inode
    /// being unlinked. Same contract as the Windows implementation below.
    #[cfg(all(unix, not(target_os = "linux")))]
    fn verify(&self) -> std::io::Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::{Seek, SeekFrom};
        use std::os::unix::fs::PermissionsExt;

        let mut file = self.file.try_clone()?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.permissions().mode() & 0o222 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "worker executable snapshot was not immutable",
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        let bytes = std::io::copy(&mut file, &mut hasher)?;
        let digest: [u8; 32] = hasher.finalize().into();
        if bytes != self.bytes || digest != self.digest {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "worker executable changed after its authority was bound",
            ));
        }
        Ok(())
    }

    #[cfg(windows)]
    fn verify(&self) -> std::io::Result<()> {
        use sha2::{Digest, Sha256};
        use std::io::{Seek, SeekFrom};

        let mut file = self.file.try_clone()?;
        file.seek(SeekFrom::Start(0))?;
        let mut hasher = Sha256::new();
        let mut total = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total = total
                .checked_add(read as u64)
                .ok_or_else(|| std::io::Error::other("worker executable byte count overflowed"))?;
            hasher.update(&buffer[..read]);
        }
        let digest: [u8; 32] = hasher.finalize().into();
        if total != self.bytes || digest != self.digest {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "worker executable changed after its authority was bound",
            ));
        }
        Ok(())
    }

    #[cfg(not(any(unix, windows)))]
    fn verify(&self) -> std::io::Result<()> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "bound worker executables are unsupported on this platform",
        ))
    }

    fn launch_path(&self) -> PathBuf {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            // memfd is sealed and unlinked, so descriptor execution is both
            // possible and the strongest available binding here.
            PathBuf::from(format!("/proc/self/fd/{}", self.file.as_raw_fd()))
        }
        // Every other platform launches the snapshot through its pathname.
        // Measured on macOS 26.6: Darwin refuses descriptor execution of an
        // unlinked inode with EACCES whether or not a writer is live, so
        // `/dev/fd` execution is not available here. `verify()` re-checks mode
        // and digest immediately before launch instead.
        #[cfg(not(target_os = "linux"))]
        {
            self.source_path.clone()
        }
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn bind(path: &Path) -> std::io::Result<Self> {
        bind_executable(path, false)
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn current() -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        {
            // This procfs link resolves the already-running image rather than
            // a replaceable ambient pathname returned by current_exe().
            bind_executable(Path::new("/proc/self/exe"), true)
        }
        #[cfg(not(target_os = "linux"))]
        {
            // macOS has no procfs equivalent, and `_NSGetExecutablePath`, which
            // backs `current_exe()`, returns the pathname the process was
            // invoked through rather than the resolved image. Verified on
            // macOS 26.6: launching through a symlink returns the symlink, and
            // opening it `O_NOFOLLOW` fails ELOOP. Homebrew links
            // `bin/minutes` into the Cellar exactly that way, so binding the
            // raw value made self-exec impossible on a standard install.
            //
            // Resolve the symlink chain first, then keep `O_NOFOLLOW` for the
            // final component so the resolved image still cannot be swapped
            // for a symlink between canonicalisation and open.
            let path = std::env::current_exe()?.canonicalize()?;
            bind_executable(&path, false)
        }
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn try_clone(&self) -> std::io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            source_path: self.source_path.clone(),
            #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
            bytes: self.bytes,
            #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
            digest: self.digest,
            #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
            snapshot_owner: Arc::clone(&self.snapshot_owner),
        })
    }
}

fn bind_executable(path: &Path, allow_proc_self_image: bool) -> std::io::Result<BoundExecutable> {
    if !path.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "bound worker executable path must be absolute",
        ));
    }

    #[cfg(unix)]
    let source = {
        use std::os::unix::fs::OpenOptionsExt;
        let no_follow = if allow_proc_self_image {
            0
        } else {
            libc::O_NOFOLLOW
        };
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_CLOEXEC | no_follow)
            .open(path)?
    };
    #[cfg(windows)]
    let source = {
        let _ = allow_proc_self_image;
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ,
        };
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?
    };
    #[cfg(not(any(unix, windows)))]
    let file = {
        let _ = allow_proc_self_image;
        std::fs::OpenOptions::new().read(true).open(path)?
    };

    #[cfg(unix)]
    let metadata = source.metadata()?;
    #[cfg(windows)]
    let metadata = source.metadata()?;
    #[cfg(not(any(unix, windows)))]
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "bound worker executable was not a regular file",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "bound worker executable was not executable",
            ));
        }
    }
    #[cfg(target_os = "linux")]
    let file = immutable_unix_executable_snapshot(&source)?;
    #[cfg(all(unix, not(target_os = "linux")))]
    let (file, source_path, snapshot_owner, bytes, digest) =
        immutable_unix_executable_snapshot(&source)?;
    #[cfg(windows)]
    let (file, source_path, snapshot_owner, bytes, digest) =
        immutable_windows_executable_snapshot(&source)?;
    #[cfg(not(any(windows, all(unix, not(target_os = "linux")))))]
    let source_path = path.to_path_buf();

    Ok(BoundExecutable {
        file,
        source_path,
        #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
        bytes,
        #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
        digest,
        #[cfg(any(windows, all(unix, not(target_os = "linux"))))]
        snapshot_owner,
    })
}

#[cfg(any(unix, windows))]
const MAX_BOUND_EXECUTABLE_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(any(unix, windows))]
fn copy_executable_snapshot(
    source: &std::fs::File,
    destination: &mut std::fs::File,
) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom};

    let mut source = source.try_clone()?;
    source.seek(SeekFrom::Start(0))?;
    let mut total = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| std::io::Error::other("worker executable byte count overflowed"))?;
        if total > MAX_BOUND_EXECUTABLE_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "worker executable exceeded its immutable snapshot budget",
            ));
        }
        destination.write_all(&buffer[..read])?;
    }
    destination.flush()?;
    destination.sync_all()?;
    destination.seek(SeekFrom::Start(0))?;
    Ok(())
}

#[cfg(windows)]
fn immutable_windows_executable_snapshot(
    source: &std::fs::File,
) -> std::io::Result<(
    std::fs::File,
    PathBuf,
    Arc<WindowsExecutableSnapshotOwner>,
    u64,
    [u8; 32],
)> {
    use sha2::{Digest, Sha256};
    use std::io::{Seek, SeekFrom};
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

    let temp_dir = tempfile::Builder::new()
        .prefix("minutes-policy-worker-")
        .tempdir()?;
    let snapshot_directory = temp_dir.path().join("private");
    let directory_guard =
        crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(&snapshot_directory)?;
    let snapshot_path = snapshot_directory.join("worker.exe");
    let mut snapshot = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&snapshot_path)?;
    copy_executable_snapshot(source, &mut snapshot)?;

    let mut reader = snapshot.try_clone()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut reader, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    drop(reader);
    drop(snapshot);

    // Windows' image loader will not execute a file while a live handle still
    // has write access unless the loader also shares writes. Keep the setup
    // handle non-write/non-delete-shared while copying, close it, then bind a
    // read-only handle and compare it with the digest captured before that
    // close/reopen boundary. A replacement in the gap therefore fails closed,
    // while the retained reader prevents any later write or name swap.
    let mut snapshot = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(&snapshot_path)?;
    let mut rebound_hasher = Sha256::new();
    let rebound_bytes = std::io::copy(&mut snapshot, &mut rebound_hasher)?;
    let rebound_digest: [u8; 32] = rebound_hasher.finalize().into();
    if rebound_bytes != bytes || rebound_digest != digest {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "worker executable snapshot changed at its read-only bind boundary",
        ));
    }
    snapshot.seek(SeekFrom::Start(0))?;
    let owner = Arc::new(WindowsExecutableSnapshotOwner {
        _directory_guard: directory_guard,
        _temp_dir: temp_dir,
    });
    Ok((snapshot, snapshot_path, owner, bytes, digest))
}

#[cfg(target_os = "linux")]
fn immutable_unix_executable_snapshot(source: &std::fs::File) -> std::io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;

    let name = b"minutes-policy-worker\0";
    let descriptor = unsafe {
        libc::syscall(
            libc::SYS_memfd_create,
            name.as_ptr().cast::<libc::c_char>(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if descriptor < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: memfd_create returned a new owned descriptor.
    let mut snapshot = unsafe { std::fs::File::from_raw_fd(descriptor as i32) };
    copy_executable_snapshot(source, &mut snapshot)?;
    if unsafe { libc::fchmod(descriptor as i32, 0o500) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
    if unsafe { libc::fcntl(descriptor as i32, libc::F_ADD_SEALS, seals) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(snapshot)
}

/// Snapshot to a real owner-private pathname rather than an unlinked inode.
///
/// The previous unlinked-`tempfile` plus `/dev/fd/N` construction could never
/// exec on macOS. Measured on macOS 26.6 arm64 against a real signed Minutes
/// binary, all three variants of the original shape:
///
/// - unlinked inode, live `O_RDWR` handle, exec `/dev/fd/N`: `EACCES`
/// - unlinked inode, writer dropped, exec `/dev/fd/N`: `EACCES`
/// - linked pathname, writer dropped, exec the path: succeeds
///
/// So the blocker is descriptor execution of an unlinked inode, not a live
/// writer, and dropping the writer alone would not have fixed it. Darwin
/// permits `execve` on a linked file that another descriptor holds open for
/// writing, unlike Linux, which returns `ETXTBSY` for that case. Only the
/// third shape works, which is why this snapshot is reachable by pathname.
///
/// The fix mirrors the Windows snapshot exactly, which has always exec'd a
/// real path for the same reason: write the bytes into an owner-private
/// directory, drop the writable handle so no live writer remains, then retain
/// only a read handle. Immutability is preserved by mode 0500 inside a 0700
/// directory plus the digest re-check in `verify()` immediately before launch,
/// rather than by unlinking.
#[cfg(all(unix, not(target_os = "linux")))]
fn immutable_unix_executable_snapshot(
    source: &std::fs::File,
) -> std::io::Result<ImmutableUnixExecutableSnapshot> {
    use sha2::{Digest, Sha256};
    use std::io::{Seek, SeekFrom};
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let temp_dir = tempfile::Builder::new()
        .prefix("minutes-bound-worker-")
        .tempdir()?;
    let directory_guard =
        crate::policy_fs::BoundRecoveryDirectory::prepare_owner_private(temp_dir.path())?;
    let snapshot_path = temp_dir.path().join("worker");

    {
        let mut snapshot = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o700)
            .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
            .open(&snapshot_path)?;
        copy_executable_snapshot(source, &mut snapshot)?;
        snapshot.set_permissions(std::fs::Permissions::from_mode(0o500))?;
        // The writable handle is dropped here so the snapshot is immutable
        // before it is ever launched. Note this is not what unblocks exec on
        // Darwin, which permits execve on a linked file another descriptor
        // holds open for writing; the linked pathname is what unblocks it.
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&snapshot_path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.permissions().mode() & 0o222 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "worker executable snapshot was not immutable",
        ));
    }

    let mut hasher = Sha256::new();
    let bytes = std::io::copy(&mut file, &mut hasher)?;
    let digest: [u8; 32] = hasher.finalize().into();
    file.seek(SeekFrom::Start(0))?;

    let owner = Arc::new(UnixExecutableSnapshotOwner {
        _directory_guard: directory_guard,
        _temp_dir: temp_dir,
    });
    Ok((file, snapshot_path, owner, bytes, digest))
}

#[cfg(unix)]
static VALIDATED_OUTER_PROCESS_GROUP: AtomicBool = AtomicBool::new(false);

/// Install the MCP process-audio helper's already-validated outer process
/// group as the containment boundary for subsequent child launches. The CLI
/// admits this mode only while holding a separate live supervisor capability;
/// this function repeats the Unix topology checks before changing behavior.
#[cfg(unix)]
pub(crate) fn install_validated_outer_process_group(process_group: i32) -> std::io::Result<()> {
    let parent = unsafe { libc::getppid() };
    let current_group = unsafe { libc::getpgrp() };
    let parent_group = unsafe { libc::getpgid(parent) };
    if process_group <= 1
        || parent != process_group
        || current_group != process_group
        || parent_group != process_group
        || unsafe { libc::getpid() } == process_group
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "outer process containment topology was not verified",
        ));
    }
    VALIDATED_OUTER_PROCESS_GROUP
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| std::io::Error::other("outer process containment was already installed"))?;
    Ok(())
}

pub(crate) const DEFAULT_STDOUT_LIMIT: u64 = 64 * 1024 * 1024;
#[cfg(feature = "parakeet")]
pub(crate) const DEFAULT_STDERR_TAIL: usize = 256 * 1024;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ChildBudget {
    pub(crate) wall_clock: Duration,
    pub(crate) stderr_tail: usize,
}

pub(crate) enum StdoutTarget {
    Capture {
        max_bytes: u64,
    },
    ExactWriter {
        writer: Box<dyn Write + Send>,
        max_bytes: u64,
    },
}

pub(crate) type StdinSource = Box<dyn Read + Send>;

/// An auditable child-launch description for the bounded supervisor.
///
/// Keeping the program, arguments, environment mutations, and working
/// directory beside the underlying [`std::process::Command`] lets the Unix
/// launch path use `execve` without losing any caller policy. The standard
/// library intentionally exposes no getter for `env_clear`, so accepting an
/// opaque `Command` here would make faithful direct execution impossible.
pub(crate) struct BoundedCommand {
    command: std::process::Command,
    #[cfg(unix)]
    program: OsString,
    arguments: Vec<OsString>,
    environment_clear: bool,
    environment: std::collections::BTreeMap<OsString, Option<OsString>>,
    current_dir: Option<PathBuf>,
    address_space_limit: Option<u64>,
    single_process: bool,
    close_extra_descriptors: bool,
    executable_authority: Option<BoundExecutable>,
}

impl BoundedCommand {
    pub(crate) fn new<S: AsRef<OsStr>>(program: S) -> Self {
        let program = program.as_ref().to_os_string();
        Self {
            command: crate::engine_process::command(&program),
            #[cfg(unix)]
            program,
            arguments: Vec::new(),
            environment_clear: false,
            environment: std::collections::BTreeMap::new(),
            current_dir: None,
            address_space_limit: None,
            single_process: false,
            close_extra_descriptors: false,
            executable_authority: None,
        }
    }

    #[cfg(all(test, not(target_os = "macos")))]
    pub(crate) fn new_bound_executable(path: &Path) -> std::io::Result<Self> {
        Self::from_bound_executable(BoundExecutable::bind(path)?)
    }

    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn from_bound_executable(authority: BoundExecutable) -> std::io::Result<Self> {
        authority.verify()?;
        let launch_path = authority.launch_path();
        let mut command = Self::new(launch_path);
        command.executable_authority = Some(authority);
        Ok(command)
    }

    pub(crate) fn arg<S: AsRef<OsStr>>(&mut self, argument: S) -> &mut Self {
        self.command.arg(argument.as_ref());
        self.arguments.push(argument.as_ref().to_os_string());
        self
    }

    pub(crate) fn args<I, S>(&mut self, arguments: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for argument in arguments {
            self.arg(argument);
        }
        self
    }

    #[cfg(test)]
    pub(crate) fn get_args(&self) -> impl Iterator<Item = &OsStr> {
        self.arguments.iter().map(OsString::as_os_str)
    }

    #[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
    pub(crate) fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.env(key.as_ref(), value.as_ref());
        self.environment.insert(
            key.as_ref().to_os_string(),
            Some(value.as_ref().to_os_string()),
        );
        self
    }

    #[allow(dead_code)] // Kept explicit so future callers cannot bypass policy tracking.
    pub(crate) fn env_remove<K: AsRef<OsStr>>(&mut self, key: K) -> &mut Self {
        self.command.env_remove(key.as_ref());
        self.environment.insert(key.as_ref().to_os_string(), None);
        self
    }

    #[allow(dead_code)] // Exercised by the Unix policy regressions.
    pub(crate) fn env_clear(&mut self) -> &mut Self {
        self.command.env_clear();
        self.environment_clear = true;
        self.environment.clear();
        self
    }

    #[allow(dead_code)] // Exercised by the Unix path-resolution regressions.
    pub(crate) fn current_dir<P: AsRef<Path>>(&mut self, directory: P) -> &mut Self {
        self.command.current_dir(directory.as_ref());
        self.current_dir = Some(directory.as_ref().to_path_buf());
        self
    }

    /// Install an OS-enforced child memory ceiling. Unix applies RLIMIT_AS
    /// before exec; Windows applies both process and job memory limits before
    /// resuming the initially suspended process. This is the hard boundary
    /// used for adversarial graph inputs; SQLite limits alone do not bound all
    /// transient allocations.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn address_space_limit(&mut self, bytes: u64) -> &mut Self {
        self.address_space_limit = Some(bytes);
        self
    }

    /// The ceiling this command will install, for callers whose containment
    /// argument depends on it and which therefore need to assert it is present
    /// rather than assume the builder chain still sets it.
    #[cfg(test)]
    pub(crate) fn configured_address_space_limit(&self) -> Option<u64> {
        self.address_space_limit
    }

    /// Refuse descendant process creation where the OS exposes a process-tree
    /// primitive that does not also block trusted worker threads. Windows
    /// installs a Job active-process limit of one before the suspended worker
    /// is resumed. Unix callers pair this flag with an exact bound executable;
    /// RLIMIT_NPROC is deliberately not used because it counts pthreads too.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn single_process(&mut self) -> &mut Self {
        self.single_process = true;
        self
    }

    /// Prevent ambient parent descriptors from becoming worker capabilities.
    /// The three explicit stdio pipes remain available; every other Unix
    /// descriptor is marked close-on-exec in the child. Windows callers that
    /// need this boundary must additionally retire inherited HANDLEs inside an
    /// immutable trusted worker before reading any authority; graph_worker does
    /// so as its first operation.
    ///
    /// `audio_decode_worker` is a KNOWN EXCEPTION to that "must": it calls this
    /// and deliberately does not retire handles on Windows. The reasoning lives
    /// at its `maybe_run_audio_decode_worker`, and it is noted here so a reader
    /// of this contract does not conclude every caller satisfies it.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub(crate) fn close_extra_descriptors(&mut self) -> &mut Self {
        self.close_extra_descriptors = true;
        self
    }

    #[cfg(unix)]
    #[cfg_attr(not(feature = "parakeet"), allow(dead_code))]
    pub(crate) unsafe fn pre_exec<F>(&mut self, function: F) -> &mut Self
    where
        F: FnMut() -> std::io::Result<()> + Send + Sync + 'static,
    {
        use std::os::unix::process::CommandExt;

        // SAFETY: the caller owns the same async-signal-safety obligation as
        // `CommandExt::pre_exec`; this wrapper only retains the explicit launch
        // policy that the bounded supervisor needs for its final execve.
        unsafe {
            self.command.pre_exec(function);
        }
        self
    }
}

#[derive(Debug)]
struct ChildSpawnFailure {
    source: std::io::Error,
    context: Option<String>,
}

impl std::fmt::Display for ChildSpawnFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.source.fmt(formatter)?;
        if let Some(context) = &self.context {
            write!(formatter, "; {context}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ChildSpawnFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

fn child_spawn_failure(source: std::io::Error) -> std::io::Error {
    std::io::Error::new(
        source.kind(),
        ChildSpawnFailure {
            source,
            context: None,
        },
    )
}

pub(crate) fn is_spawn_failure(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|source| source.downcast_ref::<ChildSpawnFailure>().is_some())
}

pub(crate) fn with_context_preserving_spawn_failure(
    error: std::io::Error,
    context: impl Into<String>,
) -> std::io::Error {
    let kind = error.kind();
    if is_spawn_failure(&error) {
        std::io::Error::new(
            kind,
            ChildSpawnFailure {
                source: error,
                context: Some(context.into()),
            },
        )
    } else {
        std::io::Error::new(kind, format!("{error}; {}", context.into()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StdinCompletion {
    /// Authenticated audio consumers must read through EOF so a corrupt suffix
    /// cannot be hidden behind a valid prefix.
    RequireComplete,
    /// A bounded metadata probe may intentionally stop after enough input.
    /// Only child-side BrokenPipe is accepted; source read errors still fail.
    AllowChildEarlyClose,
}

#[derive(Debug)]
pub(crate) struct ChildRun {
    pub(crate) output: Output,
    pub(crate) timed_out: bool,
}

enum StdinTransferError {
    SourceRead(std::io::Error),
    ChildSink(std::io::Error),
    Cancelled(std::io::Error),
}

impl StdinTransferError {
    fn into_io_error(self) -> std::io::Error {
        match self {
            Self::SourceRead(error) | Self::ChildSink(error) | Self::Cancelled(error) => error,
        }
    }
}

enum PipeEvent {
    Stdout(std::io::Result<Vec<u8>>),
    Stderr(std::io::Result<Vec<u8>>),
    Stdin(Result<(), StdinTransferError>),
    Limit(&'static str),
}

fn resource_error(message: &'static str) -> std::io::Error {
    std::io::Error::other(message)
}

#[cfg(unix)]
fn make_pipe_nonblocking(pipe: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = pipe.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(unix))]
fn make_pipe_nonblocking<T>(_pipe: &T) -> std::io::Result<()> {
    Ok(())
}

fn read_capture_bounded(
    mut source: ChildStdout,
    max_bytes: u64,
    cancel: &AtomicBool,
    events: &mpsc::Sender<PipeEvent>,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let retained_capacity = usize::try_from(max_bytes.min(1024 * 1024)).unwrap_or(1024 * 1024);
    let mut retained = Vec::with_capacity(retained_capacity);
    let mut total = 0_u64;
    let mut exceeded = false;
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "child stdout was cancelled",
            ));
        }
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if !cancel.load(Ordering::Acquire) && !exceeded && total <= max_bytes {
            retained.extend_from_slice(&buffer[..read]);
        } else if !exceeded && total > max_bytes {
            exceeded = true;
            retained.clear();
            let _ = events.send(PipeEvent::Limit("child stdout resource budget exceeded"));
        }
    }
    if exceeded {
        Err(resource_error("child stdout resource budget exceeded"))
    } else {
        Ok(retained)
    }
}

fn stream_file_bounded(
    mut source: ChildStdout,
    mut destination: Box<dyn Write + Send>,
    max_bytes: u64,
    cancel: &AtomicBool,
    events: &mpsc::Sender<PipeEvent>,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let mut total = 0_u64;
    let mut exceeded = false;
    let mut buffer = Zeroizing::new([0_u8; 256 * 1024]);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "private audio stdout was cancelled",
            ));
        }
        let read = match source.read(buffer.as_mut()) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if !cancel.load(Ordering::Acquire) && !exceeded && total <= max_bytes {
            destination.write_all(&buffer[..read])?;
        } else if !exceeded && total > max_bytes {
            exceeded = true;
            let _ = events.send(PipeEvent::Limit(
                "private audio output resource budget exceeded",
            ));
        }
    }
    if exceeded {
        Err(resource_error(
            "private audio output resource budget exceeded",
        ))
    } else if cancel.load(Ordering::Acquire) {
        Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "private audio output was cancelled",
        ))
    } else {
        destination.flush()?;
        Ok(Vec::new())
    }
}

fn read_tail(
    mut source: ChildStderr,
    max_bytes: usize,
    cancel: &AtomicBool,
) -> std::io::Result<Vec<u8>> {
    make_pipe_nonblocking(&source)?;
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancel.load(Ordering::Acquire) {
            return Ok(retained);
        }
        let read = match source.read(&mut buffer) {
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            Err(error) => return Err(error),
        };
        if read == 0 {
            break;
        }
        retained.extend_from_slice(&buffer[..read]);
        if retained.len() > max_bytes {
            let overflow = retained.len() - max_bytes;
            retained.drain(..overflow);
        }
    }
    Ok(retained)
}

fn write_input_cancelable(
    mut input: StdinSource,
    mut sink: ChildStdin,
    cancel: &AtomicBool,
) -> Result<(), StdinTransferError> {
    make_pipe_nonblocking(&sink).map_err(StdinTransferError::ChildSink)?;
    let mut buffer = Zeroizing::new([0_u8; 256 * 1024]);
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(StdinTransferError::Cancelled(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "child stdin was cancelled",
            )));
        }
        let read = input
            .read(buffer.as_mut())
            .map_err(StdinTransferError::SourceRead)?;
        if read == 0 {
            return sink.flush().map_err(StdinTransferError::ChildSink);
        }
        let mut written = 0;
        while written < read {
            if cancel.load(Ordering::Acquire) {
                return Err(StdinTransferError::Cancelled(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "child stdin was cancelled",
                )));
            }
            match sink.write(&buffer[written..read]) {
                Ok(0) => {
                    return Err(StdinTransferError::ChildSink(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "child stdin accepted zero bytes",
                    )));
                }
                Ok(count) => written += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(error) => return Err(StdinTransferError::ChildSink(error)),
            }
        }
    }
}

#[cfg(unix)]
fn configure_process_tree(
    command: &mut std::process::Command,
    use_outer_group: bool,
    address_space_limit: Option<u64>,
    single_process: bool,
    close_extra_descriptors: bool,
) {
    use std::os::unix::process::CommandExt;

    // Computed in the PARENT, where allocation and ordinary syscalls are safe,
    // and captured by value so the child closure only reads an integer.
    //
    // This used to call `getrlimit(RLIMIT_NOFILE)` inside `pre_exec` and scan up
    // to `rlim_cur`. macOS lets an unprivileged user set `ulimit -n unlimited`,
    // which reports as `RLIM_INFINITY`, and the resulting sweep was measured at
    // about 101 ns per iteration: roughly 217 seconds of spinning per spawn,
    // against a 60 second duration-probe timeout. Every probe on such a machine
    // timed out and the file was misrouted. `getdtablesize()` is the bound that
    // is actually true of the process: XNU returns the minimum of the soft
    // limit and the kernel's per-process maximum, so it stays finite exactly
    // where `getrlimit` does not. Moving it out of the closure also removes a
    // call that POSIX does not list as async-signal-safe.
    //
    // Known gap, stated rather than papered over: a process that LOWERS its
    // soft limit while holding descriptors above the new value can leave those
    // above this bound. Minutes opens its own descriptors close-on-exec, so the
    // remaining exposure is inherited-and-then-lowered, which this does not
    // cover.
    //
    // NOT TESTED, and it cannot be from Linux. Measured here, `getdtablesize()`
    // and `rlim_cur` are the same number (524288), because Linux implements the
    // former as the latter; the two diverge only on Darwin, where XNU caps it at
    // the kernel's per-process maximum. Linux also takes the `close_range` path
    // above and never reaches this loop. A test asserting this bound therefore
    // passes on Linux whichever value is used, so no such test is written: it
    // would be coverage in name only. Verifying this needs a macOS run with
    // `ulimit -n unlimited` asserting the spawn completes promptly and the
    // canary is still closed.
    let descriptor_scan_upper: libc::c_int = if close_extra_descriptors {
        // SAFETY: no arguments, no Rust-managed state, always succeeds.
        unsafe { libc::getdtablesize() }
    } else {
        0
    };

    // SAFETY: setpgid, setrlimit, and fcntl are async-signal-safe and touch no
    // Rust-managed state. All policy values are copied in.
    unsafe {
        command.pre_exec(move || {
            if !use_outer_group && libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            if let Some(bytes) = address_space_limit {
                // `rlim_t` is u64 on our current Unix builders but is not
                // guaranteed to match u64 on every supported Unix target.
                #[allow(clippy::useless_conversion)]
                let limit: libc::rlim_t = bytes.try_into().map_err(|_| {
                    std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        "child address-space limit exceeds platform range",
                    )
                })?;
                let limits = libc::rlimit {
                    rlim_cur: limit,
                    rlim_max: limit,
                };
                if libc::setrlimit(libc::RLIMIT_AS, &limits) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
            }
            let _ = single_process;
            if close_extra_descriptors {
                #[cfg(target_os = "linux")]
                {
                    // close_range(CLOEXEC) is one async-signal-safe syscall
                    // regardless of a container's often-million-fd rlimit.
                    // It preserves Rust's spawn-error pipe until successful
                    // exec while closing every ambient capability at exec.
                    const CLOSE_RANGE_CLOEXEC: libc::c_uint = 1 << 2;
                    if libc::syscall(
                        libc::SYS_close_range,
                        3 as libc::c_uint,
                        u32::MAX,
                        CLOSE_RANGE_CLOEXEC,
                    ) == 0
                    {
                        return Ok(());
                    }
                    let error = std::io::Error::last_os_error();
                    if !matches!(
                        error.raw_os_error(),
                        Some(libc::ENOSYS) | Some(libc::EINVAL)
                    ) {
                        return Err(error);
                    }
                }
                // Bounded by the parent-computed table size. Marks
                // close-on-exec rather than closing: Rust keeps an internal
                // pipe to report `pre_exec` and `exec` failures to the parent,
                // and closing that descriptor here would make a failed launch
                // look like a successful one.
                for descriptor in 3..descriptor_scan_upper {
                    let flags = libc::fcntl(descriptor, libc::F_GETFD);
                    if flags >= 0
                        && libc::fcntl(descriptor, libc::F_SETFD, flags | libc::FD_CLOEXEC) != 0
                    {
                        return Err(std::io::Error::last_os_error());
                    }
                }
            }
            Ok(())
        });
    }
}

/// Replace Rust's Unix `execvp` launch with direct `execve` after the standard
/// child setup and process-group hook. `execvp` may hand `ENOEXEC` files to
/// `/bin/sh`, which makes an invalid decoder look like a decoder that started
/// and rejected the user's audio. Direct exec keeps every kernel launch error
/// on the parent's spawn handshake, where callers can fail closed honestly,
/// while the explicit launch description preserves environment and cwd policy.
#[cfg(unix)]
fn configure_direct_exec(command: &mut BoundedCommand) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::process::CommandExt;

    fn c_string(value: &OsStr, label: &str) -> std::io::Result<CString> {
        CString::new(value.as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("child {label} contains an interior NUL byte"),
            )
        })
    }

    fn default_search_path() -> OsString {
        const FALLBACK: &str = "/bin:/usr/bin";
        let required = unsafe { libc::confstr(libc::_CS_PATH, std::ptr::null_mut(), 0) };
        if required <= 1 {
            return OsString::from(FALLBACK);
        }
        let mut buffer = vec![0_u8; required];
        let written = unsafe {
            libc::confstr(
                libc::_CS_PATH,
                buffer.as_mut_ptr().cast::<libc::c_char>(),
                buffer.len(),
            )
        };
        if written == 0 || written > buffer.len() {
            return OsString::from(FALLBACK);
        }
        buffer.truncate(written);
        if buffer.last() == Some(&0) {
            buffer.pop();
        }
        OsString::from_vec(buffer)
    }

    let mut effective_environment = if command.environment_clear {
        std::collections::BTreeMap::new()
    } else {
        std::env::vars_os().collect::<std::collections::BTreeMap<_, _>>()
    };
    for (key, value) in &command.environment {
        match value {
            Some(value) => {
                effective_environment.insert(key.clone(), value.clone());
            }
            None => {
                effective_environment.remove(key);
            }
        }
    }

    let parent_dir = std::env::current_dir()?;
    let effective_dir = match &command.current_dir {
        Some(directory) if directory.is_absolute() => directory.clone(),
        Some(directory) => parent_dir.join(directory),
        None => parent_dir,
    };
    let requested_program = command.program.clone();
    let executable_candidates: Vec<OsString> = if requested_program.as_bytes().contains(&b'/') {
        let path = PathBuf::from(&requested_program);
        if path.is_absolute() {
            vec![requested_program.clone()]
        } else {
            vec![effective_dir.join(path).into_os_string()]
        }
    } else {
        let search_path = effective_environment
            .get(OsStr::new("PATH"))
            .cloned()
            .unwrap_or_else(default_search_path);
        let mut candidates = std::env::split_paths(&search_path)
            .map(|entry| {
                let directory = if entry.as_os_str().is_empty() {
                    effective_dir.clone()
                } else if entry.is_absolute() {
                    entry
                } else {
                    effective_dir.join(entry)
                };
                directory.join(&requested_program).into_os_string()
            })
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            candidates.push(effective_dir.join(&requested_program).into_os_string());
        }
        candidates
    };
    let executable_candidates = executable_candidates
        .iter()
        .map(|candidate| c_string(candidate, "executable path"))
        .collect::<std::io::Result<Vec<_>>>()?;
    let mut arguments = Vec::with_capacity(command.arguments.len() + 1);
    arguments.push(c_string(&requested_program, "argv[0]")?);
    for argument in &command.arguments {
        arguments.push(c_string(argument, "argument")?);
    }
    // Store exposed addresses instead of raw pointers so the pre-exec closure
    // remains Send + Sync. The captured CString allocations keep every target
    // byte buffer stable until execv replaces the child image or returns.
    let mut argument_addresses = arguments
        .iter()
        .map(|argument| argument.as_ptr() as usize)
        .collect::<Vec<_>>();
    argument_addresses.push(0);

    let mut environment = Vec::with_capacity(effective_environment.len());
    for (key, value) in effective_environment {
        if key.as_bytes().contains(&b'=') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "child environment key contains '='",
            ));
        }
        let mut entry = OsString::with_capacity(key.len() + value.len() + 1);
        entry.push(key);
        entry.push("=");
        entry.push(value);
        environment.push(c_string(&entry, "environment entry")?);
    }
    let mut environment_addresses = environment
        .iter()
        .map(|entry| entry.as_ptr() as usize)
        .collect::<Vec<_>>();
    environment_addresses.push(0);

    // SAFETY: all allocation and CString validation happened in the parent.
    // The child closure performs only async-signal-safe execve and errno access.
    // Both address vectors are null-terminated and point into captured
    // CStrings whose allocations remain alive in the closure. Searching the
    // already-split candidates directly preserves legal ':' bytes in cwd/path
    // components and avoids `join_paths` round-tripping. Like execvp, EACCES
    // wins if at least one candidate was denied; unlike execvp, ENOEXEC is
    // returned directly and is never handed to a shell.
    unsafe {
        command.command.pre_exec(move || {
            let _keep_arguments_alive = &arguments;
            let _keep_environment_alive = &environment;
            let _keep_executables_alive = &executable_candidates;
            let mut permission_denied = false;
            for executable in &executable_candidates {
                libc::execve(
                    executable.as_ptr(),
                    argument_addresses.as_ptr().cast::<*const libc::c_char>(),
                    environment_addresses.as_ptr().cast::<*const libc::c_char>(),
                );
                let error = std::io::Error::last_os_error();
                match error.raw_os_error() {
                    Some(libc::EACCES) => permission_denied = true,
                    Some(libc::ENOENT) | Some(libc::ENOTDIR) => {}
                    _ => return Err(error),
                }
            }
            Err(std::io::Error::from_raw_os_error(if permission_denied {
                libc::EACCES
            } else {
                libc::ENOENT
            }))
        });
    }
    Ok(())
}

#[cfg(windows)]
fn configure_process_tree(
    command: &mut std::process::Command,
    _use_outer_group: bool,
    _address_space_limit: Option<u64>,
    _single_process: bool,
    _close_extra_descriptors: bool,
) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;

    // A live CreateProcess→AssignToJob gap lets a fast child escape before
    // containment. Start suspended; `ProcessTree::attach` assigns the job and
    // resumes only after KILL_ON_JOB_CLOSE is authoritative.
    command.creation_flags(CREATE_SUSPENDED);
}

#[cfg(not(any(unix, windows)))]
fn configure_process_tree(
    _command: &mut std::process::Command,
    _use_outer_group: bool,
    _address_space_limit: Option<u64>,
    _single_process: bool,
    _close_extra_descriptors: bool,
) {
}

#[cfg(unix)]
fn synthetic_terminated_status() -> ExitStatus {
    use std::os::unix::process::ExitStatusExt;

    ExitStatus::from_raw(libc::SIGKILL)
}

#[cfg(windows)]
fn synthetic_terminated_status() -> ExitStatus {
    use std::os::windows::process::ExitStatusExt;

    ExitStatus::from_raw(1)
}

#[cfg(not(any(unix, windows)))]
fn synthetic_terminated_status() -> ExitStatus {
    panic!("bounded child supervision is unsupported on this platform")
}

/// Observe an exited Unix leader without reaping it. The unreaped zombie keeps
/// its PID—and therefore its dedicated PGID—reserved until `ProcessTree` has
/// retired every descendant. Calling `Child::try_wait` here would reap the
/// leader and open a window in which the numeric PGID could be recycled before
/// the later group kill.
#[cfg(unix)]
fn observe_child_exit(child: &mut std::process::Child) -> std::io::Result<bool> {
    loop {
        let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                child.id() as libc::id_t,
                info.as_mut_ptr(),
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result == 0 {
            let info = unsafe { info.assume_init() };
            return match info.si_signo {
                libc::SIGCHLD => Ok(true),
                0 => Ok(false),
                signal => Err(std::io::Error::other(format!(
                    "waitid returned unexpected signal {signal}"
                ))),
            };
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

#[cfg(not(unix))]
fn observe_child_exit(child: &mut std::process::Child) -> std::io::Result<bool> {
    child.try_wait().map(|status| status.is_some())
}

#[cfg(windows)]
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
}

struct ProcessTree {
    #[cfg(unix)]
    process_group: Option<i32>,
    #[cfg(windows)]
    job: windows_sys::Win32::Foundation::HANDLE,
}

impl ProcessTree {
    fn attach(
        child: &mut std::process::Child,
        use_outer_group: bool,
        address_space_limit: Option<u64>,
        single_process: bool,
    ) -> std::io::Result<Self> {
        #[cfg(unix)]
        {
            let _ = (address_space_limit, single_process);
            Ok(Self {
                process_group: if use_outer_group {
                    None
                } else {
                    Some(i32::try_from(child.id()).map_err(|_| {
                        std::io::Error::other("child pid exceeded process-group range")
                    })?)
                },
            })
        }

        #[cfg(windows)]
        {
            Self::attach_windows(
                child,
                use_outer_group,
                address_space_limit,
                address_space_limit,
                single_process,
            )
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = (child, use_outer_group, address_space_limit, single_process);
            Ok(Self {})
        }
    }

    #[cfg(windows)]
    fn attach_windows(
        child: &mut std::process::Child,
        use_outer_group: bool,
        process_memory_limit: Option<u64>,
        job_memory_limit: Option<u64>,
        single_process: bool,
    ) -> std::io::Result<Self> {
        use std::mem::{size_of, zeroed};
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
            SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        };

        let to_native_limit = |bytes| {
            usize::try_from(bytes).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "child memory limit exceeds Windows address range",
                )
            })
        };
        let process_memory_limit = process_memory_limit.map(to_native_limit).transpose()?;
        let job_memory_limit = job_memory_limit.map(to_native_limit).transpose()?;

        let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() || job == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        let mut limits = unsafe { zeroed::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(bytes) = process_memory_limit {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            limits.ProcessMemoryLimit = bytes;
        }
        if let Some(bytes) = job_memory_limit {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            limits.JobMemoryLimit = bytes;
        }
        if single_process {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            limits.BasicLimitInformation.ActiveProcessLimit = 1;
        }
        if unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        } == 0
        {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        if unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } == 0 {
            let error = std::io::Error::last_os_error();
            unsafe { CloseHandle(job) };
            return Err(error);
        }
        let resume_status = unsafe { NtResumeProcess(child.as_raw_handle() as _) };
        if resume_status < 0 {
            unsafe { CloseHandle(job) };
            return Err(std::io::Error::other(format!(
                "failed to resume supervised child (NTSTATUS 0x{:08x})",
                resume_status as u32
            )));
        }
        let _ = use_outer_group;
        Ok(Self { job })
    }

    fn terminate(&self, child: &mut std::process::Child) {
        #[cfg(unix)]
        if let Some(process_group) = self.process_group {
            unsafe {
                libc::kill(-process_group, libc::SIGKILL);
            }
        } else {
            // Killing the shared outer group here would also kill the CLI
            // supervisor. Retire the direct child; the MCP parent retires the
            // complete outer group when the CLI exits or exceeds its budget.
            let _ = child.kill();
        }

        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job, 1);
        }

        #[cfg(not(any(unix, windows)))]
        {
            let _ = child.kill();
        }

        #[cfg(windows)]
        let _ = child;
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job);
        }
    }
}

pub(crate) fn run(
    command: &mut BoundedCommand,
    input: Option<StdinSource>,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
) -> std::io::Result<ChildRun> {
    run_with_stdin_completion(
        command,
        input,
        stdout_target,
        budget,
        StdinCompletion::RequireComplete,
    )
}

pub(crate) fn run_allowing_child_to_close_stdin(
    command: &mut BoundedCommand,
    input: StdinSource,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
) -> std::io::Result<ChildRun> {
    run_with_stdin_completion(
        command,
        Some(input),
        stdout_target,
        budget,
        StdinCompletion::AllowChildEarlyClose,
    )
}

fn run_with_stdin_completion(
    command: &mut BoundedCommand,
    input: Option<StdinSource>,
    stdout_target: StdoutTarget,
    budget: ChildBudget,
    stdin_completion: StdinCompletion,
) -> std::io::Result<ChildRun> {
    if budget.wall_clock.is_zero() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "child wall-clock budget must be positive",
        ));
    }

    command
        .command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
    #[cfg(unix)]
    let use_outer_group = VALIDATED_OUTER_PROCESS_GROUP.load(Ordering::Acquire);
    #[cfg(not(unix))]
    let use_outer_group = false;
    configure_process_tree(
        &mut command.command,
        use_outer_group,
        command.address_space_limit,
        command.single_process,
        command.close_extra_descriptors,
    );
    #[cfg(unix)]
    configure_direct_exec(command).map_err(child_spawn_failure)?;

    command
        .executable_authority
        .as_ref()
        .map_or(Ok(()), BoundExecutable::verify)
        .map_err(child_spawn_failure)?;
    let mut child = command.command.spawn().map_err(child_spawn_failure)?;
    if let Some(authority) = &command.executable_authority {
        if let Err(error) = authority.verify() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(child_spawn_failure(error));
        }
    }
    let tree = match ProcessTree::attach(
        &mut child,
        use_outer_group,
        command.address_space_limit,
        command.single_process,
    ) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::other("child stdout pipe was unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| std::io::Error::other("child stderr pipe was unavailable"))?;

    let (events_tx, events_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));

    let stdout_events = events_tx.clone();
    let stdout_cancel = Arc::clone(&cancel);
    let stdout_thread = std::thread::spawn(move || {
        let result = match stdout_target {
            StdoutTarget::Capture { max_bytes } => {
                read_capture_bounded(stdout, max_bytes, &stdout_cancel, &stdout_events)
            }
            StdoutTarget::ExactWriter { writer, max_bytes } => {
                stream_file_bounded(stdout, writer, max_bytes, &stdout_cancel, &stdout_events)
            }
        };
        let _ = stdout_events.send(PipeEvent::Stdout(result));
    });

    let stderr_events = events_tx.clone();
    let stderr_cancel = Arc::clone(&cancel);
    let stderr_thread = std::thread::spawn(move || {
        let _ = stderr_events.send(PipeEvent::Stderr(read_tail(
            stderr,
            budget.stderr_tail,
            &stderr_cancel,
        )));
    });

    let stdin_thread = input.map(|input| {
        let stdin_events = events_tx.clone();
        let stdin_cancel = Arc::clone(&cancel);
        let stdin = child
            .stdin
            .take()
            .expect("stdin is piped when exact input is present");
        std::thread::spawn(move || {
            let result = write_input_cancelable(input, stdin, &stdin_cancel);
            let _ = stdin_events.send(PipeEvent::Stdin(result));
        })
    });
    drop(events_tx);

    let deadline = Instant::now() + budget.wall_clock;
    let mut leader_exited = false;
    let mut stdout_result: Option<std::io::Result<Vec<u8>>> = None;
    let mut stderr_result: Option<std::io::Result<Vec<u8>>> = None;
    let mut stdin_result: Option<std::io::Result<()>> = stdin_thread.is_none().then_some(Ok(()));
    let mut failure: Option<std::io::Error> = None;
    let mut tree_retired = false;
    let mut failure_cleanup_started = false;
    let mut cleanup_deadline: Option<Instant> = None;
    let mut timed_out = false;

    while !leader_exited
        || stdout_result.is_none()
        || stderr_result.is_none()
        || stdin_result.is_none()
    {
        if !leader_exited {
            match observe_child_exit(&mut child) {
                Ok(exited) => {
                    leader_exited = exited;
                    if leader_exited && failure.is_none() && !tree_retired {
                        // Descendants may inherit the leader's supervisor
                        // pipes. Retire the tree before waiting for EOF so a
                        // successful leader cannot be misclassified as a
                        // timeout merely because its descendant kept a handle.
                        tree.terminate(&mut child);
                        tree_retired = true;
                    }
                }
                Err(error) => {
                    if failure.is_none() {
                        failure = Some(error);
                    }
                }
            }
        }

        let now = Instant::now();
        if now >= deadline && failure.is_none() {
            timed_out = true;
            failure = Some(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "child process tree exceeded its wall-clock budget",
            ));
        }
        if failure.is_some() && !failure_cleanup_started {
            cancel.store(true, Ordering::Release);
            if !tree_retired {
                tree.terminate(&mut child);
                tree_retired = true;
            }
            failure_cleanup_started = true;
            cleanup_deadline = Some(Instant::now() + Duration::from_secs(5));
        }
        if cleanup_deadline.is_some_and(|cleanup| Instant::now() >= cleanup) {
            break;
        }

        let wait = cleanup_deadline
            .unwrap_or(deadline)
            .saturating_duration_since(now)
            .min(Duration::from_millis(25));
        match events_rx.recv_timeout(wait) {
            Ok(PipeEvent::Stdout(result)) => {
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stdout_result = Some(result);
            }
            Ok(PipeEvent::Stderr(result)) => {
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stderr_result = Some(result);
            }
            Ok(PipeEvent::Stdin(result)) => {
                let result = match result {
                    Err(StdinTransferError::ChildSink(error))
                        if stdin_completion == StdinCompletion::AllowChildEarlyClose
                            && error.kind() == std::io::ErrorKind::BrokenPipe =>
                    {
                        Ok(())
                    }
                    Ok(()) => Ok(()),
                    Err(error) => Err(error.into_io_error()),
                };
                if let Err(error) = &result {
                    if failure.is_none() {
                        failure = Some(std::io::Error::new(error.kind(), error.to_string()));
                    }
                }
                stdin_result = Some(result);
            }
            Ok(PipeEvent::Limit(message)) => {
                if failure.is_none() {
                    failure = Some(resource_error(message));
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // All pipe workers can finish before a still-running leader.
                // Keep enforcing the deadline instead of falling into an
                // unbounded `wait()` after channel closure.
                std::thread::sleep(wait.max(Duration::from_millis(1)));
            }
        }
    }

    // Tree retirement normally happens as soon as the leader exits above. Keep
    // this fallback for paths that completed only through a pipe-side failure.
    if !tree_retired {
        cancel.store(true, Ordering::Release);
        tree.terminate(&mut child);
    }
    if !leader_exited {
        // The cleanup deadline is a hard caller-latency contract. A process
        // stuck in uninterruptible kernel I/O may not reap even after tree
        // termination, so transfer ownership to a detached reaper rather
        // than turning the bounded call into a blocking `wait()`.
        let _ = std::thread::Builder::new()
            .name("minutes-child-reaper".into())
            .spawn(move || child.wait());

        if timed_out {
            return Ok(ChildRun {
                output: Output {
                    status: synthetic_terminated_status(),
                    stdout: Vec::new(),
                    stderr: stderr_result.and_then(Result::ok).unwrap_or_default(),
                },
                timed_out: true,
            });
        }
        return Err(failure.unwrap_or_else(|| {
            std::io::Error::other("child did not terminate before its cleanup deadline")
        }));
    }
    // On Unix this is deliberately the first reaping operation. The process
    // group has already been retired above while the leader PID still anchored
    // the numeric PGID, so no later signal can target a recycled group.
    let status = child.wait()?;
    if stdout_result.is_some() {
        let _ = stdout_thread.join();
    }
    if stderr_result.is_some() {
        let _ = stderr_thread.join();
    }
    if stdin_result.is_some() {
        if let Some(stdin_thread) = stdin_thread {
            let _ = stdin_thread.join();
        }
    }

    if let Some(error) = failure {
        if timed_out {
            return Ok(ChildRun {
                output: Output {
                    status,
                    stdout: Vec::new(),
                    stderr: stderr_result.and_then(Result::ok).unwrap_or_default(),
                },
                timed_out: true,
            });
        }
        return Err(error);
    }
    let stdout = stdout_result
        .ok_or_else(|| std::io::Error::other("child stdout supervisor disconnected"))??;
    let stderr = stderr_result
        .ok_or_else(|| std::io::Error::other("child stderr supervisor disconnected"))??;
    stdin_result.ok_or_else(|| std::io::Error::other("child stdin supervisor disconnected"))??;

    Ok(ChildRun {
        output: Output {
            status,
            stdout,
            stderr,
        },
        timed_out,
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn sh(script: &str) -> BoundedCommand {
        let mut command = BoundedCommand::new("sh");
        command.args(["-c", script]);
        command
    }

    fn budget(milliseconds: u64) -> ChildBudget {
        ChildBudget {
            wall_clock: Duration::from_millis(milliseconds),
            stderr_tail: 32,
        }
    }

    #[test]
    fn address_space_limit_is_explicit_and_nonzero_for_bounded_workers() {
        let mut command = BoundedCommand::new("sh");
        command.address_space_limit(64 * 1024 * 1024);
        assert_eq!(command.address_space_limit, Some(64 * 1024 * 1024));
    }

    #[test]
    fn every_os_spawn_error_retains_spawn_stage_identity() {
        let dir = tempfile::tempdir().unwrap();
        let mut command = BoundedCommand::new(dir.path().join("missing-executable"));

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("a missing executable must fail during spawn");

        assert!(is_spawn_failure(&error));
        let contextualized = with_context_preserving_spawn_failure(error, "cleanup failed");
        assert!(is_spawn_failure(&contextualized));
        assert!(contextualized.to_string().contains("cleanup failed"));
    }

    #[test]
    fn invalid_binary_image_never_falls_back_to_a_shell() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let invalid_executable = dir.path().join("foreign-image.exe");
        std::fs::write(
            &invalid_executable,
            b"MZ\x90\0synthetic foreign executable image",
        )
        .unwrap();
        std::fs::set_permissions(&invalid_executable, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut command = BoundedCommand::new(&invalid_executable);

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("an invalid image must fail the direct exec handshake");

        assert!(is_spawn_failure(&error));
    }

    #[test]
    fn direct_exec_preserves_environment_add_clear_and_remove_policy() {
        let mut cleared = BoundedCommand::new("/usr/bin/env");
        cleared
            .env("MINUTES_STALE_CHILD_VALUE", "must-disappear")
            .env_clear()
            .env("MINUTES_EXACT_CHILD_VALUE", "preserved");
        let cleared = run(
            &mut cleared,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();
        assert!(cleared.output.status.success());
        assert_eq!(
            cleared.output.stdout,
            b"MINUTES_EXACT_CHILD_VALUE=preserved\n"
        );

        let mut removed = BoundedCommand::new("/usr/bin/env");
        removed
            .env("MINUTES_EXACT_CHILD_VALUE", "preserved")
            .env_remove("PATH");
        let removed = run(
            &mut removed,
            None,
            StdoutTarget::Capture {
                max_bytes: 1024 * 1024,
            },
            budget(5_000),
        )
        .unwrap();
        assert!(removed.output.status.success());
        let output = String::from_utf8(removed.output.stdout).unwrap();
        assert!(output
            .lines()
            .any(|line| { line == "MINUTES_EXACT_CHILD_VALUE=preserved" }));
        assert!(!output.lines().any(|line| line.starts_with("PATH=")));
    }

    #[test]
    fn bare_program_uses_platform_default_path_when_child_path_is_absent() {
        let mut cleared = BoundedCommand::new("sh");
        cleared.env_clear().args(["-c", "exit 0"]);
        let cleared = run(
            &mut cleared,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .expect("env_clear must retain the platform default executable search path");
        assert!(cleared.output.status.success());

        let mut removed = BoundedCommand::new("sh");
        removed.env_remove("PATH").args(["-c", "exit 0"]);
        let removed = run(
            &mut removed,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .expect("removing child PATH must retain the platform default executable search path");
        assert!(removed.output.status.success());
    }

    #[test]
    fn bare_program_uses_command_path_and_relative_entries_use_command_cwd() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let working = directory.path().join("working");
        let binaries = working.join("bin");
        std::fs::create_dir_all(&binaries).unwrap();
        let executable = binaries.join("minutes-path-fixture");
        std::fs::write(&executable, b"#!/bin/sh\nprintf exact-path").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut command = BoundedCommand::new("minutes-path-fixture");
        command.env("PATH", "bin").current_dir(&working);
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();

        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"exact-path");
    }

    #[test]
    fn relative_path_survives_separator_in_command_working_directory() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::TempDir::new().unwrap();
        let working = directory.path().join("working:with-separator");
        let binaries = working.join("bin");
        std::fs::create_dir_all(&binaries).unwrap();
        let executable = binaries.join("minutes-colon-path-fixture");
        std::fs::write(&executable, b"#!/bin/sh\nprintf colon-path").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let mut command = BoundedCommand::new("minutes-colon-path-fixture");
        command.env("PATH", "bin").current_dir(&working);
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 4096 },
            budget(5_000),
        )
        .unwrap();

        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"colon-path");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn non_not_found_spawn_error_retains_spawn_stage_identity() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let busy_executable = dir.path().join("busy-executable");
        std::fs::write(&busy_executable, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&busy_executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let _write_lease = std::fs::OpenOptions::new()
            .write(true)
            .open(&busy_executable)
            .unwrap();
        let mut command = BoundedCommand::new(&busy_executable);

        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("a write-leased executable must fail to spawn on Linux");

        assert_ne!(error.kind(), std::io::ErrorKind::NotFound);
        assert_ne!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(is_spawn_failure(&error));
    }

    #[test]
    fn timeout_kills_a_child_process_tree() {
        let started = Instant::now();
        let run = run(
            &mut sh("sleep 30"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stdout_overflow_is_a_bounded_resource_error() {
        let error = run(
            &mut sh("while :; do printf 0123456789abcdef; done"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();
        assert!(error.to_string().contains("stdout resource budget"));
    }

    #[test]
    fn stderr_retains_only_the_configured_tail() {
        let run = run(
            &mut sh("printf 'prefix-abcdefghijklmnopqrstuvwxyz' >&2; printf ok"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        assert_eq!(run.output.stdout, b"ok");
        assert_eq!(run.output.stderr.len(), 32);
        assert!(run.output.stderr.ends_with(b"abcdefghijklmnopqrstuvwxyz"));
    }

    #[test]
    fn successful_leader_retires_descendant_retaining_pipes_without_timeout() {
        let started = Instant::now();
        let run = run(
            &mut sh("(sleep 30) & printf output; exit 7"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(10_000),
        )
        .unwrap();
        assert!(!run.timed_out);
        assert_eq!(run.output.status.code(), Some(7));
        assert_eq!(run.output.stdout, b"output");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn exact_file_overflow_never_writes_past_the_cap() {
        let temp = tempfile::tempfile().unwrap();
        let inspect = temp.try_clone().unwrap();
        let error = run(
            &mut sh("while :; do printf 0123456789abcdef; done"),
            None,
            StdoutTarget::ExactWriter {
                writer: Box::new(temp),
                max_bytes: 1024,
            },
            budget(5_000),
        )
        .unwrap_err();
        assert!(error.to_string().contains("resource budget"));
        assert!(inspect.metadata().unwrap().len() <= 1024);
    }

    #[test]
    fn child_that_never_reads_stdin_cannot_outlive_the_deadline() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let started = Instant::now();
        let run = run(
            &mut sh("sleep 30"),
            Some(Box::new(input)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();
        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn authenticated_default_rejects_child_that_closes_stdin_early() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let error = run(
            &mut sh("dd bs=1 count=1 >/dev/null 2>/dev/null"),
            Some(Box::new(input)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }

    #[test]
    fn bounded_metadata_probe_may_stop_reading_after_sufficient_input() {
        let input = tempfile::tempfile().unwrap();
        input.set_len(8 * 1024 * 1024).unwrap();
        let run = run_allowing_child_to_close_stdin(
            &mut sh("dd bs=1 count=1 >/dev/null 2>/dev/null"),
            Box::new(input),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        assert!(!run.timed_out);
    }

    #[test]
    fn metadata_probe_never_forgives_source_side_broken_pipe() {
        struct PrefixThenBrokenPipe(bool);

        impl std::io::Read for PrefixThenBrokenPipe {
            fn read(&mut self, destination: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "synthetic source failure",
                    ));
                }
                self.0 = true;
                destination[0] = b'x';
                Ok(1)
            }
        }

        let error = run_allowing_child_to_close_stdin(
            &mut sh("cat >/dev/null"),
            Box::new(PrefixThenBrokenPipe(false)),
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .expect_err("source-side BrokenPipe must never look like child early-close");

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert!(error.to_string().contains("synthetic source failure"));
    }

    #[test]
    fn successful_exit_status_and_streams_are_preserved() {
        let run = run(
            &mut sh("printf output; printf diagnostic >&2; exit 7"),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert_eq!(run.output.status.code(), Some(7));
        assert_eq!(run.output.stdout, b"output");
        assert_eq!(run.output.stderr, b"diagnostic");
        assert!(!run.timed_out);
        assert_ne!(run.output.status.signal(), Some(9));
    }

    #[test]
    fn dedicated_group_is_retired_before_leader_reap_can_recycle_its_pgid() {
        let mut command = crate::engine_process::command("sh");
        command.args(["-c", "exit 7"]);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_process_tree(&mut command, false, None, false, false);

        let mut child = command.spawn().unwrap();
        let child_pid = i32::try_from(child.id()).unwrap();
        let tree = ProcessTree::attach(&mut child, false, None, false).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !observe_child_exit(&mut child).unwrap() {
            assert!(
                Instant::now() < deadline,
                "child did not exit before deadline"
            );
            std::thread::sleep(Duration::from_millis(5));
        }

        assert_eq!(
            unsafe { libc::kill(child_pid, 0) },
            0,
            "non-reaping observation must keep the leader PID reserved"
        );
        tree.terminate(&mut child);
        let status = child.wait().unwrap();
        assert_eq!(status.code(), Some(7));
    }

    #[test]
    fn successful_leader_cannot_leave_a_detached_descendant_running() {
        let run = run(
            &mut sh("sleep 30 </dev/null >/dev/null 2>&1 & printf '%s' \"$!\""),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
        let pid = String::from_utf8(run.output.stdout)
            .unwrap()
            .parse::<i32>()
            .unwrap();
        for _ in 0..100 {
            let alive = unsafe { libc::kill(pid, 0) == 0 };
            if !alive {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("detached engine descendant survived successful leader retirement");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn setsid_escape_cannot_hold_supervisor_pipes_past_the_deadline() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("escaped.pid");
        let mut command = BoundedCommand::new("sh");
        command
            .args([
                "-c",
                "setsid sh -c 'echo $$ > \"$1\"; while :; do sleep 30; done' escaped \"$1\" & exit 0",
                "minutes-setsid-launcher",
            ])
            .arg(&pid_path);

        let started = Instant::now();
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(150),
        )
        .unwrap();

        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(5));
        let escaped_pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        unsafe {
            libc::kill(-escaped_pid, libc::SIGKILL);
        }
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn continuously_writing_setsid_escape_cannot_hold_pipe_workers() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("escaped-writer.pid");
        let mut command = BoundedCommand::new("sh");
        // Two things had to change to stop this racing the scheduler rather
        // than testing the invariant. It failed on unrelated pull requests by
        // returning Ok with empty stdout from an already-exited launcher.
        //
        // The escapee now writes past the 1024 byte budget before publishing
        // its pid, and the launcher waits for that pid before exiting, so
        // `run` can never observe a finished parent with nothing in the pipe.
        //
        // The wall clock budget is seconds rather than 150ms because the
        // budget error fires on byte count, not on the deadline: giving the
        // escapee room to be scheduled cannot slow the passing path down, it
        // only stops a loaded runner from hitting the deadline first and
        // reporting the wrong error. The elapsed assertion below still proves
        // the escapee cannot hold the pipe workers open, which is the point.
        command
            .args([
                "-c",
                "setsid sh -c 'printf \"%02048d\" 0; echo $$ > \"$1\"; while :; do printf x; done' escaped \"$1\" & \
                 attempts=0; \
                 while [ ! -s \"$1\" ] && [ \"$attempts\" -lt 500 ]; do sleep 0.01; attempts=$((attempts+1)); done; \
                 exit 0",
                "minutes-setsid-writer-launcher",
            ])
            .arg(&pid_path);

        let started = Instant::now();
        let error = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();

        assert!(
            error.to_string().contains("stdout resource budget"),
            "expected the stdout budget to trip, got: {error}"
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        let escaped_pid = std::fs::read_to_string(pid_path)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        unsafe {
            libc::kill(-escaped_pid, libc::SIGKILL);
        }
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn bound_executable_runs_the_retained_identity_after_path_replacement() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("worker");
        std::fs::copy("/bin/true", &executable).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = BoundedCommand::new_bound_executable(&executable).unwrap();
        let replacement = directory.path().join("replacement");
        std::fs::copy("/bin/false", &replacement).unwrap();
        std::fs::set_permissions(&replacement, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::rename(&replacement, &executable).unwrap();

        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn bound_executable_runs_immutable_snapshot_after_in_place_source_rewrite() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("worker");
        std::fs::copy("/bin/true", &executable).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut command = BoundedCommand::new_bound_executable(&executable).unwrap();
        std::fs::copy("/bin/false", &executable).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();

        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn current_executable_can_be_snapshotted_immutably() {
        BoundExecutable::current().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn close_extra_descriptors_removes_an_inheritable_parent_canary() {
        use std::os::fd::AsRawFd;

        let canary = tempfile::tempfile().unwrap();
        let descriptor = canary.as_raw_fd();
        unsafe {
            assert_eq!(libc::fcntl(descriptor, libc::F_SETFD, 0), 0);
        }
        let descriptor_text = descriptor.to_string();
        let shell = Path::new("/bin/sh").canonicalize().unwrap();
        let mut command = BoundedCommand::new_bound_executable(&shell).unwrap();
        command
            .args([
                "-c",
                "test ! -e \"/proc/self/fd/$1\"",
                "minutes-fd-canary",
                descriptor_text.as_str(),
            ])
            .close_extra_descriptors();
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap();
        assert!(run.output.status.success());
    }

    /// Guards the defect class that made the path-backed snapshot unusable, and
    /// records that the two Unixes disagree about it.
    ///
    /// Linux refuses `execve` with `ETXTBSY` on a vnode a process still holds
    /// open for writing, so a snapshot that retains its setup handle cannot be
    /// launched there. Darwin permits exactly that, measured on macOS 26.6.
    ///
    /// WHAT IT COVERS, and it is less than the name suggests. It constructs no
    /// `BoundExecutable` and no `BoundedCommand`: it execs a plain script, so no
    /// production mutation can fail it. It is a canary for the platform rule the
    /// snapshot design rests on, not coverage OF the snapshot.
    ///
    /// The two halves also have different reach. The write-rule half asserts on
    /// Linux and macOS only; on any other Unix it compiles and asserts nothing
    /// about the rule. The second half, that a sealed snapshot execs once no
    /// writer is live, is universal. The separate Darwin measurement that
    /// descriptor execution of an unlinked inode fails EACCES is recorded at
    /// `launch_path`; nothing here attempts descriptor execution.
    ///
    /// Item 4 of the track-1 remediation list. This asserted ETXTBSY for every
    /// Unix, so the suite was deterministically red on macOS: the prose above had
    /// been corrected to say Darwin differs while the assertion below still said
    /// it did not.
    ///
    /// What actually protects Darwin from a swapped snapshot is not the exec
    /// rule but the launch-time digest re-check in `verify()`, covered by
    /// `a_mutated_path_backed_snapshot_is_refused_before_launch` below.
    #[cfg(unix)]
    #[test]
    fn the_platform_write_rule_the_path_backed_snapshot_design_depends_on() {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker");
        let script = b"#!/bin/sh\nexit 0\n";

        let mut writable = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .unwrap();
        writable.write_all(script).unwrap();
        writable.flush().unwrap();
        let busy = crate::engine_process::command(&path).status();
        #[cfg(target_os = "linux")]
        assert_eq!(
            busy.err().map(|error| error.raw_os_error()),
            Some(Some(libc::ETXTBSY)),
            "Linux must refuse exec on a vnode with a live writer"
        );
        #[cfg(target_os = "macos")]
        assert!(
            busy.is_ok(),
            "Darwin permits exec with a live writer, measured on macOS 26.6; if this now \
             fails, the platform rule changed and the snapshot design should be re-read: {busy:?}"
        );
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            // No measurement exists for other Unixes from this lane, so assert
            // nothing rather than assume Linux's rule holds there.
            let _ = busy;
        }

        // Dropping the writer and sealing the mode is what makes launch work,
        // and that half is the same everywhere.
        drop(writable);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();
        let retained_read = std::fs::File::open(&path).unwrap();
        let status = crate::engine_process::command(&path)
            .status()
            .expect("exec must succeed once no writer is live");
        assert!(status.success());
        drop(retained_read);
    }

    /// The control that actually protects the path-backed platforms: a snapshot
    /// whose bytes changed after its authority was bound must be refused before
    /// launch, not executed and audited afterwards.
    ///
    /// EXECUTED NATIVELY ON macOS FROM THIS LANE. The digest re-check is
    /// `cfg(all(unix, not(target_os = "linux")))` because Linux binds a sealed
    /// memfd that cannot be rewritten, so this test is compiled out there. The
    /// cfg selects every non-Linux Unix, not macOS; it is this repo's CI matrix,
    /// which has a macOS runner and no other non-Linux Unix, that makes macOS
    /// the only place it currently executes in CI.
    #[cfg(all(unix, not(target_os = "linux")))]
    #[test]
    fn a_mutated_path_backed_snapshot_is_refused_before_launch() {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker");
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
            .unwrap();
        file.write_all(b"#!/bin/sh\nexit 0\n").unwrap();
        file.flush().unwrap();
        drop(file);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o500)).unwrap();

        let authority = BoundExecutable::bind(&path).expect("a sealed script must bind");
        // Rewrite the snapshot the authority is holding, which is what a
        // pathname-reachable snapshot makes possible and a memfd does not. The
        // mode is restored afterwards so the refusal comes from the digest
        // rather than from the mode half of the same check.
        let snapshot = authority.launch_path();
        std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(&snapshot, b"#!/bin/sh\nexit 9\n").unwrap();
        std::fs::set_permissions(&snapshot, std::fs::Permissions::from_mode(0o500)).unwrap();

        // Assert the DIGEST refusal specifically. `verify()` also refuses a
        // snapshot that is no longer a regular file or has regained a write bit,
        // and the mode was restored above precisely so those cannot be what
        // fires; naming the message is what stops a later mode-only check from
        // keeping this green with the digest comparison deleted.
        let error = match BoundedCommand::from_bound_executable(authority) {
            Ok(_) => panic!("a snapshot rewritten after binding must not reach exec"),
            Err(error) => error,
        };
        assert!(
            error
                .to_string()
                .contains("changed after its authority was bound"),
            "the refusal must come from the digest re-check, not another guard: {error}"
        );
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn powershell(script: &str) -> BoundedCommand {
        let mut command = BoundedCommand::new("powershell.exe");
        command.args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            script,
        ]);
        command
    }

    fn budget(milliseconds: u64) -> ChildBudget {
        ChildBudget {
            wall_clock: Duration::from_millis(milliseconds),
            stderr_tail: 32,
        }
    }

    #[test]
    fn read_only_executable_snapshot_launches_without_a_writer_sharing_conflict() {
        let authority = BoundExecutable::current().expect("bind the running test image");
        let mut command =
            BoundedCommand::from_bound_executable(authority).expect("bind its immutable snapshot");
        command.args([
            "--exact",
            "bounded_child::windows_tests::read_only_snapshot_child",
            "--nocapture",
        ]);
        let run = run(
            &mut command,
            None,
            StdoutTarget::Capture {
                max_bytes: 16 * 1024,
            },
            budget(30_000),
        )
        .expect("launch the read-only snapshot");
        assert!(
            run.output.status.success(),
            "snapshot child failed: {}",
            String::from_utf8_lossy(&run.output.stderr)
        );
    }

    #[test]
    fn read_only_snapshot_child() {}

    fn run_with_windows_memory_limits(
        script: &str,
        process_memory_limit: u64,
        job_memory_limit: u64,
    ) -> Output {
        let mut bounded = powershell(script);
        bounded
            .command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null());
        configure_process_tree(&mut bounded.command, false, None, false, false);

        let mut child = bounded
            .command
            .spawn()
            .expect("the independently limited child must launch suspended");
        let tree = ProcessTree::attach_windows(
            &mut child,
            false,
            Some(process_memory_limit),
            Some(job_memory_limit),
            false,
        )
        .expect("the independently limited child must attach to its Job");
        let output = child
            .wait_with_output()
            .expect("the independently limited child must exit");
        drop(tree);
        output
    }

    fn quote_powershell_literal(value: &std::path::Path) -> String {
        value.display().to_string().replace('\'', "''")
    }

    fn child_and_grandchild_script(pid_path: &std::path::Path, leader_tail: &str) -> String {
        // `-NoNewWindow` deliberately gives the grandchild the leader's
        // standard handles. A successful leader must still return promptly
        // with its own buffered output after Job retirement closes them.
        format!(
            "$grandchild = Start-Process \
             -FilePath \"$env:SystemRoot\\System32\\WindowsPowerShell\\v1.0\\powershell.exe\" \
             -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-Command',\
             'Start-Sleep -Seconds 30') -NoNewWindow -PassThru; \
             [IO.File]::WriteAllText('{}', [string]$grandchild.Id); {leader_tail}",
            quote_powershell_literal(pid_path)
        )
    }

    fn read_pid(path: &std::path::Path) -> u32 {
        std::fs::read_to_string(path)
            .expect("supervised leader must publish its descendant pid")
            .trim()
            .parse()
            .expect("descendant pid must be numeric")
    }

    fn process_is_active(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE: u32 = 259;

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }
            let mut exit_code = 0;
            let active =
                GetExitCodeProcess(handle, &mut exit_code) != 0 && exit_code == STILL_ACTIVE;
            CloseHandle(handle);
            active
        }
    }

    fn assert_process_tree_retired(pid: u32) {
        for _ in 0..200 {
            if !process_is_active(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("supervised Windows descendant {pid} survived Job Object retirement");
    }

    /// Prove both configured Windows Job Object committed-memory controls at
    /// runtime, rather than only asserting that the command builder carries a
    /// number.
    ///
    /// The per-process probe uses a deliberately looser whole-Job limit, so
    /// only `JOB_OBJECT_LIMIT_PROCESS_MEMORY` can refuse its allocation. The
    /// aggregate probe keeps each descendant below that per-process ceiling
    /// while their combined commit exceeds the whole-Job ceiling, so only
    /// `JOB_OBJECT_LIMIT_JOB_MEMORY` can refuse the second allocation.
    /// Unbounded controls are load-bearing in both cases: without them, ambient
    /// allocation pressure would look like enforcement.
    #[test]
    fn windows_job_memory_limit_refuses_committed_memory_over_budget() {
        const MIB: u64 = 1024 * 1024;
        const LIMIT_BYTES: u64 = 384 * MIB;
        const LOOSE_JOB_LIMIT_BYTES: u64 = 768 * MIB;
        const ALLOCATION_BYTES: u64 = 512 * MIB;
        const DESCENDANT_ALLOCATION_BYTES: u64 = 160 * MIB;
        let process_script = format!(
            "$ErrorActionPreference = 'Stop'; \
             [Console]::Out.Write('started|'); \
             try {{ \
                 $bytes = [byte[]]::new({ALLOCATION_BYTES}); \
                 [Console]::Out.Write('allocated'); \
                 exit 91; \
             }} catch {{ \
                 $base = $_.Exception.GetBaseException(); \
                 if ($base -is [System.OutOfMemoryException]) {{ \
                     [Console]::Out.Write('refused'); \
                     exit 0; \
                 }} \
                 [Console]::Error.Write($base.GetType().FullName + ': ' + $base.Message); \
                 exit 92; \
             }}"
        );

        let control = run(
            &mut powershell(&process_script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(15_000),
        )
        .expect("the unbounded allocation control must launch");
        assert!(!control.timed_out);
        assert_eq!(
            control.output.status.code(),
            Some(91),
            "the unbounded control must reach its intentional success exit"
        );
        assert_eq!(
            control.output.stdout, b"started|allocated",
            "the allocation must be viable before attributing refusal to the Job limit"
        );

        let process_limited =
            run_with_windows_memory_limits(&process_script, LIMIT_BYTES, LOOSE_JOB_LIMIT_BYTES);
        assert_eq!(
            process_limited.status.code(),
            Some(0),
            "the child must catch the independently enforced per-process refusal"
        );
        assert_eq!(
            process_limited.stdout, b"started|refused",
            "the loose whole-Job limit must leave the per-process limit as the refusing control"
        );

        let directory = tempfile::TempDir::new().unwrap();
        let holder_path = directory.path().join("memory-holder.ps1");
        let probe_path = directory.path().join("memory-probe.ps1");
        let ready_path = directory.path().join("memory-holder.ready");
        std::fs::write(
            &holder_path,
            format!(
                "$ErrorActionPreference = 'Stop'\n\
                 $bytes = [byte[]]::new({DESCENDANT_ALLOCATION_BYTES})\n\
                 [IO.File]::WriteAllText('{}', 'allocated')\n\
                 Start-Sleep -Seconds 15\n",
                quote_powershell_literal(&ready_path)
            ),
        )
        .unwrap();
        std::fs::write(
            &probe_path,
            format!(
                "$ErrorActionPreference = 'Stop'\n\
                 try {{\n\
                     $bytes = [byte[]]::new({DESCENDANT_ALLOCATION_BYTES})\n\
                     exit 91\n\
                 }} catch {{\n\
                     $base = $_.Exception.GetBaseException()\n\
                     if ($base -is [System.OutOfMemoryException]) {{ exit 0 }}\n\
                     [Console]::Error.Write($base.GetType().FullName + ': ' + $base.Message)\n\
                     exit 92\n\
                 }}\n"
            ),
        )
        .unwrap();
        let aggregate_script = format!(
            "$ErrorActionPreference = 'Stop'; \
             $holder = Start-Process -FilePath 'powershell.exe' \
                 -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-File','{}') \
                 -WindowStyle Hidden -PassThru; \
             try {{ \
                 $deadline = [DateTime]::UtcNow.AddSeconds(10); \
                 while (-not [IO.File]::Exists('{}')) {{ \
                     if ($holder.HasExited) {{ throw 'memory holder exited before allocating' }} \
                     if ([DateTime]::UtcNow -ge $deadline) {{ throw 'memory holder did not become ready' }} \
                     Start-Sleep -Milliseconds 25; \
                 }} \
                 $probe = Start-Process -FilePath 'powershell.exe' \
                     -ArgumentList @('-NoLogo','-NoProfile','-NonInteractive','-File','{}') \
                     -WindowStyle Hidden -Wait -PassThru; \
                 [Console]::Out.Write('holder|' + [IO.File]::ReadAllText('{}') + \
                     '|probe-exit:' + [string]$probe.ExitCode); \
                 exit $probe.ExitCode; \
             }} finally {{ \
                 if (-not $holder.HasExited) {{ Stop-Process -Id $holder.Id -Force }} \
                 $holder.WaitForExit(); \
             }}",
            quote_powershell_literal(&holder_path),
            quote_powershell_literal(&ready_path),
            quote_powershell_literal(&probe_path),
            quote_powershell_literal(&ready_path),
        );

        let aggregate_control = run(
            &mut powershell(&aggregate_script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            // This script starts PowerShell, which starts a holder process,
            // waits on a ready file, then starts a probe process, and both
            // children allocate. That is three PowerShell startups plus two
            // large allocations, and on a loaded Windows runner 20s was not
            // enough headroom: the wall clock, not the Job memory ceiling
            // under test, is what tripped.
            budget(60_000),
        )
        .expect("the unbounded aggregate allocation control must launch");
        assert!(
            !aggregate_control.timed_out,
            "the control tree exceeded its wall clock before the memory behaviour \
             under test could be observed; this is a harness budget problem, not a \
             Job memory ceiling result"
        );
        assert_eq!(
            aggregate_control.output.status.code(),
            Some(91),
            "both descendant allocations must succeed without a Job memory ceiling"
        );
        assert_eq!(
            aggregate_control.output.stdout,
            b"holder|allocated|probe-exit:91",
            "the unbounded aggregate control must keep the first allocation alive through the second"
        );

        std::fs::remove_file(&ready_path).unwrap();
        let mut aggregate_limited = powershell(&aggregate_script);
        aggregate_limited.address_space_limit(LIMIT_BYTES);
        let aggregate_limited = run(
            &mut aggregate_limited,
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(60_000),
        )
        .expect("the aggregate-limited process tree must launch");
        assert!(
            !aggregate_limited.timed_out,
            "the limited tree exceeded its wall clock before the Job refusal could \
             be observed; this is a harness budget problem, not a memory result"
        );
        assert_eq!(
            aggregate_limited.output.status.code(),
            Some(0),
            "the second under-process-budget allocation must catch the whole-Job refusal"
        );
        assert_eq!(
            aggregate_limited.output.stdout,
            b"holder|allocated|probe-exit:0",
            "the first under-budget descendant must stay committed while the whole-Job limit refuses the second"
        );
    }

    #[test]
    fn timeout_kills_windows_child_and_grandchild_tree() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("timeout-grandchild.pid");
        let script = child_and_grandchild_script(&pid_path, "Start-Sleep -Seconds 30");
        let started = Instant::now();

        let run = run(
            &mut powershell(&script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(10_000),
        )
        .unwrap();

        assert!(run.timed_out);
        assert!(started.elapsed() < Duration::from_secs(20));
        assert_process_tree_retired(read_pid(&pid_path));
    }

    #[test]
    fn successful_windows_leader_exit_retires_grandchild_and_preserves_status() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("successful-grandchild.pid");
        let ready_path = directory.path().join("successful-leader-ready");
        let leader_tail = format!(
            "[IO.File]::WriteAllText('{}', 'ready'); \
             [Console]::Out.Write('leader-output'); exit 7",
            quote_powershell_literal(&ready_path)
        );
        let script = child_and_grandchild_script(&pid_path, &leader_tail);

        // Hosted PowerShell cold-start can consume most of a wall-clock
        // budget under runner contention. Measure the ordering contract from
        // the leader's pre-exit marker instead: tree retirement and pipe drain
        // must be prompt once the leader is ready to exit.
        let runner = std::thread::spawn(move || {
            run(
                &mut powershell(&script),
                None,
                StdoutTarget::Capture { max_bytes: 1024 },
                budget(30_000),
            )
        });
        let ready_deadline = Instant::now() + Duration::from_secs(30);
        while !ready_path.exists() && !runner.is_finished() && Instant::now() < ready_deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let ready_observed = ready_path.exists();
        let retirement_started = Instant::now();
        let run = runner
            .join()
            .expect("supervisor thread must not panic")
            .unwrap();
        let retirement_elapsed = retirement_started.elapsed();

        assert!(
            ready_observed,
            "successful leader must publish its pre-exit marker"
        );
        assert!(!run.timed_out);
        assert!(
            retirement_elapsed < Duration::from_secs(10),
            "successful leader retirement took {retirement_elapsed:?} after its pre-exit marker"
        );
        assert_eq!(run.output.status.code(), Some(7));
        assert_eq!(run.output.stdout, b"leader-output");
        assert_process_tree_retired(read_pid(&pid_path));
    }

    #[test]
    fn windows_stdout_overflow_retires_child_and_grandchild_tree() {
        let directory = tempfile::TempDir::new().unwrap();
        let pid_path = directory.path().join("overflow-grandchild.pid");
        let script = child_and_grandchild_script(
            &pid_path,
            "while ($true) { [Console]::Out.Write('0123456789abcdef') }",
        );

        let error = run(
            &mut powershell(&script),
            None,
            StdoutTarget::Capture { max_bytes: 1024 },
            budget(5_000),
        )
        .unwrap_err();

        assert!(error.to_string().contains("stdout resource budget"));
        assert_process_tree_retired(read_pid(&pid_path));
    }
}
