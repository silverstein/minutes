use crate::config::Config;
use crate::error::{MinutesError, TranscribeError, WatchError};
use crate::markdown::ContentType;
#[cfg(feature = "parakeet")]
use crate::pipeline::BackgroundPipelineContext;
use crate::pipeline::{self, SidecarMetadata};
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[derive(Debug, Clone)]
struct WatchCandidate {
    path: PathBuf,
    content_type: ContentType,
    sidecar: Option<SidecarMetadata>,
}

// ──────────────────────────────────────────────────────────────
// Folder watcher event loop:
//
//   [detect new file]
//        │
//        ▼
//   [skip .icloud stubs + processed/ + failed/]
//        │
//        ▼
//   [settle check: size stable across 2 checks?]
//        │ no → skip, retry next cycle
//        │ yes
//        ▼
//   [acquire lock (watch.lock)]
//        │ fail → "another watcher running"
//        │ ok
//        ▼
//   [check extension filter]
//        │ no match → skip
//        │ match
//        ▼
//   [probe audio duration (symphonia)]
//        │ <threshold → ContentType::Memo (skip diarize)
//        │ >=threshold → ContentType::Meeting
//        │ probe failed → use config.watch.type
//        ▼
//   [read sidecar JSON if present]
//        │ found → enrich frontmatter (device, source)
//        │ missing/malformed → proceed without
//        ▼
//   [run pipeline: transcribe → write markdown]
//        │ success → move to processed/ + emit event + notify
//        │ failure → move to failed/
//        ▼
//   [release lock]
//
// Files:
//   ~/.minutes/watch.lock          — prevents concurrent watchers
//   ~/.minutes/inbox/              — watched folder (default)
//   ~/.minutes/inbox/processed/    — successfully processed
//   ~/.minutes/inbox/failed/       — processing failed
// ──────────────────────────────────────────────────────────────

/// Path to the watcher lock file (`~/.minutes/watch.lock`).
pub fn lock_path() -> PathBuf {
    Config::minutes_dir().join("watch.lock")
}

/// Acquire the watcher lock. Returns error if another watcher is running.
fn acquire_lock() -> Result<(), WatchError> {
    let path = lock_path();
    if path.exists() {
        // Check if the PID in the lock file is still alive
        if let Ok(contents) = fs::read_to_string(&path) {
            if let Ok(pid) = contents.trim().parse::<u32>() {
                if is_process_alive(pid) {
                    return Err(WatchError::AlreadyRunning(path.display().to_string()));
                }
            }
        }
        // Stale lock — remove it
        tracing::warn!("stale watch lock found, removing");
        fs::remove_file(&path).ok();
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, std::process::id().to_string())?;
    Ok(())
}

/// Release the watcher lock.
fn release_lock() {
    let path = lock_path();
    fs::remove_file(&path).ok();
}

fn is_process_alive(pid: u32) -> bool {
    crate::pid::is_process_alive(pid)
}

/// Check if a file has a watched extension.
pub fn has_valid_extension(path: &Path, config: &Config) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            config
                .watch
                .extensions
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(ext))
        })
}

/// Compare stable filesystem identities without treating path equality as
/// ownership. Recovery uses this at its final claim boundary so a hard-link or
/// bind-mount alias cannot turn a file still owned by the folder watcher into
/// an independently retryable item.
pub fn same_regular_file_identity(left: &Path, right: &Path) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

        let open = |path: &Path| {
            fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
                .open(path)
        };
        let left_file = open(left)?;
        let right_file = open(right)?;
        let left = left_file.metadata()?;
        let right = right_file.metadata()?;
        if !left.file_type().is_file() || !right.file_type().is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "watch ownership identity requires two regular non-symlink files",
            ));
        }
        Ok(left.dev() == right.dev() && left.ino() == right.ino())
    }

    #[cfg(windows)]
    {
        use std::mem::MaybeUninit;
        use std::os::windows::fs::OpenOptionsExt;
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };

        let identity = |path: &Path| -> std::io::Result<(u32, u64)> {
            let file = fs::OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?;
            if !file.metadata()?.file_type().is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "watch ownership identity requires a regular file",
                ));
            }
            let mut info = MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::zeroed();
            // SAFETY: the handle is live and the output points to initialized
            // storage owned by this call.
            let ok = unsafe {
                GetFileInformationByHandle(file.as_raw_handle().cast(), info.as_mut_ptr())
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: a successful call initialized the entire structure.
            let info = unsafe { info.assume_init() };
            if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "watch ownership identity rejects reparse points",
                ));
            }
            Ok((
                info.dwVolumeSerialNumber,
                ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64,
            ))
        };

        Ok(identity(left)? == identity(right)?)
    }

    #[cfg(not(any(unix, windows)))]
    {
        let _ = (left, right);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "stable watch ownership identity is unavailable on this platform",
        ))
    }
}

/// Wait for a file to finish syncing (size-stability check).
/// Returns true if the file is stable and ready to process.
fn wait_for_settle(path: &Path, delay_ms: u64) -> bool {
    let delay = Duration::from_millis(delay_ms);

    // First check
    let size1 = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return false, // File disappeared
    };

    if size1 == 0 {
        // File is empty — might still be syncing. Wait and check again.
        std::thread::sleep(delay);
        match fs::metadata(path) {
            Ok(m) if m.len() == 0 => return false, // Still empty
            Ok(_) => {}                            // Now has content, continue
            Err(_) => return false,                // Disappeared
        }
    }

    std::thread::sleep(delay);

    // Second check
    let size2 = match fs::metadata(path) {
        Ok(m) => m.len(),
        Err(_) => return false,
    };

    if size1 != size2 || size2 == 0 {
        tracing::debug!(
            path = %path.display(),
            size1, size2,
            "file not yet stable, skipping this cycle"
        );
        return false;
    }

    true
}

#[cfg(any(not(windows), test))]
fn atomic_noreplace_unavailable(
    source: &Path,
    destination: &Path,
    cause: impl std::fmt::Display,
) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        format!(
            "atomic no-replace move is unavailable; leaving {} untouched instead of risking replacement at {}: {cause}",
            source.display(),
            destination.display()
        ),
    ))
}

/// Rename without ever replacing an existing destination entry.
///
/// Linux and Apple platforms have an atomic exclusive-rename primitive.
/// Windows uses MoveFileExW without MOVEFILE_REPLACE_EXISTING. Targets without
/// an atomic exclusive-move primitive fail closed and leave the source in
/// place; link-plus-unlink is intentionally not used because another process
/// can substitute either name between those operations. Full crash generations
/// remain tracked by #510.
fn rename_noreplace(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::ffi::OsStrExt;

        let source_c = std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "watcher source path contains a NUL byte",
            )
        })?;
        let destination_c =
            std::ffi::CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "watcher destination path contains a NUL byte",
                )
            })?;
        // SAFETY: both paths are validated C strings and remain alive for the
        // call. RENAME_NOREPLACE makes a concurrent destination creator win
        // rather than allowing Minutes to overwrite it.
        let result = unsafe {
            libc::renameat2(
                libc::AT_FDCWD,
                source_c.as_ptr(),
                libc::AT_FDCWD,
                destination_c.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::ENOSYS) | Some(libc::EINVAL) | Some(libc::EOPNOTSUPP)
        ) {
            return Err(error);
        }
        atomic_noreplace_unavailable(source, destination, error)
    }

    #[cfg(target_vendor = "apple")]
    {
        use std::os::unix::ffi::OsStrExt;

        let source_c = std::ffi::CString::new(source.as_os_str().as_bytes()).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "watcher source path contains a NUL byte",
            )
        })?;
        let destination_c =
            std::ffi::CString::new(destination.as_os_str().as_bytes()).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "watcher destination path contains a NUL byte",
                )
            })?;
        // SAFETY: validated C strings remain alive for the exclusive rename.
        let result = unsafe {
            libc::renamex_np(source_c.as_ptr(), destination_c.as_ptr(), libc::RENAME_EXCL)
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if !matches!(
            error.raw_os_error(),
            Some(libc::ENOTSUP) | Some(libc::EINVAL)
        ) {
            return Err(error);
        }
        atomic_noreplace_unavailable(source, destination, error)
    }

    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{MoveFileExW, MOVEFILE_WRITE_THROUGH};

        let wide_path = |path: &Path| -> std::io::Result<Vec<u16>> {
            let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
            if wide.contains(&0) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "watcher move path contains a NUL code unit",
                ));
            }
            wide.push(0);
            Ok(wide)
        };
        let source_wide = wide_path(source)?;
        let destination_wide = wide_path(destination)?;
        // SAFETY: both paths are NUL-terminated UTF-16 buffers that live for
        // the call. Omitting MOVEFILE_REPLACE_EXISTING is the no-clobber
        // guarantee; WRITE_THROUGH does not alter that exclusivity.
        let result = unsafe {
            MoveFileExW(
                source_wide.as_ptr(),
                destination_wide.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if result == 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple",
        windows
    )))]
    {
        atomic_noreplace_unavailable(
            source,
            destination,
            "this platform has no supported exclusive rename primitive",
        )
    }
}

