use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const MAX_GRAPH_VNODE_WATCHES: usize = 8_192;
const GRAPH_VNODE_FD_HEADROOM: usize = 512;
const MAX_DESCRIPTOR_OCCUPANCY_SCAN: usize = 100_000;
const CONTENT_FLAGS: u32 = libc::NOTE_DELETE
    | libc::NOTE_WRITE
    | libc::NOTE_EXTEND
    | libc::NOTE_LINK
    | libc::NOTE_RENAME
    | libc::NOTE_REVOKE;
// Broad ancestors exist only to detect identity-changing replacement. macOS
// reports NOTE_ATTRIB on a directory when unrelated children update metadata,
// so treating it as an input mutation makes every sibling database write a
// false positive. Attribute-only changes cannot substitute a different vnode
// or alter the exact bytes/sensitivity snapshot; identity and content changes
// remain covered by the flags above.
const STRUCTURAL_FLAGS: u32 = libc::NOTE_DELETE | libc::NOTE_RENAME | libc::NOTE_REVOKE;

/// An exact kernel journal for one graph snapshot.
///
/// Each relevant vnode remains open and registered in one kqueue. Vnode flags
/// stay pending in the kernel until `changed` drains them, so write/restore and
/// rename/restore ABA attempts cannot disappear between revision hashes. We do
/// not use FSEvents here: it is intentionally coalesced, may require a rescan,
/// and cannot provide this exact synchronous ordering boundary.
pub(super) struct MacGraphJournal {
    kqueue: RawFd,
    vnodes: Vec<RawFd>,
    correction_root_fd: RawFd,
    correction_targets: Vec<(PathBuf, Option<CorrectionTargetState>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CorrectionTargetState {
    device: u64,
    inode: u64,
    links: u64,
    length: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

impl MacGraphJournal {
    pub(super) fn start(
        corpus_root: &Path,
        correction_root: &Path,
        vocabulary_path: &Path,
        overlays_path: &Path,
    ) -> io::Result<Self> {
        Self::start_with_limit(
            corpus_root,
            correction_root,
            vocabulary_path,
            overlays_path,
            MAX_GRAPH_VNODE_WATCHES,
        )
    }

    fn start_with_limit(
        corpus_root: &Path,
        correction_root: &Path,
        vocabulary_path: &Path,
        overlays_path: &Path,
        max_watches: usize,
    ) -> io::Result<Self> {
        Self::start_with_limit_and_hook(
            corpus_root,
            correction_root,
            vocabulary_path,
            overlays_path,
            max_watches,
            || {},
        )
    }

    fn start_with_limit_and_hook(
        corpus_root: &Path,
        correction_root: &Path,
        vocabulary_path: &Path,
        overlays_path: &Path,
        max_watches: usize,
        after_roots_registered: impl FnOnce(),
    ) -> io::Result<Self> {
        // Resolve macOS's `/var` and `/tmp` compatibility symlinks once before
        // opening every component with O_NOFOLLOW. Inputs below each canonical
        // root remain selected by retained vnodes rather than later path
        // resolution.
        let corpus_root = std::fs::canonicalize(corpus_root)?;
        let correction_root = std::fs::canonicalize(correction_root)?;
        let vocabulary_path = correction_root.join(
            vocabulary_path
                .file_name()
                .ok_or_else(|| io::Error::other("graph vocabulary path had no leaf"))?,
        );
        let overlays_path = correction_root.join(
            overlays_path
                .file_name()
                .ok_or_else(|| io::Error::other("graph overlays path had no leaf"))?,
        );
        let correction_targets = correction_paths(&vocabulary_path, &overlays_path)
            .into_iter()
            .map(|path| {
                let state = correction_target_state(&path)?;
                Ok((path, state))
            })
            .collect::<io::Result<Vec<_>>>()?;
        // Register both mutable roots and every rename-capable ancestor before
        // inventory. This closes the old enumerate-then-register gap: anything
        // created after a directory was visited either receives its own vnode
        // watch or leaves a pending NOTE_WRITE on an already-live parent.
        let mut roots = BTreeMap::<PathBuf, bool>::new();
        add_ancestors(&mut roots, &corpus_root);
        add_ancestors(&mut roots, &correction_root);
        roots.insert(corpus_root.clone(), true);
        roots.insert(correction_root.clone(), true);
        if roots.len() > max_watches {
            return Err(io::Error::other(
                "macOS graph journal vnode budget exceeded",
            ));
        }
        raise_descriptor_limit(roots.len().saturating_add(1))?;

        // SAFETY: kqueue has no Rust aliasing preconditions.
        let kqueue = unsafe { libc::kqueue() };
        if kqueue < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut journal = Self {
            kqueue,
            vnodes: Vec::with_capacity(max_watches.min(roots.len().saturating_add(128))),
            correction_root_fd: -1,
            correction_targets,
        };
        for (path, content) in &roots {
            let descriptor = journal.register_path(path, *content)?;
            if path == &correction_root {
                journal.correction_root_fd = descriptor;
            }
        }
        if journal.correction_root_fd < 0 {
            return Err(io::Error::other(
                "macOS graph correction-root watch was unavailable",
            ));
        }
        after_roots_registered();

        let mut paths = BTreeMap::<PathBuf, bool>::new();

        // The corpus root and every existing descendant carry content flags.
        // Directory NOTE_WRITE covers children created, removed, or renamed
        // after setup; each existing active file vnode covers in-place
        // mutation. Mirror the source walk's inactive-directory pruning so
        // archive/recovery traffic cannot consume the watch budget or make an
        // otherwise stable projection unavailable.
        let walker = walkdir::WalkDir::new(&corpus_root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || !entry.file_type().is_dir()
                    || !crate::markdown::is_inactive_corpus_dir_name(entry.file_name())
            });
        for entry in walker {
            let entry = entry.map_err(|error| io::Error::other(error.to_string()))?;
            if entry.file_type().is_symlink() {
                return Err(io::Error::other(
                    "macOS graph journal refuses symlinked corpus entries",
                ));
            }
            if !roots.contains_key(entry.path()) {
                paths.insert(entry.path().to_path_buf(), true);
            }
            if roots.len().saturating_add(paths.len()) > max_watches {
                return Err(io::Error::other(
                    "macOS graph journal vnode budget exceeded",
                ));
            }
        }

        // The correction root catches creation/replacement of a currently
        // absent file. Exact existing files and SQLite sidecars catch in-place
        // writes without relying on a pathname lookup after setup.
        for path in correction_paths(&vocabulary_path, &overlays_path) {
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(io::Error::other(
                        "macOS graph journal refuses symlinked correction inputs",
                    ));
                }
                Ok(_) => {
                    if !roots.contains_key(&path) {
                        paths.insert(path, true);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
        }

        if roots.len().saturating_add(paths.len()) > max_watches {
            return Err(io::Error::other(
                "macOS graph journal vnode budget exceeded",
            ));
        }
        raise_descriptor_limit(paths.len())?;
        for (path, content) in paths {
            journal.register_path(&path, content)?;
        }
        Ok(journal)
    }

    fn register_path(&mut self, path: &Path, content: bool) -> io::Result<RawFd> {
        let fd = open_vnode(path)?;
        if let Err(error) = register_vnode(self.kqueue, fd, content) {
            // SAFETY: fd was returned by open and is not otherwise owned.
            unsafe { libc::close(fd) };
            return Err(error);
        }
        self.vnodes.push(fd);
        Ok(fd)
    }

    pub(super) fn changed(&mut self) -> io::Result<bool> {
        let mut events = Vec::with_capacity(self.vnodes.len().max(1));
        events.resize_with(self.vnodes.len().max(1), blank_event);
        let timeout = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: kqueue and the output buffer are live for this call.
        let count = unsafe {
            libc::kevent(
                self.kqueue,
                std::ptr::null(),
                0,
                events.as_mut_ptr(),
                events.len() as i32,
                &timeout,
            )
        };
        if count < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut correction_root_changed = false;
        for event in &events[..count as usize] {
            if event.ident as RawFd != self.correction_root_fd {
                return Ok(true);
            }
            if event.fflags & STRUCTURAL_FLAGS != 0 {
                // The retained correction-root vnode itself was renamed,
                // removed, or revoked. Restoring the same pathname/metadata
                // cannot erase that identity transition.
                return Ok(true);
            }
            correction_root_changed = true;
        }
        // Directory kqueue events do not name the changed child. Re-attest
        // only the exact correction leaves so unrelated jobs/audit activity in
        // ~/.minutes does not invalidate graph reads. Existing correction
        // leaves also have their own vnodes, so write/restore and replacement
        // ABA remain independently latched.
        for (path, expected) in &self.correction_targets {
            if correction_target_state(path)? != *expected {
                return Ok(true);
            }
        }
        if correction_root_changed
            && self
                .correction_targets
                .iter()
                .any(|(_, expected)| expected.is_none())
        {
            // Directory kqueue events carry no child name. If a relevant leaf
            // was absent when authority started, create/delete ABA can restore
            // `None` before this checkpoint and is indistinguishable from
            // unrelated root traffic. Fail closed rather than erase it.
            return Ok(true);
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::{descriptor_scan_ceiling, MacGraphJournal, MAX_DESCRIPTOR_OCCUPANCY_SCAN};

    #[test]
    fn high_soft_descriptor_limits_skip_the_linear_occupancy_scan() {
        assert_eq!(
            descriptor_scan_ceiling(MAX_DESCRIPTOR_OCCUPANCY_SCAN as libc::rlim_t).unwrap(),
            Some(MAX_DESCRIPTOR_OCCUPANCY_SCAN)
        );
        assert_eq!(
            descriptor_scan_ceiling((MAX_DESCRIPTOR_OCCUPANCY_SCAN + 1) as libc::rlim_t).unwrap(),
            None
        );
        assert_eq!(descriptor_scan_ceiling(libc::RLIM_INFINITY).unwrap(), None);
    }

    #[test]
    fn vnode_budget_fails_closed_before_opening_an_unbounded_namespace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        std::fs::write(corpus.join("one.md"), b"one").unwrap();

        let error = match MacGraphJournal::start_with_limit(
            &corpus,
            &corrections,
            &corrections.join("vocabulary.toml"),
            &corrections.join("overlays.db"),
            1,
        ) {
            Ok(_) => panic!("over-budget journal unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("vnode budget"));
    }

    #[test]
    fn root_registration_precedes_inventory_and_catches_a_new_source() {
        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        let created = corpus.join("created-after-root-registration.md");

        let mut journal = MacGraphJournal::start_with_limit_and_hook(
            &corpus,
            &corrections,
            &corrections.join("vocabulary.toml"),
            &corrections.join("overlays.db"),
            64,
            || std::fs::write(&created, b"sensitivity: normal\n").unwrap(),
        )
        .unwrap();
        assert!(
            journal.changed().unwrap(),
            "the pre-inventory root watch must retain the create event"
        );
    }

    #[test]
    fn unrelated_correction_root_activity_is_re_attested_and_ignored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        std::fs::write(corpus.join("meeting.md"), b"sensitivity: normal\n").unwrap();
        std::fs::write(corrections.join("vocabulary.toml"), b"version = 1\n").unwrap();
        std::fs::write(corrections.join("overlays.db"), b"stable").unwrap();
        for suffix in ["-wal", "-shm", "-journal"] {
            std::fs::write(corrections.join(format!("overlays.db{suffix}")), b"stable").unwrap();
        }
        let mut journal = MacGraphJournal::start(
            &corpus,
            &corrections,
            &corrections.join("vocabulary.toml"),
            &corrections.join("overlays.db"),
        )
        .unwrap();

        std::fs::write(corrections.join("unrelated-job.json"), b"{}").unwrap();
        assert!(!journal.changed().unwrap());
        std::fs::write(corrections.join("vocabulary.toml"), b"version = 2\n").unwrap();
        assert!(journal.changed().unwrap());
    }

    #[test]
    fn absent_correction_create_delete_aba_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        std::fs::write(corpus.join("meeting.md"), b"sensitivity: normal\n").unwrap();
        let vocabulary = corrections.join("vocabulary.toml");
        let mut journal = MacGraphJournal::start(
            &corpus,
            &corrections,
            &vocabulary,
            &corrections.join("overlays.db"),
        )
        .unwrap();

        std::fs::write(&vocabulary, b"version = 1\n").unwrap();
        std::fs::remove_file(&vocabulary).unwrap();
        assert!(
            journal.changed().unwrap(),
            "an initially absent correction leaf cannot erase create/delete ABA evidence"
        );
    }

    #[test]
    fn correction_root_rename_restore_aba_fails_closed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        let displaced = tmp.path().join("state-displaced");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        std::fs::write(corpus.join("meeting.md"), b"sensitivity: normal\n").unwrap();
        std::fs::write(corrections.join("vocabulary.toml"), b"version = 1\n").unwrap();
        let mut journal = MacGraphJournal::start(
            &corpus,
            &corrections,
            &corrections.join("vocabulary.toml"),
            &corrections.join("overlays.db"),
        )
        .unwrap();

        std::fs::rename(&corrections, &displaced).unwrap();
        std::fs::rename(&displaced, &corrections).unwrap();
        assert!(
            journal.changed().unwrap(),
            "restoring the correction-root pathname cannot erase its vnode rename"
        );
    }

    #[test]
    fn inactive_corpus_subtrees_do_not_consume_or_poison_vnode_watches() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::TempDir::new().unwrap();
        let corpus = tmp.path().join("meetings");
        let corrections = tmp.path().join("state");
        let archive = corpus.join("archive");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&corrections).unwrap();
        symlink("/tmp", archive.join("ignored-link")).unwrap();
        let mut journal = MacGraphJournal::start(
            &corpus,
            &corrections,
            &corrections.join("vocabulary.toml"),
            &corrections.join("overlays.db"),
        )
        .unwrap();

        std::fs::write(archive.join("ignored.md"), b"ignored").unwrap();
        assert!(!journal.changed().unwrap());
    }
}

impl Drop for MacGraphJournal {
    fn drop(&mut self) {
        for fd in self.vnodes.drain(..) {
            // SAFETY: every descriptor is owned by this journal exactly once.
            unsafe { libc::close(fd) };
        }
        // SAFETY: kqueue is owned by this journal exactly once.
        unsafe { libc::close(self.kqueue) };
    }
}

fn add_ancestors(paths: &mut BTreeMap<PathBuf, bool>, root: &Path) {
    for ancestor in root.ancestors().skip(1) {
        // An ancestor should detect its own replacement/rename but not every
        // unrelated sibling create in broad directories such as /tmp.
        paths.entry(ancestor.to_path_buf()).or_insert(false);
    }
}

fn correction_paths(vocabulary: &Path, overlays: &Path) -> Vec<PathBuf> {
    let mut paths = vec![vocabulary.to_path_buf(), overlays.to_path_buf()];
    let Some(name) = overlays.file_name().and_then(|name| name.to_str()) else {
        return paths;
    };
    let Some(parent) = overlays.parent() else {
        return paths;
    };
    for suffix in ["-wal", "-shm", "-journal"] {
        paths.push(parent.join(format!("{name}{suffix}")));
    }
    paths
}

fn correction_target_state(path: &Path) -> io::Result<Option<CorrectionTargetState>> {
    use std::os::unix::fs::MetadataExt;

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(io::Error::other(
            "macOS graph correction input was not a regular file",
        ));
    }
    Ok(Some(CorrectionTargetState {
        device: metadata.dev(),
        inode: metadata.ino(),
        links: metadata.nlink(),
        length: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    }))
}

