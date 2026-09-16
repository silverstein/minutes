//! Queue admission, cancellation and replay protection. No executor is hidden here.
use super::{text, Result};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    FinishedAfterCancellation,
}

#[derive(Debug, Default)]
pub struct Calls {
    entries: BTreeMap<String, CallState>,
}

impl Calls {
    pub fn register(&mut self, id: &str) -> Result<()> {
        text(id)?;
        // Retain tombstones through the whole session: never evict an ID and
        // accidentally replay an outward call after reconnect.
        if self.entries.len() >= 4096 || self.entries.contains_key(id) {
            return Err("duplicate call or session call budget exhausted".into());
        }
        self.entries.insert(id.into(), CallState::Queued);
        Ok(())
    }
    pub fn begin(&mut self, id: &str) -> bool {
        if self.entries.get(id) != Some(&CallState::Queued) {
            return false;
        }
        self.entries.insert(id.into(), CallState::Running);
        true
    }
    pub fn cancel(&mut self, id: &str) -> Option<CallState> {
        let state = self.entries.get_mut(id)?;
        *state = match *state {
            CallState::Queued => CallState::Cancelled,
            CallState::Running => CallState::CancelRequested,
            other => other,
        };
        Some(*state)
    }
    pub fn cancel_all(&mut self) {
        for state in self.entries.values_mut() {
            *state = match *state {
                CallState::Queued => CallState::Cancelled,
                CallState::Running => CallState::CancelRequested,
                other => other,
            };
        }
    }
    /// False means do not publish as a successful model tool result. It does
    /// not claim an already running external operation was undone.
    pub fn finish(&mut self, id: &str) -> bool {
        let Some(state) = self.entries.get_mut(id) else {
            return false;
        };
        let publish = *state == CallState::Running;
        *state = match *state {
            CallState::Running => CallState::Completed,
            CallState::CancelRequested => CallState::FinishedAfterCancellation,
            other => other,
        };
        publish
    }
    pub fn active(&self) -> usize {
        self.entries
            .values()
            .filter(|s| {
                matches!(
                    s,
                    CallState::Queued | CallState::Running | CallState::CancelRequested
                )
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_prevents_queued_execution() {
        let mut calls = Calls::default();
        calls.register("a").unwrap();
        assert_eq!(calls.cancel("a"), Some(CallState::Cancelled));
        assert!(!calls.begin("a"));
        assert_eq!(calls.active(), 0);
        assert!(calls.register("a").is_err());
    }
    #[test]
    fn active_is_not_falsely_reported_as_cancelled() {
        let mut calls = Calls::default();
        calls.register("a").unwrap();
        assert!(calls.begin("a"));
        calls.cancel_all();
        assert_eq!(calls.active(), 1);
        assert!(!calls.finish("a"));
        assert_eq!(calls.active(), 0);
        assert_eq!(
            calls.cancel("a"),
            Some(CallState::FinishedAfterCancellation)
        );
    }
    #[test]
    fn completion_is_once_only() {
        let mut calls = Calls::default();
        calls.register("a").unwrap();
        assert!(calls.begin("a"));
        assert!(calls.finish("a"));
        assert!(!calls.begin("a"));
        assert!(!calls.finish("a"));
    }
}