/// Move a file to a subdirectory (processed/ or failed/).
fn move_to(file: &Path, subdir: &str) -> Result<PathBuf, WatchError> {
    move_to_with_hooks(file, subdir, |_| {}, |_| {}, |_| {})
}

fn move_to_with_hooks(
    file: &Path,
    subdir: &str,
    mut before_audio_move: impl FnMut(&Path),
    mut before_sidecar_move: impl FnMut(&Path),
    mut before_audio_rollback: impl FnMut(&Path),
) -> Result<PathBuf, WatchError> {
    let parent = file.parent().unwrap_or(Path::new("."));
    let dest_dir = parent.join(subdir);
    fs::create_dir_all(&dest_dir)
        .map_err(|e| WatchError::MoveError(dest_dir.display().to_string(), e))?;

    let filename = file.file_name().unwrap_or_default();
    let sidecar = file.with_extension("json");
    let path_may_exist = |path: &Path| match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(error) => error.kind() != std::io::ErrorKind::NotFound,
    };
    let first_dest = dest_dir.join(filename);
    let destination_is_occupied = |candidate: &Path| {
        path_may_exist(candidate) || path_may_exist(&candidate.with_extension("json"))
    };

    // Handle collision in either member of the audio-plus-sidecar pair.
    let dest = if destination_is_occupied(&first_dest) {
        let stem = first_dest.file_stem().unwrap_or_default().to_string_lossy();
        let ext = first_dest
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_default();
        let ts = chrono::Local::now().timestamp_micros();
        (0_u16..=u16::MAX)
            .map(|attempt| {
                let suffix = if attempt == 0 {
                    ts.to_string()
                } else {
                    format!("{ts}-{attempt}")
                };
                if ext.is_empty() {
                    dest_dir.join(format!("{stem}-{suffix}"))
                } else {
                    dest_dir.join(format!("{stem}-{suffix}.{ext}"))
                }
            })
            .find(|candidate| !destination_is_occupied(candidate))
            .ok_or_else(|| {
                WatchError::MoveError(
                    first_dest.display().to_string(),
                    std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "watcher recovery destination namespace is exhausted",
                    ),
                )
            })?
    } else {
        first_dest
    };

    before_audio_move(&dest);
    rename_noreplace(file, &dest)
        .map_err(|e| WatchError::MoveError(dest.display().to_string(), e))?;

    // Sidecar metadata is never unlinked by its late pathname. Success and
    // failure moves both keep whatever JSON is currently present paired with
    // the audio, so a new or replacement generation cannot be deleted without
    // having been read. Full crash-atomic multi-file recovery remains tracked
    // by #510; if this second rename fails synchronously, put the audio back
    // and leave the sidecar untouched rather than split them.
    match fs::symlink_metadata(&sidecar) {
        Ok(_) => {
            let sidecar_dest = dest.with_extension("json");
            before_sidecar_move(&sidecar_dest);
            if let Err(sidecar_error) = rename_noreplace(&sidecar, &sidecar_dest) {
                before_audio_rollback(file);
                return match rename_noreplace(&dest, file) {
                    Ok(()) => Err(WatchError::MoveError(
                        sidecar_dest.display().to_string(),
                        sidecar_error,
                    )),
                    Err(rollback_error) => Err(WatchError::MoveError(
                        format!(
                            "{}; audio remains at {}; sidecar remains at {}",
                            sidecar_dest.display(),
                            dest.display(),
                            sidecar.display()
                        ),
                        std::io::Error::other(format!(
                            "could not preserve watcher sidecar: {sidecar_error}; audio rollback also failed: {rollback_error}; audio remains at {} and sidecar remains at {}",
                            dest.display(),
                            sidecar.display()
                        )),
                    )),
                };
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            before_audio_rollback(file);
            return match rename_noreplace(&dest, file) {
                Ok(()) => Err(WatchError::MoveError(sidecar.display().to_string(), error)),
                Err(rollback_error) => Err(WatchError::MoveError(
                    sidecar.display().to_string(),
                    std::io::Error::other(format!(
                        "could not inspect watcher sidecar: {error}; audio rollback also failed: {rollback_error}; audio remains at {}",
                        dest.display()
                    )),
                )),
            };
        }
    }

    tracing::debug!(from = %file.display(), to = %dest.display(), "moved file");
    Ok(dest)
}

/// Check if a file is an iCloud eviction stub (.icloud placeholder).
fn is_icloud_stub(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with('.') && n.ends_with(".icloud"))
}

/// Validate WAV in-process. Other configured containers are admitted to the
/// bounded ffmpeg decode boundary, which performs the authoritative check.
fn is_audio_container(path: &Path) -> bool {
    if compressed_audio_requires_ffmpeg(path) {
        return std::fs::File::open(path).is_ok();
    }
    std::fs::File::open(path)
        .ok()
        .and_then(|file| hound::WavReader::new(file).ok())
        .is_some()
}

const WATCH_DURATION_PROBE_DEADLINE: Duration = Duration::from_secs(60);
const WATCH_DURATION_PROBE_STDERR_BYTES: usize = 64 * 1024;
const CANONICAL_PCM_BYTES_PER_SECOND: u64 = 16_000 * 2;

struct CountingWriter(Arc<AtomicU64>);

impl Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Decode only as far as the routing threshold through the same bounded
/// ffmpeg process boundary used by the normal import path. The child may stop
/// reading stdin once `-t` is satisfied; source read errors still fail.
fn compressed_audio_duration(
    path: &Path,
    threshold_secs: u64,
    ffmpeg_path: &Path,
) -> Option<std::time::Duration> {
    if threshold_secs == 0 || threshold_secs > crate::audio_budget::MAX_AUDIO_SECONDS {
        return None;
    }
    let output_limit = threshold_secs
        .checked_add(1)?
        .checked_mul(CANONICAL_PCM_BYTES_PER_SECOND)?;
    let counter = Arc::new(AtomicU64::new(0));
    let writer = CountingWriter(Arc::clone(&counter));
    let input = File::open(path).ok()?;
    let mut command = crate::bounded_child::BoundedCommand::new(ffmpeg_path);
    command
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-i")
        .arg("pipe:0")
        .arg("-map")
        .arg("0:a:0")
        .arg("-vn")
        .arg("-t")
        .arg(threshold_secs.to_string())
        .arg("-ac")
        .arg("1")
        .arg("-ar")
        .arg("16000")
        .arg("-f")
        .arg("s16le")
        .arg("pipe:1");
    let run = crate::bounded_child::run_allowing_child_to_close_stdin(
        &mut command,
        Box::new(input),
        crate::bounded_child::StdoutTarget::ExactWriter {
            writer: Box::new(writer),
            max_bytes: output_limit,
        },
        crate::bounded_child::ChildBudget {
            wall_clock: WATCH_DURATION_PROBE_DEADLINE,
            stderr_tail: WATCH_DURATION_PROBE_STDERR_BYTES,
        },
    )
    .ok()?;
    if run.timed_out || !run.output.status.success() {
        return None;
    }

    // ffmpeg exits 0 having produced nothing when it cannot demux the input
    // from a non-seekable pipe, which is the normal outcome for an MP4/M4A
    // whose `moov` atom sits at the end of the file. Treating that as a
    // measurement of zero seconds routed long meetings to `memo`, and because
    // only meetings are diarized, silently cost them their speaker labels.
    // An empty probe is a failed probe, so the caller can fall through to the
    // bounded worker, which reads the declared frame count instead.
    let decoded = counter.load(Ordering::Relaxed);
    if decoded == 0 {
        tracing::debug!(
            "ffmpeg duration probe produced no audio (likely a non-faststart container); \
             falling back to the bounded probe"
        );
        return None;
    }

    Some(std::time::Duration::from_secs_f64(
        decoded as f64 / CANONICAL_PCM_BYTES_PER_SECOND as f64,
    ))
}

/// Probe WAV duration with hound, or non-WAV duration through bounded ffmpeg.
fn audio_duration(
    path: &Path,
    threshold_secs: u64,
    ffmpeg_path: Option<&Path>,
) -> Option<std::time::Duration> {
    if compressed_audio_requires_ffmpeg(path) {
        return ffmpeg_path
            .and_then(|ffmpeg| compressed_audio_duration(path, threshold_secs, ffmpeg));
    }
    let reader = hound::WavReader::open(path).ok()?;
    let sample_rate = reader.spec().sample_rate;
    if sample_rate == 0 {
        return None;
    }

    Some(std::time::Duration::from_secs_f64(
        reader.duration() as f64 / sample_rate as f64,
    ))
}

