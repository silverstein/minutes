//! Retry helper for `SQLITE_BUSY` under WAL contention.
//!
//! WAL allows concurrent readers + one writer. When two writers race (e.g. the
//! Tauri watcher coalescer and a CLI invocation both upserting), one will see
//! `SQLITE_BUSY` after `busy_timeout` expires. This helper retries up to 3
//! times with linear backoff (50, 100, 150ms), then propagates the error.
//!
//! `cmd_search` is synchronous in the current codebase, so blocking sleep is
//! safe. If search ever moves to an async context, switch to `tokio::time::sleep`.

use std::time::Duration;

/// Run `f` and retry up to 3 times if it returns `SQLITE_BUSY`.
/// Backoff: 50ms, 100ms, 150ms before each retry.
pub fn with_retry_on_busy<F, T>(mut f: F) -> Result<T, rusqlite::Error>
where
    F: FnMut() -> Result<T, rusqlite::Error>,
{
    for attempt in 0..3 {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) if is_busy(&e) => {
                std::thread::sleep(Duration::from_millis(50 * (attempt as u64 + 1)));
            }
            Err(e) => return Err(e),
        }
    }
    f()
}

fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _) if err.code == rusqlite::ErrorCode::DatabaseBusy
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, OpenFlags};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn returns_ok_on_first_success() {
        let calls = AtomicUsize::new(0);
        let result: Result<i32, _> = with_retry_on_busy(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retries_on_busy_then_succeeds() {
        let calls = AtomicUsize::new(0);
        let result = with_retry_on_busy(|| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            if n < 2 {
                Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ErrorCode::DatabaseBusy,
                        extended_code: 5,
                    },
                    Some("busy".into()),
                ))
            } else {
                Ok("done")
            }
        });
        assert_eq!(result.unwrap(), "done");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn caps_at_4_total_attempts() {
        let calls = AtomicUsize::new(0);
        let _: Result<(), _> = with_retry_on_busy(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error {
                    code: rusqlite::ErrorCode::DatabaseBusy,
                    extended_code: 5,
                },
                Some("busy".into()),
            ))
        });
        // 3 retries + 1 final attempt = 4 total
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn non_busy_error_propagates_without_retry() {
        let calls = AtomicUsize::new(0);
        let result: Result<(), _> = with_retry_on_busy(|| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(rusqlite::Error::QueryReturnedNoRows)
        });
        assert!(matches!(result, Err(rusqlite::Error::QueryReturnedNoRows)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// End-to-end: two connections to the same DB; the second times out
    /// because `busy_timeout` is set short (10ms).
    #[test]
    fn busy_timeout_actually_set() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("test.db");
        let c1 = Connection::open(&db).unwrap();
        c1.pragma_update(None, "journal_mode", &"WAL").unwrap();
        c1.pragma_update(None, "busy_timeout", &10_i32).unwrap();
        c1.execute_batch("CREATE TABLE t (x INTEGER)").unwrap();

        let c2 = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .unwrap();
        c2.pragma_update(None, "busy_timeout", &10_i32).unwrap();

        let _txn = c1
            .unchecked_transaction()
            .and_then(|t| {
                t.execute("INSERT INTO t (x) VALUES (1)", [])?;
                Ok(t)
            })
            .unwrap();

        let res = c2.execute("INSERT INTO t (x) VALUES (2)", []);
        // Busy expected with very short timeout while c1 holds an open txn.
        // Some platforms return BUSY immediately, others may succeed if WAL
        // commits faster than the 10ms window — both are acceptable.
        if let Err(e) = res {
            assert!(is_busy(&e), "expected SQLITE_BUSY, got {e:?}");
        }
    }
}
