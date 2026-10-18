//! Worker-local cancellation scope; no process-global or model-controlled PID.
use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
thread_local! { static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) }; }
pub(super) fn cancelled() -> bool {
    CANCEL.with(|slot| {
        slot.borrow()
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    })
}
pub(super) fn with_cancellation<T>(token: Option<Arc<AtomicBool>>, run: impl FnOnce() -> T) -> T {
    struct Reset(Option<Arc<AtomicBool>>);
    impl Drop for Reset {
        fn drop(&mut self) {
            CANCEL.with(|s| *s.borrow_mut() = self.0.take());
        }
    }
    let previous = CANCEL.with(|s| s.replace(token));
    let _reset = Reset(previous);
    run()
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_scoped_to_its_worker() {
        let token = Arc::new(AtomicBool::new(true));
        with_cancellation(Some(token), || {
            assert!(cancelled());
            assert!(!std::thread::spawn(cancelled).join().unwrap());
        });
        assert!(!cancelled());
    }
}