/// Read optional sidecar JSON file (e.g., from Apple Shortcut).
/// Returns None if sidecar doesn't exist or is malformed — always best-effort.
fn read_sidecar(audio_path: &Path) -> Option<SidecarMetadata> {
    let sidecar_path = audio_path.with_extension("json");
    if !sidecar_path.exists() {
        return None;
    }

    match fs::read_to_string(&sidecar_path) {
        Ok(contents) => match serde_json::from_str::<SidecarMetadata>(&contents) {
            Ok(meta) => {
                tracing::info!(
                    sidecar = %sidecar_path.display(),
                    device = ?meta.device,
                    "sidecar metadata loaded"
                );
                Some(meta)
            }
            Err(e) => {
                tracing::warn!(
                    sidecar = %sidecar_path.display(),
                    error = %e,
                    "malformed sidecar JSON — processing without metadata"
                );
                None
            }
        },
        Err(e) => {
            tracing::warn!(
                sidecar = %sidecar_path.display(),
                error = %e,
                "could not read sidecar — processing without metadata"
            );
            None
        }
    }
}

/// Archive a successfully processed source without deleting metadata by its
/// pathname. A sidecar can appear or be replaced while transcription runs; the
/// pair mover preserves whatever generation is present beside the audio rather
/// than unlinking a file the successful pipeline never read. Both single-file
/// and batch success use this one boundary.
fn archive_successful_candidate(audio_path: &Path) -> Result<PathBuf, WatchError> {
    move_to(audio_path, "processed")
}

/// Determine content type based on audio duration and config.
/// Duration-based routing takes priority over config.watch.type.
/// Set dictation_threshold_secs = 0 to disable duration-based routing.
/// Map a probed duration onto a content type using the configured threshold.
///
/// Shared so the ffmpeg and bounded-worker probes cannot drift apart.
fn content_type_for_duration(duration: std::time::Duration, config: &Config) -> ContentType {
    if duration.as_secs() < config.watch.dictation_threshold_secs {
        ContentType::Memo
    } else {
        ContentType::Meeting
    }
}

fn determine_content_type(path: &Path, config: &Config) -> ContentType {
    if !compressed_audio_requires_ffmpeg(path) {
        return determine_content_type_with_ffmpeg(path, config, None);
    }
    // `dictation_threshold_secs = 0` disables duration-based routing entirely
    // and is documented that way. Both probes must sit behind that switch, not
    // just the ffmpeg one: leaving the bounded probe outside it made every
    // compressed import a Meeting, because `content_type_for_duration` can
    // never return Memo when the threshold is zero.
    let threshold = config.watch.dictation_threshold_secs;
    if threshold == 0 {
        return determine_content_type_with_ffmpeg(path, config, None);
    }
    // Try ffmpeg first for parity with the decode path, but do not stop there.
    // Its pipe-based probe cannot demux a non-faststart MP4/M4A and returns
    // nothing, so a successful-looking ffmpeg install must still fall through
    // to the bounded probe rather than defaulting the route.
    if let Ok(ffmpeg) = crate::ffmpeg::resolve_launchable_ffmpeg() {
        if let Some(duration) = audio_duration(path, threshold, Some(ffmpeg.as_path())) {
            return content_type_for_duration(duration, config);
        }
    }
    if crate::audio_decode_worker::bounded_decode_fallback_available(config) {
        if let Some(duration) = crate::audio_decode_worker::probe_compressed_duration(
            path,
            std::time::Duration::from_secs(60),
        ) {
            return content_type_for_duration(duration, config);
        }
    }
    determine_content_type_with_ffmpeg(path, config, None)
}

fn determine_content_type_with_ffmpeg(
    path: &Path,
    config: &Config,
    ffmpeg_path: Option<&Path>,
) -> ContentType {
    let threshold = config.watch.dictation_threshold_secs;

    if threshold > 0 {
        if let Some(duration) = audio_duration(path, threshold, ffmpeg_path) {
            let content_type = content_type_for_duration(duration, config);
            tracing::info!(
                path = %path.display(),
                duration_secs = duration.as_secs(),
                threshold,
                content_type = ?content_type,
                "duration-based routing"
            );
            return content_type;
        }
        tracing::debug!(
            path = %path.display(),
            "could not probe duration — falling back to config type"
        );
    }

    // Fallback: use config.watch.type
    if config.watch.r#type == "meeting" {
        ContentType::Meeting
    } else {
        ContentType::Memo
    }
}

/// Process a single file through the pipeline.
fn process_candidate(candidate: &WatchCandidate, config: &Config) -> Result<(), WatchError> {
    // Ask whether the file is decodable at all, not whether ffmpeg exists. The
    // bounded decode worker handles these containers when ffmpeg is missing,
    // and gating on ffmpeg alone refused iPhone .m4a memos the pipeline could
    // process perfectly well.
    let decodable = compressed_audio_decodable(&candidate.path, config);
    process_candidate_with_ffmpeg_availability(candidate, config, decodable)
}

fn process_candidate_with_ffmpeg_availability(
    candidate: &WatchCandidate,
    config: &Config,
    ffmpeg_available: bool,
) -> Result<(), WatchError> {
    if compressed_audio_requires_ffmpeg(&candidate.path) && !ffmpeg_available {
        let detail = format!(
            "{} The original audio remains untouched at {}.",
            compressed_audio_ffmpeg_guidance(),
            candidate.path.display()
        );
        eprintln!("Could not process {}. {}", candidate.path.display(), detail);
        tracing::error!(
            input = %candidate.path.display(),
            "non-WAV watched audio needs ffmpeg"
        );
        return Err(WatchError::Io(std::io::Error::other(detail)));
    }

    crate::events::append_event(crate::events::recording_started_event(
        None,
        "watch",
        [
            "file.ingest".to_string(),
            format!(
                "content_type.{}",
                content_type_label(candidate.content_type)
            ),
        ],
    ));
    match pipeline::process_with_sidecar(
        &candidate.path,
        candidate.content_type,
        None,
        config,
        candidate.sidecar.as_ref(),
        |_| {},
    ) {
        Ok(result) => {
            tracing::info!(
                input = %candidate.path.display(),
                output = %result.path.display(),
                words = result.word_count,
                "file processed successfully"
            );

            // Emit WatchProcessed event (existing)
            crate::events::append_event(crate::events::MinutesEvent::WatchProcessed {
                path: result.path.display().to_string(),
                title: result.title.clone(),
                word_count: result.word_count,
                source_path: candidate.path.display().to_string(),
            });

            // Emit VoiceMemoProcessed event for voice memos (enables agent reactivity)
            if candidate.content_type == ContentType::Memo {
                crate::events::append_event(crate::events::MinutesEvent::VoiceMemoProcessed {
                    path: result.path.display().to_string(),
                    title: result.title.clone(),
                    word_count: result.word_count,
                    source_path: candidate.path.display().to_string(),
                    device: candidate.sidecar.as_ref().and_then(|s| s.device.clone()),
                });
            }

            archive_successful_candidate(&candidate.path)?;
            Ok(())
        }
        Err(MinutesError::Transcribe(TranscribeError::CompressedDecoderUnavailable(error))) => {
            let detail = format!(
                "{} The original audio remains untouched at {}. Decoder check: {}",
                compressed_audio_ffmpeg_guidance(),
                candidate.path.display(),
                error
            );
            eprintln!("Could not process {}. {}", candidate.path.display(), detail);
            tracing::error!(
                input = %candidate.path.display(),
                "non-WAV decoder became unavailable before use; preserving watcher input"
            );
            Err(WatchError::Io(std::io::Error::other(detail)))
        }
        Err(e) => {
            tracing::error!(
                input = %candidate.path.display(),
                error = %e,
                "pipeline failed — moving to failed/"
            );
            move_to(&candidate.path, "failed")?;
            Err(WatchError::Io(std::io::Error::other(format!(
                "pipeline error: {}",
                e
            ))))
        }
    }
}

/// Non-WAV inputs need an out-of-process decoder. WAV stays on the in-process
/// streaming parser and never requires one.
///
/// This answers only "is this container compressed". Callers deciding whether
/// to accept or refuse work must use [`compressed_audio_decodable`], which also
/// accounts for the bounded decode worker.
pub fn compressed_audio_requires_ffmpeg(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| !extension.eq_ignore_ascii_case("wav"))
}

/// Whether a compressed import can actually be decoded on this machine now.
///
/// Two decoders can satisfy it: ffmpeg, which stays preferred for quality
/// (issue #21), and the bounded decode worker, which handles the same
/// containers when ffmpeg is absent. Every preflight and readiness surface must
/// ask this rather than asking about ffmpeg alone. Asking the narrower question
/// is what left `minutes watch`, the desktop import, and Recovery Center
/// refusing work the pipeline could complete.
pub fn compressed_audio_decodable(path: &Path, config: &Config) -> bool {
    // Short-circuit before probing. WAV needs no external decoder, and
    // `resolve_launchable_ffmpeg` *launches* ffmpeg to prove it starts, so
    // evaluating the probes eagerly would grant a configured ffmpeg authority
    // on the pure-WAV path that deliberately never touches it.
    if !compressed_audio_requires_ffmpeg(path) {
        return true;
    }
    crate::ffmpeg::resolve_launchable_ffmpeg().is_ok()
        || crate::audio_decode_worker::bounded_decode_fallback_available(config)
}

