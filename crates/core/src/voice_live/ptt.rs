//! Host-owned PTT state, independent of the bounded control-message queue.
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(super) struct PttGate(AtomicU64);

impl PttGate {
    pub fn set(&self, down: bool) {
        // A repeated press/release does not restart an utterance. On exhaustion
        // force release; never wrap an old generation back into authority.
        let changed = self
            .0
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |old| {
                if (old & 1 != 0) == down {
                    return None;
                }
                old.checked_add(2).map(|next| (next & !1) | u64::from(down))
            });
        if changed.is_err_and(|old| old >= u64::MAX - 2) {
            self.0.store(u64::MAX - 1, Ordering::SeqCst);
        }
    }
    pub fn snapshot(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    pub fn permits_audio(&self, observed: u64) -> bool {
        observed & 1 != 0 && self.snapshot() == observed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_revokes_audio_without_enqueuing_a_control_message() {
        let gate = PttGate::default();
        gate.set(true);
        let press = gate.snapshot();
        assert!(gate.permits_audio(press));
        gate.set(false);
        assert!(!gate.permits_audio(press));
        gate.set(true);
        assert!(!gate.permits_audio(press));
        assert!(gate.permits_audio(gate.snapshot()));
    }
    #[test]
    fn repeated_gestures_do_not_restart_and_exhaustion_releases() {
        let gate = PttGate::default();
        gate.set(true);
        let first = gate.snapshot();
        gate.set(true);
        assert_eq!(first, gate.snapshot());
        gate.0.store(u64::MAX, Ordering::SeqCst);
        gate.set(false);
        assert!(!gate.permits_audio(gate.snapshot()));
    }
}