fn raise_descriptor_limit(additional: usize) -> io::Result<()> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: limit is a valid out pointer.
    if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let Some(scan_ceiling) = descriptor_scan_ceiling(limit.rlim_cur)? else {
        // A raised limit inherited from Node or another launcher can be much
        // larger than the journal's bounded 8,192-watch budget. Do not turn
        // that plentiful-capacity case into a mandatory linear scan. If the
        // process is unusually close to its high limit, the actual vnode opens
        // remain authoritative and fail closed while the journal is built.
        return Ok(());
    };
    let mut open_count = 0usize;
    let mut high_water = 0usize;
    for descriptor in 0..scan_ceiling {
        let status = unsafe { libc::fcntl(descriptor as libc::c_int, libc::F_GETFD) };
        if status >= 0 {
            open_count = open_count.saturating_add(1);
            high_water = descriptor.saturating_add(1);
        } else if io::Error::last_os_error().raw_os_error() != Some(libc::EBADF) {
            return Err(io::Error::last_os_error());
        }
    }
    let planned = additional
        .checked_add(GRAPH_VNODE_FD_HEADROOM)
        .ok_or_else(|| io::Error::other("macOS graph descriptor budget overflowed"))?;
    let required = high_water
        .checked_add(planned)
        .and_then(|value| value.max(open_count.checked_add(planned)?).checked_add(1))
        .ok_or_else(|| io::Error::other("macOS graph descriptor budget overflowed"))?;
    let required = libc::rlim_t::try_from(required)
        .map_err(|_| io::Error::other("macOS graph journal descriptor budget overflowed"))?;
    if required <= limit.rlim_cur {
        return Ok(());
    }
    if required > limit.rlim_max {
        return Err(io::Error::other(
            "macOS graph journal exceeds the process descriptor ceiling",
        ));
    }
    let raised = libc::rlimit {
        rlim_cur: required,
        rlim_max: limit.rlim_max,
    };
    // SAFETY: raised preserves the hard limit and only increases the soft one.
    if unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &raised) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn descriptor_scan_ceiling(limit: libc::rlim_t) -> io::Result<Option<usize>> {
    let limit = usize::try_from(limit)
        .map_err(|_| io::Error::other("macOS graph descriptor limit was not representable"))?;
    Ok((limit <= MAX_DESCRIPTOR_OCCUPANCY_SCAN).then_some(limit))
}