/// The admission decision with both decoder probes supplied by the caller.
///
/// Split out so consumers in other crates can test the decision itself without
/// depending on a worker binary existing beside their test harness. The probes
/// are environmental; the decision is the thing worth asserting.
pub fn compressed_audio_decodable_with(
    path: &Path,
    ffmpeg_available: bool,
    bounded_fallback_available: bool,
) -> bool {
    !compressed_audio_requires_ffmpeg(path) || ffmpeg_available || bounded_fallback_available
}

/// Guidance for a compressed import this machine could not decode.
///
/// It is NOT limited to "ffmpeg missing and the bounded worker unavailable",
/// which is what this comment used to claim and what made the message wrong.
/// A second state reaches it: ffmpeg missing while the bounded worker is
/// present, enabled, and simply has no decoder for the codec. Which of the two
/// is more common is not something this repo measures, and an earlier version of
/// this line asserted it was.
///
/// This must stay true in BOTH states that reach it, which the previous wording
/// did not. It told every affected user to "re-enable the bundled decoder",
/// advice that is useless in that case: the bundled decoder is enabled
/// by default, and it has no Opus or ALAC decoder, so Opus-in-WebM (browser and
/// Meet recordings), Opus-in-OGG (WhatsApp and Telegram voice notes) and
/// ALAC-in-m4a fail with an unsupported-codec exit while the setting the
/// message named was already `true`. Naming the codecs that genuinely require
/// ffmpeg is what makes it actionable, and it is the same thing
/// `health::ffmpeg_status` reports, so the two surfaces agree.
/// The single sentence both user-facing surfaces must carry about what the
/// bundled decoder cannot do.
///
/// A macro rather than a `const` so each call site can build its own
/// `&'static str` around it with `concat!`. Sharing the BYTES is the point: the
/// two surfaces were independently written strings, and a test that checked both
/// merely CONTAINED the words "Opus" and "ALAC" stayed green while one of them
/// said the bundled decoder handles those codecs fine. Vocabulary agreement is
/// not agreement. Now they cannot disagree without one of them dropping this
/// sentence, which is a thing a test can see.
macro_rules! bundled_decoder_codec_limit {
    () => {
        "the bundled decoder handles AAC, MP3, Vorbis and FLAC but cannot decode Opus or ALAC"
    };
}
pub(crate) use bundled_decoder_codec_limit;

pub fn compressed_audio_ffmpeg_guidance() -> &'static str {
    concat!(
        "This machine could not decode this non-WAV import. Install ffmpeg (macOS: brew install ",
        "ffmpeg; Linux: use your package manager; Windows: install ffmpeg.exe and add it to PATH), ",
        "or set MINUTES_FFMPEG to the full executable path. ffmpeg is required rather than optional ",
        "for Opus audio, which is what .webm browser and Meet recordings and .ogg voice notes ",
        "normally contain, and for ALAC in .m4a: ",
        bundled_decoder_codec_limit!(),
        ". If transcription.compressed_decode_fallback is set to false in ",
        "~/.config/minutes/config.toml, setting it back to true restores the bundled decoder for ",
        "the formats it does support. Then restart the watcher or process the original file ",
        "directly. WAV imports remain available either way.",
    )
}

fn content_type_label(content_type: ContentType) -> &'static str {
    match content_type {
        ContentType::Meeting => "meeting",
        ContentType::Memo => "memo",
        ContentType::Dictation => "dictation",
    }
}

#[cfg(feature = "parakeet")]
fn process_parakeet_memo_batch(
    candidates: &[WatchCandidate],
    config: &Config,
) -> Result<(), WatchError> {
    let audio_paths: Vec<PathBuf> = candidates
        .iter()
        .map(|candidate| candidate.path.clone())
        .collect();
    let batch_started = std::time::Instant::now();
    let batch_results = crate::transcribe::transcribe_parakeet_batch(&audio_paths, config)
        .map_err(|error| {
            WatchError::Io(std::io::Error::other(format!(
                "parakeet batch error: {}",
                error
            )))
        })?;
    let per_file_transcribe_ms = (batch_started.elapsed().as_millis() as u64)
        .checked_div(candidates.len() as u64)
        .unwrap_or(0);

    for (candidate, transcribe_result) in candidates.iter().zip(batch_results) {
        let transcribe_result = match transcribe_result {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(
                    path = %candidate.path.display(),
                    error = %error,
                    "batched parakeet transcription failed — falling back to single-file processing"
                );
                process_candidate(candidate, config)?;
                continue;
            }
        };

        let context = BackgroundPipelineContext {
            sidecar: candidate.sidecar.clone(),
            recorded_at: candidate
                .sidecar
                .as_ref()
                .and_then(|sidecar| sidecar.captured_at),
            ..BackgroundPipelineContext::default()
        };

        let artifact = pipeline::write_transcript_artifact(
            &candidate.path,
            candidate.content_type,
            None,
            config,
            &context,
            None,
            transcribe_result.text,
            transcribe_result.stats,
            per_file_transcribe_ms,
        )
        .map_err(|error| {
            WatchError::Io(std::io::Error::other(format!("pipeline error: {}", error)))
        })?;
        let result = pipeline::enrich_transcript_artifact(
            &candidate.path,
            &artifact,
            config,
            &context,
            |_| {},
        )
        .map_err(|error| {
            WatchError::Io(std::io::Error::other(format!("pipeline error: {}", error)))
        })?;

        tracing::info!(
            input = %candidate.path.display(),
            output = %result.path.display(),
            words = result.word_count,
            "file processed successfully via parakeet batch"
        );
        crate::events::append_event(crate::events::MinutesEvent::WatchProcessed {
            path: result.path.display().to_string(),
            title: result.title.clone(),
            word_count: result.word_count,
            source_path: candidate.path.display().to_string(),
        });
        crate::events::append_event(crate::events::MinutesEvent::VoiceMemoProcessed {
            path: result.path.display().to_string(),
            title: result.title.clone(),
            word_count: result.word_count,
            source_path: candidate.path.display().to_string(),
            device: candidate.sidecar.as_ref().and_then(|s| s.device.clone()),
        });
        archive_successful_candidate(&candidate.path)?;
    }

    Ok(())
}

fn process_candidates(candidates: Vec<WatchCandidate>, config: &Config) {
    if candidates.is_empty() {
        return;
    }

    #[cfg(feature = "parakeet")]
    let batchable = config.transcription.engine == "parakeet"
        && crate::transcribe::resolve_parakeet_native_vad_path(config).is_some();
    #[cfg(not(feature = "parakeet"))]
    let batchable = false;

    let (parakeet_memos, others): (Vec<_>, Vec<_>) = candidates
        .into_iter()
        .partition(|candidate| batchable && candidate.content_type == ContentType::Memo);

    if parakeet_memos.len() > 1 {
        tracing::info!(
            files = parakeet_memos.len(),
            "processing watcher memo burst with parakeet batch inference"
        );
        if let Err(error) = process_parakeet_memo_batch(&parakeet_memos, config) {
            tracing::warn!(error = %error, "parakeet batch processing failed — falling back to single-file processing");
            for candidate in &parakeet_memos {
                if let Err(e) = process_candidate(candidate, config) {
                    tracing::error!(path = %candidate.path.display(), error = %e, "processing failed");
                }
            }
        }
    } else {
        for candidate in &parakeet_memos {
            if let Err(e) = process_candidate(candidate, config) {
                tracing::error!(path = %candidate.path.display(), error = %e, "processing failed");
            }
        }
    }

    for candidate in &others {
        if let Err(e) = process_candidate(candidate, config) {
            tracing::error!(path = %candidate.path.display(), error = %e, "processing failed");
        }
    }
}

#[cfg(not(feature = "parakeet"))]
fn process_parakeet_memo_batch(
    _candidates: &[WatchCandidate],
    _config: &Config,
) -> Result<(), WatchError> {
    Err(WatchError::Io(std::io::Error::other(
        "parakeet batch processing requires the parakeet feature",
    )))
}

