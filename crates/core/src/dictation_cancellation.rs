//! A decoder may finish after Escape has been pressed. Check both sides of
//! that blocking work before allowing its result to reach an output consumer.
//! This discards pending text; it cannot undo output already committed.

use std::sync::atomic::{AtomicBool, Ordering};

pub(super) fn requested(cancel: Option<&AtomicBool>) -> bool {
    cancel.is_some_and(|flag| flag.load(Ordering::Acquire))
}

pub(super) fn decode_unless_cancelled<T>(
    cancel: Option<&AtomicBool>,
    decode: impl FnOnce() -> T,
) -> Option<T> {
    if requested(cancel) {
        return None;
    }
    let result = decode();
    if requested(cancel) {
        None
    } else {
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn cancelled_session_does_not_start_decode() {
        let cancel = AtomicBool::new(true);
        assert_eq!(
            decode_unless_cancelled(Some(&cancel), || panic!("decode started")),
            None::<()>
        );
    }

    #[test]
    fn cancellation_while_decode_is_blocked_discards_its_result() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (started_tx, started_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            decode_unless_cancelled(Some(worker_cancel.as_ref()), || {
                started_tx.send(()).unwrap();
                resume_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                "text that must not be delivered"
            })
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.store(true, Ordering::Release);
        resume_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), None);
    }

    #[test]
    fn uncancelled_decode_keeps_its_result() {
        let cancel = AtomicBool::new(false);
        assert_eq!(
            decode_unless_cancelled(Some(&cancel), || "kept"),
            Some("kept")
        );
    }

    #[test]
    fn callers_without_cancellation_keep_existing_behavior() {
        assert_eq!(decode_unless_cancelled(None, || "kept"), Some("kept"));
        assert!(!requested(None));
    }
}
