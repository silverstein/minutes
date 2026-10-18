//! Queue admission, cancellation and replay protection. No executor is hidden here.
use super::{text, Result};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

#[derive(Debug)]
struct Details {
    name: String,
    started: Instant,
    finished: Option<Duration>,
    cancellation: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Queued,
    Running,
    CancelRequested,
    Cancelled,
    Completed,
    Failed,
    FinishedAfterCancellation,
}

#[derive(Debug, Default)]
pub struct Calls {
    entries: BTreeMap<String, CallState>,
    details: BTreeMap<String, Details>,
    provider_cancelled: BTreeSet<String>,
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
        self.details.insert(
            id.into(),
            Details {
                name: String::new(),
                started: Instant::now(),
                finished: None,
                cancellation: Arc::new(AtomicBool::new(false)),
            },
        );
        Ok(())
    }
    pub fn name(&mut self, id: &str, tool: &str) {
        if let Some(detail) = self.details.get_mut(id) {
            detail.name = tool.to_owned();
        }
    }
    pub fn cancellation(&self, id: &str) -> Option<Arc<AtomicBool>> {
        self.details
            .get(id)
            .map(|detail| detail.cancellation.clone())
    }
    pub fn cancel_from_provider(&mut self, id: &str) -> Option<CallState> {
        let state = self.cancel(id)?;
        self.provider_cancelled.insert(id.to_owned());
        Some(state)
    }
    pub fn needs_cancellation_receipt(&self, id: &str) -> bool {
        self.entries.contains_key(id) && !self.provider_cancelled.contains(id)
    }
    pub fn snapshots(&self) -> Vec<(String, String, CallState, u64)> {
        let mut ordered: Vec<_> = self.details.iter().collect();
        ordered.sort_by_key(|(_, detail)| detail.started);
        ordered
            .into_iter()
            .map(|(id, detail)| {
                (
                    id.clone(),
                    detail.name.clone(),
                    self.entries[id],
                    detail
                        .finished
                        .unwrap_or_else(|| detail.started.elapsed())
                        .as_secs(),
                )
            })
            .collect()
    }
    fn finish_clock(&mut self, id: &str) {
        if let Some(detail) = self.details.get_mut(id) {
            if detail.finished.is_none() {
                detail.finished = Some(detail.started.elapsed());
            }
        }
    }
    pub fn finish_outcome(&mut self, id: &str, failed: bool, stopped: bool) -> bool {
        if stopped && self.entries.get(id) == Some(&CallState::CancelRequested) {
            self.entries.insert(id.into(), CallState::Cancelled);
            self.finish_clock(id);
            return false;
        }
        let publish = self.finish(id);
        if publish && failed {
            self.entries.insert(id.into(), CallState::Failed);
        }
        publish
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
        if let Some(detail) = self.details.get_mut(id) {
            detail.cancellation.store(true, Ordering::SeqCst);
            if *state == CallState::Queued {
                detail.finished = Some(detail.started.elapsed());
            }
        }
        *state = match *state {
            CallState::Queued => CallState::Cancelled,
            CallState::Running => CallState::CancelRequested,
            other => other,
        };
        Some(*state)
    }
    pub fn cancel_all(&mut self) {
        let ids: Vec<_> = self.entries.keys().cloned().collect();
        for id in ids {
            self.cancel(&id);
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
        self.finish_clock(id);
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

    #[test]
    fn snapshots_are_chronological_and_completed_time_is_frozen() {
        let mut calls = Calls::default();
        calls.register("z-first").unwrap();
        calls.begin("z-first");
        calls.finish("z-first");
        let finished = calls.details["z-first"].finished;
        calls.register("a-later").unwrap();
        assert_eq!(calls.snapshots()[0].0, "z-first");
        calls.finish("z-first");
        assert_eq!(calls.details["z-first"].finished, finished);
    }
    #[test]
    fn host_cancellation_settles_request_but_provider_cancellation_does_not_reply() {
        let mut calls = Calls::default();
        calls.register("host-requested").unwrap();
        calls.cancel("host-requested");
        calls.register("provider-requested").unwrap();
        calls.cancel_from_provider("provider-requested");
        assert!(calls.needs_cancellation_receipt("host-requested"));
        assert!(!calls.needs_cancellation_receipt("provider-requested"));
        assert!(!calls.needs_cancellation_receipt("unknown"));
    }

    #[test]
    fn cancellation_token_and_failure_receipts_match_execution() {
        let mut calls = Calls::default();
        calls.register("a").unwrap();
        calls.name("a", "build_prototype");
        let token = calls.cancellation("a").unwrap();
        calls.begin("a");
        calls.cancel("a");
        assert!(token.load(Ordering::SeqCst));
        assert!(!calls.finish_outcome("a", true, true));
        assert_eq!(calls.snapshots()[0].2, CallState::Cancelled);
        calls.register("b").unwrap();
        calls.begin("b");
        assert!(calls.finish_outcome("b", true, false));
        assert_eq!(calls.snapshots()[1].2, CallState::Failed);
    }
}