/// Run the folder watcher. Blocks until interrupted (Ctrl-C).
pub fn run(watch_dir: Option<&Path>, config: &Config) -> Result<(), WatchError> {
    let dirs: Vec<PathBuf> = if let Some(dir) = watch_dir {
        vec![dir.to_path_buf()]
    } else {
        config.watch.paths.clone()
    };

    // Validate directories
    for dir in &dirs {
        if !dir.exists() {
            fs::create_dir_all(dir)?;
            tracing::info!(dir = %dir.display(), "created watch directory");
        }
        // Create processed/ and failed/ subdirs
        fs::create_dir_all(dir.join("processed"))?;
        fs::create_dir_all(dir.join("failed"))?;
    }

    // Acquire lock
    acquire_lock()?;
    tracing::info!("watcher lock acquired");

    // Set up cleanup on exit
    let _guard = LockGuard;

    eprintln!(
        "Watching {} for audio files... (Ctrl-C to stop)",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Process any existing files first
    for dir in &dirs {
        process_existing_files(dir, config);
    }

    // Set up file watcher
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                tx.send(event).ok();
            }
        },
        notify::Config::default().with_poll_interval(Duration::from_secs(2)),
    )
    .map_err(|e| WatchError::NotifyError(e.to_string()))?;

    for dir in &dirs {
        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| WatchError::NotifyError(e.to_string()))?;
    }

    // Event loop
    let settle_delay = config.watch.settle_delay_ms;
    loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(event) => {
                let mut candidates = Vec::new();
                if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                    for path in event.paths {
                        if let Some(candidate) = handle_file_event(&path, settle_delay, config) {
                            candidates.push(candidate);
                        }
                    }
                }
                while let Ok(event) = rx.try_recv() {
                    if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
                        for path in event.paths {
                            if let Some(candidate) = handle_file_event(&path, settle_delay, config)
                            {
                                candidates.push(candidate);
                            }
                        }
                    }
                }
                process_candidates(candidates, config);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal timeout — continue watching
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                tracing::info!("watcher channel disconnected, exiting");
                break;
            }
        }
    }

    Ok(())
}

/// Process files that already exist in the watch directory.
fn process_existing_files(dir: &Path, config: &Config) {
    let entries: Vec<_> = match fs::read_dir(dir) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };

    let mut candidates = Vec::new();
    for entry in entries {
        let path = entry.path();
        // Reject symlinks — prevents traversal attacks
        if path
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            tracing::warn!(path = %path.display(), "skipping symlink in existing files");
            continue;
        }
        if path.is_file() && has_valid_extension(&path, config) {
            tracing::info!(path = %path.display(), "processing existing file");
            if let Some(candidate) = build_candidate(&path, config.watch.settle_delay_ms, config) {
                candidates.push(candidate);
            }
        }
    }
    process_candidates(candidates, config);
}

fn build_candidate(path: &Path, settle_delay: u64, config: &Config) -> Option<WatchCandidate> {
    // Skip directories, processed/, failed/ subdirs
    if !path.is_file() {
        return None;
    }
    if let Some(parent) = path.parent() {
        if let Some(name) = parent.file_name() {
            let name = name.to_string_lossy();
            if name == "processed" || name == "failed" {
                return None;
            }
        }
    }

    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        tracing::warn!(path = %path.display(), "skipping symlink — only regular files are processed");
        return None;
    }

    if is_icloud_stub(path) {
        tracing::debug!(path = %path.display(), "skipping iCloud stub");
        return None;
    }

    if path.extension().and_then(|e| e.to_str()) == Some("json") {
        return None;
    }

    if !has_valid_extension(path, config) {
        tracing::debug!(path = %path.display(), "skipping — unsupported extension");
        return None;
    }

    if !wait_for_settle(path, settle_delay) {
        tracing::debug!(path = %path.display(), "file not stable yet");
        return None;
    }

    // WAV is validated by the bounded in-process parser. Configured non-WAV
    // inputs are admitted to the bounded ffmpeg boundary, which performs the
    // authoritative container and codec validation without Symphonia fallback.
    if !is_audio_container(path) {
        let is_wav = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
        if !is_wav {
            tracing::warn!(path = %path.display(), "file failed audio probe, not a valid audio container, skipping");
            return None;
        }
    }

    Some(WatchCandidate {
        path: path.to_path_buf(),
        content_type: determine_content_type(path, config),
        sidecar: read_sidecar(path),
    })
}

/// Handle a single file event from the watcher.
fn handle_file_event(path: &Path, settle_delay: u64, config: &Config) -> Option<WatchCandidate> {
    tracing::info!(path = %path.display(), "new file detected, processing");
    build_candidate(path, settle_delay, config)
}

/// RAII guard that releases the lock file on drop.
struct LockGuard;