fn open_vnode(path: &Path) -> io::Result<RawFd> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::other("macOS graph journal path contains NUL"))?;
    // SAFETY: path is NUL-terminated and flags take no mode argument.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_EVTONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

fn register_vnode(kqueue: RawFd, fd: RawFd, content: bool) -> io::Result<()> {
    let flags = if content {
        CONTENT_FLAGS
    } else {
        STRUCTURAL_FLAGS
    };
    let change = libc::kevent {
        ident: fd as libc::uintptr_t,
        filter: libc::EVFILT_VNODE,
        flags: libc::EV_ADD | libc::EV_CLEAR | libc::EV_RECEIPT,
        fflags: flags,
        data: 0,
        udata: std::ptr::null_mut(),
    };
    let mut receipt = blank_event();
    // SAFETY: both event pointers refer to live single-element values.
    let count = unsafe { libc::kevent(kqueue, &change, 1, &mut receipt, 1, std::ptr::null()) };
    if count != 1 {
        return if count < 0 {
            Err(io::Error::last_os_error())
        } else {
            Err(io::Error::other(
                "macOS graph journal registration returned no receipt",
            ))
        };
    }
    if receipt.flags & libc::EV_ERROR != 0 && receipt.data != 0 {
        return Err(io::Error::from_raw_os_error(receipt.data as i32));
    }
    Ok(())
}

fn blank_event() -> libc::kevent {
    libc::kevent {
        ident: 0,
        filter: 0,
        flags: 0,
        fflags: 0,
        data: 0,
        udata: std::ptr::null_mut(),
    }
}
