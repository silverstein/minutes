//! Declare active live work to macOS, without preventing display or idle sleep.
//! The lease ends before standalone post-processing, and on sidecar exit/errors.

#[cfg(any(test, target_os = "macos"))]
struct ActivityLease<T, End: FnMut(T)> {
    token: Option<T>,
    end: End,
}

#[cfg(any(test, target_os = "macos"))]
impl<T, End: FnMut(T)> ActivityLease<T, End> {
    fn new(token: T, end: End) -> Self {
        Self {
            token: Some(token),
            end,
        }
    }
}

#[cfg(any(test, target_os = "macos"))]
impl<T, End: FnMut(T)> Drop for ActivityLease<T, End> {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            (self.end)(token);
        }
    }
}

#[cfg(target_os = "macos")]
fn activity_options() -> objc2_foundation::NSActivityOptions {
    // Apple's live user-work classification, explicitly allowing idle system
    // sleep. Do not add idle-display/system-sleep or latency-critical flags.
    objc2_foundation::NSActivityOptions::UserInitiatedAllowingIdleSystemSleep
}

#[cfg(target_os = "macos")]
pub(crate) fn begin_live_work() -> impl Drop {
    use objc2_foundation::{ns_string, NSProcessInfo};
    let process = NSProcessInfo::processInfo();
    let token = process.beginActivityWithOptions_reason(
        activity_options(),
        ns_string!("Minutes active live transcription"),
    );
    ActivityLease::new((process, token), |(process, token)| {
        // SAFETY: this retained token is the exact object returned by the
        // matching beginActivity call above, and this lease ends it once.
        unsafe { process.endActivity(&token) };
    })
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn begin_live_work() -> impl Drop {
    // An empty owned guard keeps the same lexical scope on other platforms.
    struct NoActivity;
    impl Drop for NoActivity {
        fn drop(&mut self) {}
    }
    NoActivity
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn lease(events: &Rc<RefCell<Vec<&'static str>>>) -> impl Drop {
        events.borrow_mut().push("begin");
        let ended = Rc::clone(events);
        ActivityLease::new((), move |()| ended.borrow_mut().push("end"))
    }

    #[test]
    fn normal_stop_ends_once_before_following_processing() {
        let events = Rc::new(RefCell::new(Vec::new()));
        {
            let _guard = lease(&events);
            events.borrow_mut().push("live");
        }
        events.borrow_mut().push("batch");
        assert_eq!(*events.borrow(), ["begin", "live", "end", "batch"]);
    }

    #[test]
    fn error_return_and_unwind_end_the_activity() {
        let events = Rc::new(RefCell::new(Vec::new()));
        let error = || -> Result<(), ()> {
            let _guard = lease(&events);
            Err(())
        };
        assert!(error().is_err());
        assert_eq!(*events.borrow(), ["begin", "end"]);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = lease(&events);
            panic!("synthetic live error");
        }));
        assert_eq!(*events.borrow(), ["begin", "end", "begin", "end"]);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn non_macos_live_scope_is_supported_without_platform_apis() {
        drop(begin_live_work());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn user_activity_allows_idle_sleep_and_does_not_prevent_display_sleep() {
        use objc2_foundation::NSActivityOptions as Options;
        let options = activity_options();
        assert_eq!(options, Options::UserInitiatedAllowingIdleSystemSleep);
        assert!(!options
            .intersects(Options::IdleSystemSleepDisabled | Options::IdleDisplaySleepDisabled));
        assert!(!options.intersects(Options::LatencyCritical));
    }
}
