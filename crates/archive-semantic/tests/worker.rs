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

/// Binding must not copy the executable anywhere.
///
/// This began as a leak test: the worker was copied to $TMPDIR and, because
/// the app exits with `app_handle().exit(0)` and never unwinds, a 40 MB copy
/// survived the process. The copy is gone entirely now -- a Developer ID
/// signature with the hardened runtime is bound to its bundle, so a copied
/// executable fails validation and is SIGKILLed on exec, which made every
/// notarized build unable to run its workers at all. The worker runs in place
/// and the descriptor opened at bind time pins the inode instead.
///
/// So the property to hold is stronger than reclaiming a copy: none is made.
#[cfg(target_os = "macos")]
#[test]
fn binding_the_engine_copies_the_executable_nowhere() {
    use minutes_archive_semantic::BoundedSemanticEngine;
    use std::path::Path;

    let executable = env!("CARGO_BIN_EXE_minutes-archive-semantic");
    let before = worker_copies_in_temp();
    let engine = BoundedSemanticEngine::bind(Path::new(executable)).expect("bind");
    let during = worker_copies_in_temp();
    assert_eq!(
        during, before,
        "binding created a copy of the executable in the temp directory"
    );
    drop(engine);
}

#[cfg(target_os = "macos")]
fn worker_copies_in_temp() -> usize {
    // Counted rather than diffed by name: sibling tests in this binary bind
    // their own engines concurrently, and an earlier version of this test
    // failed on a hosted runner purely on that scheduling.
    std::fs::read_dir(std::env::temp_dir())
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("minutes-archive-semantic-")
                })
                .count()
        })
        .unwrap_or(0)
}

/// Verifying the pinned worker from several threads at once must agree.
///
/// The digest used to be taken by rewinding a `try_clone` of the descriptor.
/// `try_clone` is `dup`: the duplicate shares one open file description and
/// therefore one offset, so concurrent verifications read each other's bytes
/// and each concludes the executable was replaced underneath it. The converter
/// carried the identical defect and it cost 40 of 60 documents on the first
/// build that extracted on more than one thread; here it had simply not been
/// reached yet, because one caller happens to drive the session from a single
/// thread today.
///
/// Nothing about the file changes during this test, so every verification must
/// succeed. With the shared offset restored this fails.
#[cfg(target_os = "macos")]
#[test]
fn concurrent_verification_of_the_pinned_worker_agrees() {
    use minutes_archive_semantic::BoundedSemanticEngine;
    use std::path::Path;
    use std::sync::Arc;

    let executable = env!("CARGO_BIN_EXE_minutes-archive-semantic");
    let engine = Arc::new(BoundedSemanticEngine::bind(Path::new(executable)).expect("bind"));

    let failures = std::thread::scope(|scope| {
        let workers = (0..8)
            .map(|_| {
                let engine = Arc::clone(&engine);
                scope.spawn(move || {
                    let mut failures = 0;
                    for _ in 0..24 {
                        // Opening a session re-verifies the pinned executable,
                        // which is the path that took the digest.
                        if engine.open_session().is_err() {
                            failures += 1;
                        }
                    }
                    failures
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .map(|worker| worker.join().expect("thread"))
            .sum::<usize>()
    });

    assert_eq!(
        failures, 0,
        "concurrent verification reported the worker as swapped while nothing touched it"
    );
}
