//! One automatic start attempt per observed native call. Keep this separate
//! from banner reminders, which deliberately repeat while recording is off.
use std::collections::HashMap;
use std::time::{Duration, Instant};

const REARM_AFTER_ABSENCE: Duration = Duration::from_secs(30);

#[derive(Default)]
pub(super) struct AutomaticStarts {
    calls: HashMap<&'static str, (Instant, bool)>,
}

impl AutomaticStarts {
    pub(super) fn observe(&mut self, app: &str, now: Instant, enabled: bool) -> bool {
        let Some(id) = minutes_core::config::auto_record_app_id(app) else {
            return false;
        };
        // A different observed call also establishes absence for older apps.
        // Keep the current app's attempt across unobserved gaps, so disabling
        // detection or skipping a poll cannot authorize a surprise restart.
        self.calls.retain(|other_id, (last_seen, _)| {
            *other_id == id || now.duration_since(*last_seen) < REARM_AFTER_ABSENCE
        });
        let (last_seen, consumed) = self.calls.entry(id).or_insert((now, false));
        *last_seen = now;
        if !enabled || *consumed {
            return false;
        }
        // Consume before launching, even when consent, ownership or native
        // availability will reject the attempt. Never retry a surprise start.
        *consumed = true;
        true
    }

    pub(super) fn observe_no_call(&mut self, now: Instant) {
        self.calls
            .retain(|_, (last_seen, _)| now.duration_since(*last_seen) < REARM_AFTER_ABSENCE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_manual_stop_and_failed_launch_do_not_retry() {
        let now = Instant::now();
        let mut starts = AutomaticStarts::default();
        assert!(starts.observe("Microsoft Teams", now, true));
        for seconds in [1, 20, 60, 3600] {
            assert!(!starts.observe("MSTeams", now + Duration::from_secs(seconds), true));
        }
    }

    #[test]
    fn brief_mute_does_not_rearm_but_observed_call_end_does() {
        let now = Instant::now();
        let mut starts = AutomaticStarts::default();
        assert!(starts.observe("zoom.us", now, true));
        starts.observe_no_call(now + Duration::from_secs(20));
        assert!(!starts.observe("zoom.us", now + Duration::from_secs(21), true));
        starts.observe_no_call(now + Duration::from_secs(51));
        assert!(starts.observe("zoom.us", now + Duration::from_secs(52), true));
    }

    #[test]
    fn switching_apps_rearms_the_absent_app_after_thirty_seconds() {
        let now = Instant::now();
        let mut starts = AutomaticStarts::default();
        assert!(starts.observe("MSTeams", now, true));
        assert!(starts.observe("zoom.us", now + Duration::from_secs(1), true));
        assert!(!starts.observe("zoom.us", now + Duration::from_secs(31), true));
        assert!(starts.observe("Microsoft Teams", now + Duration::from_secs(32), true));
        assert!(!starts.observe("MSTeams", now + Duration::from_secs(40), true));
    }

    #[test]
    fn opt_in_is_required_and_toggle_does_not_erase_attempt() {
        let now = Instant::now();
        let mut starts = AutomaticStarts::default();
        assert!(!starts.observe("zoom.us", now, false));
        assert!(starts.observe("zoom.us", now, true));
        assert!(!starts.observe("zoom.us", now, false));
        assert!(!starts.observe("zoom.us", now, true));
        assert!(!starts.observe("teams-web", now, true));
        assert!(!starts.observe("WhatsApp", now, true));
    }
}