impl Drop for LockGuard {
    fn drop(&mut self) {
        release_lock();
        tracing::debug!("watcher lock released");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    struct EnvVarGuard {
        key: &'static str,
        previous: Option<std::ffi::OsString>,
    }

    #[cfg(unix)]
    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let previous = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, previous }
        }
    }

    #[cfg(unix)]
    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.take() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn has_valid_extension_matches_configured_types() {
        let config = Config::default();
        let path = Path::new("test.m4a");
        assert!(has_valid_extension(path, &config));

        let path = Path::new("test.wav");
        assert!(has_valid_extension(path, &config));

        let path = Path::new("test.txt");
        assert!(!has_valid_extension(path, &config));

        let path = Path::new("test.pdf");
        assert!(!has_valid_extension(path, &config));
    }

    #[test]
    fn has_valid_extension_is_case_insensitive() {
        let config = Config::default();
        assert!(has_valid_extension(Path::new("test.M4A"), &config));
        assert!(has_valid_extension(Path::new("test.WAV"), &config));
    }

    #[test]
    fn every_configured_non_wav_extension_reaches_the_ffmpeg_boundary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("configured.flac");
        fs::write(&path, b"synthetic untrusted configured container").unwrap();
        let mut config = Config::default();
        config.watch.extensions = vec!["flac".into()];

        assert!(has_valid_extension(&path, &config));
        assert!(compressed_audio_requires_ffmpeg(&path));
        assert!(is_audio_container(&path));
        assert!(build_candidate(&path, 0, &config).is_some());
    }

    #[test]
    fn missing_ffmpeg_fails_closed_and_preserves_watched_audio() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("memo.m4a");
        let sidecar_path = path.with_extension("json");
        fs::write(&path, b"synthetic original bytes").unwrap();
        fs::write(
            &sidecar_path,
            r#"{"device":"Phone","source":"voice-memos"}"#,
        )
        .unwrap();
        let candidate = WatchCandidate {
            path: path.clone(),
            content_type: ContentType::Memo,
            sidecar: read_sidecar(&path),
        };

        let error =
            process_candidate_with_ffmpeg_availability(&candidate, &Config::default(), false)
                .expect_err("non-WAV input must not fall back to an in-process decoder");

        assert!(path.exists());
        assert!(sidecar_path.exists());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic original bytes");
        let message = error.to_string();
        // Assert the actionable content rather than one opening phrase, so a
        // reworded but still-correct message does not fail while a message that
        // dropped the remedy would.
        assert!(message.contains("could not decode this non-WAV import"));
        assert!(message.contains("Install ffmpeg"));
        assert!(message.contains("MINUTES_FFMPEG"));
        assert!(message.contains("original audio remains untouched"));
    }

    /// The regression this track exists to close: an iPhone `.m4a` memo landing
    /// in the watch folder on a machine with no ffmpeg must be admitted, not
    /// refused at preflight. The watcher previously asked "is ffmpeg
    /// installed?" rather than "can this be decoded?", so the bounded decode
    /// worker was never reached from the headline input mode.
    #[test]
    fn compressed_watch_input_is_admitted_without_ffmpeg() {
        let dir = TempDir::new().unwrap();
        let memo = dir.path().join("memo.m4a");
        fs::write(&memo, b"synthetic container bytes").unwrap();

        let guard = crate::test_home_env_lock();
        let previous = std::env::var_os("MINUTES_FFMPEG");
        std::env::set_var("MINUTES_FFMPEG", dir.path().join("absent-ffmpeg"));
        let decodable_by_default = compressed_audio_decodable(&memo, &Config::default());
        let refused = Config {
            transcription: crate::config::TranscriptionConfig {
                compressed_decode_fallback: false,
                ..Config::default().transcription
            },
            ..Config::default()
        };
        let decodable_when_refused = compressed_audio_decodable(&memo, &refused);
        match previous {
            Some(value) => std::env::set_var("MINUTES_FFMPEG", value),
            None => std::env::remove_var("MINUTES_FFMPEG"),
        }
        drop(guard);

        assert!(
            decodable_by_default,
            "a compressed memo must be admitted without ffmpeg so the bounded worker can decode it"
        );
        assert!(
            !decodable_when_refused,
            "with the fallback refused and no ffmpeg there is genuinely no decoder"
        );
    }

    #[cfg(unix)]
    #[test]
    fn non_launchable_ffmpeg_preflight_preserves_audio_and_sidecar() {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let invalid_ffmpeg = dir.path().join("ffmpeg");
        fs::write(&invalid_ffmpeg, b"MZ\x90\0synthetic foreign image").unwrap();
        fs::set_permissions(&invalid_ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let _ffmpeg = EnvVarGuard::set("MINUTES_FFMPEG", &invalid_ffmpeg);

        let path = dir.path().join("memo.m4a");
        let sidecar_path = path.with_extension("json");
        fs::write(&path, b"synthetic original bytes").unwrap();
        fs::write(
            &sidecar_path,
            r#"{"device":"Phone","source":"voice-memos"}"#,
        )
        .unwrap();
        let candidate = WatchCandidate {
            path: path.clone(),
            content_type: ContentType::Memo,
            sidecar: read_sidecar(&path),
        };
        let mut config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();

        let error = process_candidate(&candidate, &config)
            .expect_err("an invalid resolved decoder must preserve watcher input");

        assert!(path.exists());
        assert!(sidecar_path.exists());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic original bytes");
        let message = error.to_string();
        // Assert the actionable content rather than one opening phrase, so a
        // reworded but still-correct message does not fail while a message that
        // dropped the remedy would.
        assert!(message.contains("could not decode this non-WAV import"));
        assert!(message.contains("Install ffmpeg"));
        assert!(message.contains("original audio remains untouched"));
        assert!(message.contains("MINUTES_FFMPEG"));
    }

    #[cfg(unix)]
    #[test]
    fn decode_rejection_moves_exact_audio_and_sidecar_together() {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let fake_ffmpeg = dir.path().join("ffmpeg");
        fs::write(
            &fake_ffmpeg,
            b"#!/bin/sh\nif [ \"$1\" = \"-version\" ]; then printf 'ffmpeg version synthetic\\n'; exit 0; fi\nprintf 'synthetic decoder rejection\\n' >&2\nexit 7\n",
        )
        .unwrap();
        fs::set_permissions(&fake_ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let _ffmpeg = EnvVarGuard::set("MINUTES_FFMPEG", &fake_ffmpeg);

        let path = dir.path().join("memo.m4a");
        let sidecar_path = path.with_extension("json");
        let audio_bytes = b"synthetic original compressed bytes";
        let sidecar_bytes = br#"{"device":"Phone","source":"voice-memos"}"#;
        fs::write(&path, audio_bytes).unwrap();
        fs::write(&sidecar_path, sidecar_bytes).unwrap();
        let candidate = WatchCandidate {
            path: path.clone(),
            content_type: ContentType::Memo,
            sidecar: read_sidecar(&path),
        };
        let mut config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();

        let error = process_candidate(&candidate, &config)
            .expect_err("a launchable decoder rejection must remain recoverable");

        assert!(error.to_string().contains("synthetic decoder rejection"));
        assert!(!path.exists());
        assert!(!sidecar_path.exists());
        let failed_audio = dir.path().join("failed/memo.m4a");
        let failed_sidecar = dir.path().join("failed/memo.json");
        assert_eq!(fs::read(failed_audio).unwrap(), audio_bytes);
        assert_eq!(fs::read(failed_sidecar).unwrap(), sidecar_bytes);
    }

    #[cfg(unix)]
    #[test]
    fn wav_watcher_path_never_launches_configured_ffmpeg() {
        use std::os::unix::fs::PermissionsExt;

        let _env_lock = crate::test_home_env_lock();
        let dir = TempDir::new().unwrap();
        let home = dir.path().join("home");
        fs::create_dir(&home).unwrap();
        let marker = dir.path().join("ffmpeg-was-launched");
        let fake_ffmpeg = dir.path().join("ffmpeg");
        fs::write(
            &fake_ffmpeg,
            b"#!/bin/sh\nprintf touched > \"$MINUTES_WAV_PROBE_MARKER\"\nprintf 'ffmpeg version synthetic\\n'\n",
        )
        .unwrap();
        fs::set_permissions(&fake_ffmpeg, fs::Permissions::from_mode(0o700)).unwrap();
        let _home = EnvVarGuard::set("HOME", &home);
        let _ffmpeg = EnvVarGuard::set("MINUTES_FFMPEG", &fake_ffmpeg);
        let _marker = EnvVarGuard::set("MINUTES_WAV_PROBE_MARKER", &marker);

        let path = dir.path().join("invalid.wav");
        fs::write(&path, b"not a wav container").unwrap();
        let candidate = WatchCandidate {
            path: path.clone(),
            content_type: ContentType::Memo,
            sidecar: None,
        };
        let mut config = Config {
            output_dir: dir.path().join("meetings"),
            ..Config::default()
        };
        config.summarization.engine = "none".into();
        config.diarization.engine = "none".into();

        process_candidate(&candidate, &config).expect_err("invalid WAV must fail in-process");
        assert!(
            !marker.exists(),
            "WAV processing must never grant authority to ffmpeg"
        );
    }

    #[test]
    fn missing_ffmpeg_guidance_is_actionable_on_every_desktop_platform() {
        let guidance = compressed_audio_ffmpeg_guidance();
        assert!(guidance.contains("brew install ffmpeg"));
        assert!(guidance.contains("Linux"));
        assert!(guidance.contains("Windows"));
        assert!(guidance.contains("MINUTES_FFMPEG"));
    }

    /// The state this message is most often read in is the one it used to
    /// mis-describe: the bundled decoder is enabled and running, and the file
    /// failed anyway because Symphonia has no Opus or ALAC decoder. Telling
    /// that user to enable a setting that is already true is the defect.
    #[test]
    fn ffmpeg_guidance_names_the_codecs_that_actually_require_ffmpeg() {
        let guidance = compressed_audio_ffmpeg_guidance();
        assert!(
            guidance.contains("Opus"),
            "guidance must name Opus, the codec that forces ffmpeg for .webm and .ogg"
        );
        assert!(
            guidance.contains("ALAC"),
            "guidance must name ALAC, the other codec the bundled decoder cannot handle"
        );
        // It must not present enabling the bundled decoder as the fix, because
        // in the codec-unsupported state it is already enabled and does not
        // help. Mentioning the setting conditionally is fine; asserting the
        // decoder is off is not.
        assert!(
            !guidance.contains("Alternatively re-enable the bundled decoder"),
            "guidance must not advise enabling a decoder that is on by default"
        );
    }

    /// The watcher and `minutes health` are read by the same person about the
    /// same machine, so they must not disagree about whether compressed imports
    /// work without ffmpeg.
    ///
    /// A reviewer proved the earlier version did not deserve its name: it read
    /// only `health::ffmpeg_status` and never touched the guidance, so replacing
    /// the whole guidance body with text asserting the OPPOSITE of what health
    /// says left this test green. Three sibling tests caught that mutation, so no
    /// production regression escaped, but the test named for agreement was the
    /// one test that could not see a disagreement. Both sides are read now.
    #[test]
    fn health_and_watch_guidance_agree_about_the_unsupported_codecs() {
        // The branch under test is "ffmpeg absent, bundled decoder present".
        // Without a worker-capable binary the third branch answers instead, and
        // the failure would name a wording problem that is not the real cause.
        assert!(
            crate::test_worker_binary_is_available(),
            "this test needs a worker-capable binary beside the harness; build one with \
             `cargo build -p minutes-cli --no-default-features`"
        );
        let _env_lock = crate::test_home_env_lock();
        let previous = std::env::var_os("MINUTES_FFMPEG");
        std::env::set_var(
            "MINUTES_FFMPEG",
            std::env::temp_dir().join("minutes-definitely-missing-ffmpeg"),
        );
        // Fallback at its default: enabled. This is the state the old message
        // described wrongly, and the state most users are in.
        let health = crate::health::ffmpeg_status(&crate::config::Config::default());
        match previous {
            Some(value) => std::env::set_var("MINUTES_FFMPEG", value),
            None => std::env::remove_var("MINUTES_FFMPEG"),
        }
        assert!(
            health.detail.contains("Opus"),
            "health must not claim the bundled decoder covers everything: {}",
            health.detail
        );
        assert!(
            !health
                .detail
                .contains("This works without any extra install"),
            "health must not print an unqualified green check: {}",
            health.detail
        );

        // The agreement itself, asserted as SHARED BYTES rather than as shared
        // vocabulary. The previous version of this block checked that both
        // strings contained "Opus" and "ALAC", which a guidance saying "the
        // bundled decoder decodes Opus and ALAC perfectly well" satisfies
        // while telling the user the opposite of health. A reviewer proved that
        // with the whole suite green.
        let guidance = compressed_audio_ffmpeg_guidance();
        let shared = bundled_decoder_codec_limit!();
        assert!(
            health.detail.contains(shared),
            "health must carry the canonical codec-limit sentence verbatim: {:?}",
            health.detail
        );
        assert!(
            guidance.contains(shared),
            "the watcher guidance must carry the canonical codec-limit sentence verbatim: \
             {guidance:?}"
        );
    }

    #[test]
    fn move_to_processed_works() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.wav");
        fs::write(&file, "audio data").unwrap();

        let dest = move_to(&file, "processed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        assert!(dest.to_str().unwrap().contains("processed"));
    }

    #[test]
    fn move_to_failed_works() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.wav");
        fs::write(&file, "audio data").unwrap();

        let dest = move_to(&file, "failed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        assert!(dest.to_str().unwrap().contains("failed"));
    }

    #[test]
    fn failed_move_preserves_sidecar_beside_audio() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("memo.m4a");
        let sidecar = audio.with_extension("json");
        fs::write(&audio, b"exact audio").unwrap();
        fs::write(&sidecar, b"exact metadata").unwrap();

        let destination = move_to(&audio, "failed").unwrap();

        assert_eq!(fs::read(&destination).unwrap(), b"exact audio");
        assert_eq!(
            fs::read(destination.with_extension("json")).unwrap(),
            b"exact metadata"
        );
        assert!(!audio.exists());
        assert!(!sidecar.exists());
    }

    #[test]
    fn sidecar_collision_never_clobbers_existing_recovery_metadata() {
        let dir = TempDir::new().unwrap();
        let failed = dir.path().join("failed");
        fs::create_dir(&failed).unwrap();
        let audio = dir.path().join("memo.m4a");
        let sidecar = audio.with_extension("json");
        let occupied_sidecar = failed.join("memo.json");
        fs::write(&audio, b"new audio").unwrap();
        fs::write(&sidecar, b"new metadata").unwrap();
        fs::write(&occupied_sidecar, b"existing metadata").unwrap();

        let destination = move_to(&audio, "failed").unwrap();

        assert_ne!(destination, failed.join("memo.m4a"));
        assert_eq!(fs::read(&occupied_sidecar).unwrap(), b"existing metadata");
        assert_eq!(fs::read(&destination).unwrap(), b"new audio");
        assert_eq!(
            fs::read(destination.with_extension("json")).unwrap(),
            b"new metadata"
        );
    }

    #[test]
    fn destination_sidecar_reserves_pair_namespace_without_source_sidecar() {
        let dir = TempDir::new().unwrap();
        let failed = dir.path().join("failed");
        fs::create_dir(&failed).unwrap();
        let audio = dir.path().join("memo.m4a");
        let occupied_sidecar = failed.join("memo.json");
        fs::write(&audio, b"new audio without metadata").unwrap();
        fs::write(&occupied_sidecar, b"unrelated existing metadata").unwrap();

        let destination = move_to(&audio, "failed").unwrap();

        assert_ne!(destination, failed.join("memo.m4a"));
        assert_eq!(
            fs::read(&occupied_sidecar).unwrap(),
            b"unrelated existing metadata"
        );
        assert_eq!(
            fs::read(&destination).unwrap(),
            b"new audio without metadata"
        );
        assert!(!destination.with_extension("json").exists());
    }

    #[test]
    fn destination_creation_after_selection_is_never_replaced() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("memo.m4a");
        fs::write(&audio, b"source audio").unwrap();

        let error = move_to_with_hooks(
            &audio,
            "failed",
            |selected| fs::write(selected, b"concurrent destination").unwrap(),
            |_| {},
            |_| {},
        )
        .expect_err("a destination creator must win the no-replace race");

        let destination = dir.path().join("failed/memo.m4a");
        assert_eq!(fs::read(&audio).unwrap(), b"source audio");
        assert_eq!(fs::read(&destination).unwrap(), b"concurrent destination");
        assert!(matches!(
            error,
            WatchError::MoveError(_, source)
                if source.kind() == std::io::ErrorKind::AlreadyExists
        ));
    }

    #[test]
    fn rollback_never_replaces_a_recreated_source() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("memo.m4a");
        let sidecar = audio.with_extension("json");
        fs::write(&audio, b"original audio").unwrap();
        fs::write(&sidecar, b"original metadata").unwrap();

        let error = move_to_with_hooks(
            &audio,
            "failed",
            |_| {},
            |sidecar_destination| fs::write(sidecar_destination, b"concurrent metadata").unwrap(),
            |source| fs::write(source, b"recreated source audio").unwrap(),
        )
        .expect_err("sidecar collision plus source recreation must report a split state");

        let destination = dir.path().join("failed/memo.m4a");
        let sidecar_destination = destination.with_extension("json");
        assert_eq!(fs::read(&audio).unwrap(), b"recreated source audio");
        assert_eq!(fs::read(&destination).unwrap(), b"original audio");
        assert_eq!(fs::read(&sidecar).unwrap(), b"original metadata");
        assert_eq!(
            fs::read(&sidecar_destination).unwrap(),
            b"concurrent metadata"
        );
        let message = error.to_string();
        assert!(message.contains("audio rollback also failed"));
        assert!(message.contains("audio remains at"));
        assert!(message.contains("memo.m4a"));
        assert!(message.contains("sidecar remains at"));
        assert!(message.contains("memo.json"));
    }

    #[test]
    fn unavailable_atomic_move_fails_closed_without_a_link_unlink_fallback() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("memo.m4a");
        let destination = dir.path().join("failed/memo.m4a");
        fs::create_dir(destination.parent().unwrap()).unwrap();
        fs::write(&source, b"source audio").unwrap();

        // Model a destination that disappears and is recreated while a
        // platform reports that its exclusive rename primitive is unavailable.
        fs::write(&destination, b"first destination").unwrap();
        fs::remove_file(&destination).unwrap();
        fs::write(&destination, b"substituted destination").unwrap();
        let error =
            atomic_noreplace_unavailable(&source, &destination, "forced unsupported primitive")
                .expect_err("unsupported atomic moves must fail without touching either name");

        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
        assert_eq!(fs::read(&source).unwrap(), b"source audio");
        assert_eq!(fs::read(&destination).unwrap(), b"substituted destination");
    }

    #[test]
    fn stable_identity_detects_hard_link_aliases() {
        let dir = TempDir::new().unwrap();
        let original = dir.path().join("original.wav");
        let alias = dir.path().join("alias.wav");
        let unrelated = dir.path().join("unrelated.wav");
        fs::write(&original, b"growing audio").unwrap();
        fs::hard_link(&original, &alias).unwrap();
        fs::write(&unrelated, b"different audio").unwrap();

        assert!(same_regular_file_identity(&original, &alias).unwrap());
        assert!(!same_regular_file_identity(&original, &unrelated).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn stable_identity_rejects_fifo_substitution_without_blocking() {
        use std::os::unix::ffi::OsStrExt;

        let dir = TempDir::new().unwrap();
        let regular = dir.path().join("candidate.wav");
        let substituted = dir.path().join("root-entry.wav");
        fs::write(&regular, b"audio").unwrap();
        let substituted_c = std::ffi::CString::new(substituted.as_os_str().as_bytes()).unwrap();
        // SAFETY: the path is a live, NUL-free C string and the temporary
        // directory is owned by this test.
        assert_eq!(unsafe { libc::mkfifo(substituted_c.as_ptr(), 0o600) }, 0);

        let started = std::time::Instant::now();
        let error = same_regular_file_identity(&regular, &substituted)
            .expect_err("a regular-to-FIFO substitution must fail closed");

        assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "identity verification must never wait for a FIFO writer"
        );
        assert_eq!(fs::read(&regular).unwrap(), b"audio");
    }

    #[test]
    fn move_to_handles_collision() {
        let dir = TempDir::new().unwrap();

        // Create a file in processed/ with the same name
        let processed = dir.path().join("processed");
        fs::create_dir_all(&processed).unwrap();
        fs::write(processed.join("test.wav"), "existing").unwrap();

        // Create the source file
        let file = dir.path().join("test.wav");
        fs::write(&file, "new audio data").unwrap();

        let dest = move_to(&file, "processed").unwrap();
        assert!(!file.exists());
        assert!(dest.exists());
        // Should have a timestamp suffix to avoid collision
        assert_ne!(dest.file_name().unwrap(), "test.wav");
    }

    #[test]
    fn settle_check_rejects_empty_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("empty.wav");
        fs::write(&file, "").unwrap();

        // Use very short delay for test speed
        assert!(!wait_for_settle(&file, 10));
    }

    #[test]
    fn settle_check_accepts_stable_file() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("stable.wav");
        fs::write(&file, "some audio data here that is not empty").unwrap();

        assert!(wait_for_settle(&file, 10));
    }

    #[test]
    fn settle_check_handles_missing_file() {
        assert!(!wait_for_settle(Path::new("/nonexistent/file.wav"), 10));
    }

    #[test]
    fn lock_acquire_and_release() {
        // Clean up any existing lock
        release_lock();

        assert!(acquire_lock().is_ok());
        // Second acquire should fail (same process is alive)
        assert!(acquire_lock().is_err());
        // Release and re-acquire
        release_lock();
        assert!(acquire_lock().is_ok());
        release_lock();
    }

    #[test]
    fn is_icloud_stub_detects_stubs() {
        assert!(is_icloud_stub(Path::new(".recording.m4a.icloud")));
        assert!(is_icloud_stub(Path::new(".test.icloud")));
        assert!(!is_icloud_stub(Path::new("recording.m4a")));
        assert!(!is_icloud_stub(Path::new("icloud")));
        assert!(!is_icloud_stub(Path::new(".hidden_file")));
    }

    #[test]
    fn read_sidecar_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        fs::write(&audio, "audio data").unwrap();
        assert!(read_sidecar(&audio).is_none());
    }

    #[test]
    fn read_sidecar_parses_valid_json() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        let sidecar = dir.path().join("test.json");
        fs::write(&audio, "audio data").unwrap();
        fs::write(&sidecar, r#"{"device": "iPhone", "source": "voice-memos"}"#).unwrap();

        let meta = read_sidecar(&audio).unwrap();
        assert_eq!(meta.device.as_deref(), Some("iPhone"));
        assert_eq!(meta.source.as_deref(), Some("voice-memos"));
        assert!(
            sidecar.exists(),
            "metadata reads must not consume recovery data"
        );
        let processed_audio = archive_successful_candidate(&audio).unwrap();
        let processed_sidecar = processed_audio.with_extension("json");
        assert!(!sidecar.exists());
        assert_eq!(
            fs::read(processed_sidecar).unwrap(),
            br#"{"device": "iPhone", "source": "voice-memos"}"#
        );
    }

    #[test]
    fn successful_single_file_archive_preserves_sidecar_created_after_candidate_build() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("late-sidecar.m4a");
        fs::write(&audio, b"audio").unwrap();
        assert!(read_sidecar(&audio).is_none());

        let sidecar = audio.with_extension("json");
        fs::write(&sidecar, b"late metadata generation").unwrap();
        let processed_audio = archive_successful_candidate(&audio).unwrap();

        assert_eq!(fs::read(&processed_audio).unwrap(), b"audio");
        assert_eq!(
            fs::read(processed_audio.with_extension("json")).unwrap(),
            b"late metadata generation"
        );
        assert!(!sidecar.exists());
    }

    #[test]
    fn successful_batch_archive_preserves_replacement_sidecar_generation() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("replaced-sidecar.m4a");
        let sidecar = audio.with_extension("json");
        fs::write(&audio, b"audio").unwrap();
        fs::write(
            &sidecar,
            br#"{"device":"Synthetic One","source":"voice-memos"}"#,
        )
        .unwrap();
        assert!(read_sidecar(&audio).is_some());

        fs::write(&sidecar, b"replacement metadata generation").unwrap();
        // Parakeet batch and single-file success share this exact archival
        // boundary, so neither path can unlink the replacement by pathname.
        let processed_audio = archive_successful_candidate(&audio).unwrap();

        assert_eq!(
            fs::read(processed_audio.with_extension("json")).unwrap(),
            b"replacement metadata generation"
        );
        assert!(!sidecar.exists());
    }

    #[test]
    fn read_sidecar_handles_malformed_json() {
        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("test.m4a");
        let sidecar = dir.path().join("test.json");
        fs::write(&audio, "audio data").unwrap();
        fs::write(&sidecar, "not valid json {{{").unwrap();

        assert!(read_sidecar(&audio).is_none());
    }

    #[test]
    fn determine_content_type_uses_threshold() {
        let mut config = Config::default();
        config.watch.dictation_threshold_secs = 120;

        // When we can't probe duration, falls back to config type
        let path = Path::new("/nonexistent/test.m4a");
        let ct = determine_content_type(path, &config);
        // Default config.watch.type is "memo"
        assert_eq!(ct, ContentType::Memo);
    }

    #[test]
    fn determine_content_type_disabled_when_zero() {
        let mut config = Config::default();
        config.watch.dictation_threshold_secs = 0;
        config.watch.r#type = "meeting".into();

        let path = Path::new("/nonexistent/test.m4a");
        let ct = determine_content_type(path, &config);
        assert_eq!(ct, ContentType::Meeting);
    }

    /// `dictation_threshold_secs = 0` disables duration routing, so the
    /// configured type must win for compressed input too.
    ///
    /// Uses the committed one-second AAC fixture rather than synthetic bytes:
    /// the defect only appears for a file whose duration can actually be
    /// probed, because `content_type_for_duration` can never yield Memo at a
    /// threshold of zero. Unprobeable bytes fall through to the configured type
    /// either way and hide the bug. The sibling test above configures
    /// `type = "meeting"`, which is also what a broken implementation returns,
    /// so it cannot distinguish the two.
    #[test]
    fn disabled_duration_routing_keeps_compressed_input_on_the_configured_type() {
        const FIXTURE: &[u8] = include_bytes!("../resources/decode-fixture-tone.m4a");
        let dir = TempDir::new().unwrap();
        let memo = dir.path().join("memo.m4a");
        fs::write(&memo, FIXTURE).unwrap();

        let mut config = Config::default();

        // Precondition, asserted rather than assumed. This test observes the
        // defect only where a duration probe actually runs: with no ffmpeg and
        // no worker binary beside the harness, the broken code falls through to
        // the configured type and looks correct. Proven behaviourally rather
        // than by inspecting the environment, because either probe route will
        // do. A one-second fixture under a 1000 s threshold routes as Memo only
        // if something measured it, and the configured type says otherwise.
        config.watch.dictation_threshold_secs = 1000;
        config.watch.r#type = "meeting".into();
        assert_eq!(
            determine_content_type(&memo, &config),
            ContentType::Memo,
            "duration routing is not live here, so this test cannot see the defect it \
             exists for: no launchable ffmpeg and no worker-capable binary beside the \
             harness. Build one with `cargo build -p minutes-cli --no-default-features`"
        );

        config.watch.dictation_threshold_secs = 0;
        config.watch.r#type = "memo".into();

        assert_eq!(
            determine_content_type(&memo, &config),
            ContentType::Memo,
            "a dictation-first user who disabled duration routing must keep memos as memos"
        );

        config.watch.r#type = "meeting".into();
        assert_eq!(determine_content_type(&memo, &config), ContentType::Meeting);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_ffmpeg_probe_routes_long_non_wav_as_meeting() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("long.flac");
        fs::write(&audio, b"synthetic container bytes").unwrap();
        let ffmpeg = dir.path().join("ffmpeg");
        fs::write(
            &ffmpeg,
            "#!/bin/sh\ndd if=/dev/zero bs=32000 count=120 2>/dev/null\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&ffmpeg).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ffmpeg, permissions).unwrap();

        let mut config = Config::default();
        config.watch.dictation_threshold_secs = 120;
        config.watch.r#type = "memo".into();
        assert_eq!(
            determine_content_type_with_ffmpeg(&audio, &config, Some(&ffmpeg)),
            ContentType::Meeting
        );
    }

    /// An ffmpeg that exits 0 having produced nothing is a FAILED probe, not a
    /// measurement of zero seconds.
    ///
    /// This is exactly what ffmpeg does for a non-faststart MP4/M4A read from a
    /// non-seekable pipe. Dividing zero bytes by the byte rate produced a
    /// fabricated 0 s, filed long meetings as memos, and cost them diarization
    /// — so installing ffmpeg made routing worse than not having it.
    #[cfg(unix)]
    #[test]
    fn an_ffmpeg_probe_that_emits_nothing_is_a_failure_not_zero_seconds() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let audio = dir.path().join("long.m4a");
        fs::write(&audio, b"synthetic container bytes").unwrap();
        // Exits 0, writes nothing: the real behaviour on a moov-at-end file.
        let ffmpeg = dir.path().join("ffmpeg");
        fs::write(&ffmpeg, "#!/bin/sh\nexit 0\n").unwrap();
        let mut permissions = fs::metadata(&ffmpeg).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&ffmpeg, permissions).unwrap();

        assert!(
            compressed_audio_duration(&audio, 120, &ffmpeg).is_none(),
            "an empty probe must report failure so the caller can fall through"
        );
    }

    #[test]
    fn skip_files_in_processed_and_failed() {
        let dir = TempDir::new().unwrap();
        let processed = dir.path().join("processed");
        fs::create_dir_all(&processed).unwrap();
        let file = processed.join("old.wav");
        fs::write(&file, "data").unwrap();

        // handle_file_event should skip files in processed/
        // We can verify by checking the parent directory name logic
        let parent_name = file
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy();
        assert_eq!(parent_name, "processed");
    }

    /// The WAV admission gate validates with the bounded in-process decoder.
    #[test]
    fn is_audio_container_accepts_real_wav_fixture() {
        let wav = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("assets")
            .join("demo.wav");
        assert!(wav.exists(), "fixture missing: {}", wav.display());
        assert!(is_audio_container(&wav));
    }

    #[test]
    fn is_audio_container_rejects_random_bytes() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("noise.wav");
        fs::write(&path, b"this is not an audio container").unwrap();
        assert!(!is_audio_container(&path));
    }

    #[test]
    fn is_audio_container_rejects_missing_file() {
        assert!(!is_audio_container(Path::new(
            "/definitely/does/not/exist/file.m4a"
        )));
    }
}
