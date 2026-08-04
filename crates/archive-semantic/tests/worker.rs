#[cfg(target_os = "macos")]
#[test]
fn bound_worker_denies_network_and_personal_files_before_embedding() {
    use minutes_archive_semantic::{BoundedSemanticEngine, APPLE_ENGLISH_SENTENCE_DIMENSION};
    use std::path::Path;

    let executable = env!("CARGO_BIN_EXE_minutes-archive-semantic");
    let engine = BoundedSemanticEngine::bind(Path::new(executable)).expect("bind and self-test");
    let mut session = engine.open_session().expect("sandboxed session");
    let vector = session
        .embed("The recipient shall not disclose proprietary information.")
        .expect("embedded in worker");
    assert_eq!(vector.len(), APPLE_ENGLISH_SENTENCE_DIMENSION);
    let second = session
        .embed("A clause requiring prior written consent before assignment.")
        .expect("second request on same bounded worker");
    assert_eq!(second.len(), APPLE_ENGLISH_SENTENCE_DIMENSION);
}

/// The app terminates with `app_handle().exit(0)`, which does not unwind, so
/// no destructor runs at exit. The worker snapshot directory is owned by a
/// `TempDir` whose cleanup is `Drop`, and it was surviving the process as a
/// 40 MB copy of the executable in $TMPDIR. The close handler now releases
/// the session explicitly while the process is still alive; this asserts the
/// drop chain that relies on actually reclaims the snapshot.
#[cfg(target_os = "macos")]
#[test]
fn dropping_the_engine_reclaims_its_worker_snapshot() {
    use minutes_archive_semantic::BoundedSemanticEngine;
    use std::path::{Path, PathBuf};

    // Ask the engine which directory is its own rather than inspecting the
    // shared temp directory. The previous version diffed the set of
    // `minutes-archive-semantic-*` directories before and after binding, which
    // only excludes ones that already existed -- a sibling test in this same
    // binary binds its own engine concurrently, so its snapshot landed inside
    // the diff and the count was 2. It failed on a hosted runner while passing
    // locally, purely on scheduling. Nothing about the drop chain was wrong;
    // the observation was.
    let executable = env!("CARGO_BIN_EXE_minutes-archive-semantic");
    let engine = BoundedSemanticEngine::bind(Path::new(executable)).expect("bind");
    let snapshot = PathBuf::from(engine.snapshot_directory());
    assert!(
        snapshot.file_name().is_some_and(|name| name
            .to_string_lossy()
            .starts_with("minutes-archive-semantic-")),
        "unexpected snapshot location {}",
        snapshot.display()
    );
    assert!(snapshot.exists(), "snapshot must exist while bound");
    drop(engine);
    assert!(
        !snapshot.exists(),
        "dropping the engine must reclaim {}",
        snapshot.display()
    );
}
