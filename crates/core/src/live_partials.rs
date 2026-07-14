//! Ephemeral, in-process transcript partials for real-time consumers.
//!
//! The producer runs on the standalone live audio/VAD thread. Its only shared
//! operations are lock-free queue pushes and atomic stores. It never waits for
//! a consumer and never performs I/O. `ArrayQueue` is lock-free rather than
//! wait-free and may use a brief bounded backoff while a concurrent slot stamp
//! settles; that microsecond-scale in-memory spin is acceptable here and has
//! none of the multi-second file-lock behavior this path was built to avoid. A
//! full data ring drops the hypothesis; partials are hints, while the existing
//! durable final path remains the authority.

use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// The default ring is deliberately small: a slow copilot should skip ahead
/// instead of making the capture thread accumulate work.
pub const DEFAULT_PARTIAL_CHANNEL_CAPACITY: usize = 8;

#[derive(Debug, Clone)]
pub struct LivePartial {
    pub session_epoch: u64,
    pub utterance_sequence: u64,
    pub revision: u64,
    pub is_final: bool,
    pub text: String,
    pub speaker: Option<String>,
    pub offset_ms: u64,
    pub audio_received_at: Instant,
    pub partial_published_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupersessionReason {
    Finalized,
    Discarded,
}

impl SupersessionReason {
    fn encode(self) -> u8 {
        match self {
            Self::Finalized => 1,
            Self::Discarded => 2,
        }
    }

    fn decode(value: u8) -> Self {
        match value {
            1 => Self::Finalized,
            _ => Self::Discarded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePartialSuperseded {
    pub session_epoch: u64,
    /// Every partial at or below this utterance sequence is invalid.
    pub through_utterance_sequence: u64,
    pub last_revision: u64,
    pub reason: SupersessionReason,
}

#[derive(Debug, Clone)]
pub enum LivePartialEvent {
    Partial(LivePartial),
    Superseded(LivePartialSuperseded),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialPublishOutcome {
    Published,
    DroppedFull,
}

struct ChannelInner {
    epoch: u64,
    partials: ArrayQueue<LivePartial>,
    freshness_version: AtomicU64,
    latest_utterance_sequence: AtomicU64,
    latest_revision: AtomicU64,
    superseded_through: AtomicU64,
    superseded_last_revision: AtomicU64,
    superseded_reason: AtomicU8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePartialFreshness {
    pub session_epoch: u64,
    pub utterance_sequence: u64,
    pub revision: u64,
    pub superseded_through_utterance_sequence: u64,
    pub superseded_last_revision: u64,
    pub superseded_reason: SupersessionReason,
}

impl LivePartialFreshness {
    pub fn latest_identity(self) -> Option<(u64, u64)> {
        (self.utterance_sequence > self.superseded_through_utterance_sequence && self.revision > 0)
            .then_some((self.utterance_sequence, self.revision))
    }

    pub fn is_current(self, session_epoch: u64, utterance_sequence: u64, revision: u64) -> bool {
        self.session_epoch == session_epoch
            && self.latest_identity() == Some((utterance_sequence, revision))
    }
}

impl ChannelInner {
    fn begin_freshness_write(&self) {
        let prior = self.freshness_version.fetch_add(1, Ordering::SeqCst);
        debug_assert_eq!(prior & 1, 0, "live partial freshness writer overlapped");
    }

    fn end_freshness_write(&self) {
        let prior = self.freshness_version.fetch_add(1, Ordering::SeqCst);
        debug_assert_eq!(prior & 1, 1, "live partial freshness write was not open");
    }

    fn freshness_snapshot(&self) -> LivePartialFreshness {
        loop {
            let before = self.freshness_version.load(Ordering::SeqCst);
            if before & 1 != 0 {
                std::hint::spin_loop();
                continue;
            }

            let snapshot = LivePartialFreshness {
                session_epoch: self.epoch,
                utterance_sequence: self.latest_utterance_sequence.load(Ordering::SeqCst),
                revision: self.latest_revision.load(Ordering::SeqCst),
                superseded_through_utterance_sequence: self
                    .superseded_through
                    .load(Ordering::SeqCst),
                superseded_last_revision: self.superseded_last_revision.load(Ordering::SeqCst),
                superseded_reason: SupersessionReason::decode(
                    self.superseded_reason.load(Ordering::SeqCst),
                ),
            };
            let after = self.freshness_version.load(Ordering::SeqCst);
            if before == after {
                return snapshot;
            }
            std::hint::spin_loop();
        }
    }
}

/// Single-producer endpoint owned by the standalone live audio/VAD loop.
///
/// This type is intentionally not `Clone`: the queue has one producer, which
/// makes its capture-thread ownership obvious at the type boundary.
pub struct LivePartialPublisher {
    inner: Arc<ChannelInner>,
    utterance_sequence: u64,
    revision: u64,
    audio_received_at: Option<Instant>,
}

impl LivePartialPublisher {
    pub fn session_epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// Record the first audio receipt for the current utterance. Repeated VAD
    /// chunks are a no-op so the latency origin stays at the leading edge.
    pub fn begin_utterance(&mut self, received_at: Instant) {
        if self.audio_received_at.is_none() {
            self.audio_received_at = Some(received_at);
        }
    }

    /// Publish one revision without waiting. On a full ring the value is
    /// returned to the caller only as `DroppedFull`; no retry or wakeup occurs
    /// on the capture thread.
    pub fn try_publish(&mut self, text: String, offset_ms: u64) -> PartialPublishOutcome {
        let audio_received_at = self.audio_received_at.unwrap_or_else(Instant::now);
        self.revision = self.revision.saturating_add(1);

        // Publish freshness as one version-guarded snapshot before the data
        // push. A consumer can therefore reject an older queued/in-flight
        // revision even when this push drops, without tearing fields across
        // two utterances.
        self.inner.begin_freshness_write();
        self.inner
            .latest_revision
            .store(self.revision, Ordering::SeqCst);
        self.inner
            .latest_utterance_sequence
            .store(self.utterance_sequence, Ordering::SeqCst);
        self.inner.end_freshness_write();

        let partial_published_at = Instant::now();
        let partial = LivePartial {
            session_epoch: self.inner.epoch,
            utterance_sequence: self.utterance_sequence,
            revision: self.revision,
            is_final: false,
            text,
            speaker: None,
            offset_ms,
            audio_received_at,
            partial_published_at,
        };

        match self.inner.partials.push(partial) {
            Ok(()) => PartialPublishOutcome::Published,
            Err(_) => PartialPublishOutcome::DroppedFull,
        }
    }

    /// Invalidate the current utterance and advance to the next one.
    ///
    /// The control signal is an atomic watermark rather than another queue
    /// entry, so it remains observable even when the partial ring is full.
    pub fn supersede_current(&mut self, reason: SupersessionReason) {
        if self.audio_received_at.is_none() && self.revision == 0 {
            return;
        }
        self.inner.begin_freshness_write();
        self.inner
            .superseded_last_revision
            .store(self.revision, Ordering::SeqCst);
        self.inner
            .superseded_reason
            .store(reason.encode(), Ordering::SeqCst);
        self.inner
            .superseded_through
            .store(self.utterance_sequence, Ordering::SeqCst);
        self.inner.end_freshness_write();

        self.utterance_sequence = self.utterance_sequence.saturating_add(1);
        self.revision = 0;
        self.audio_received_at = None;
    }
}

/// Single-consumer endpoint used by the in-process copilot bridge.
pub struct LivePartialSubscriber {
    inner: Arc<ChannelInner>,
    seen_superseded_through: u64,
}

impl LivePartialSubscriber {
    pub fn session_epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// Read producer identity and supersession as one linearizable snapshot.
    pub fn freshness_snapshot(&self) -> LivePartialFreshness {
        self.inner.freshness_snapshot()
    }

    /// Returns the freshest producer revision that has not been finalized or
    /// discarded. This includes revisions whose data was dropped on overflow,
    /// allowing the consumer to cancel stale in-flight advice.
    pub fn latest_identity(&self) -> Option<(u64, u64)> {
        self.freshness_snapshot().latest_identity()
    }

    pub fn is_current(&self, partial: &LivePartial) -> bool {
        self.freshness_snapshot().is_current(
            partial.session_epoch,
            partial.utterance_sequence,
            partial.revision,
        )
    }

    pub fn is_identity_current(
        &self,
        session_epoch: u64,
        utterance_sequence: u64,
        revision: u64,
    ) -> bool {
        self.freshness_snapshot()
            .is_current(session_epoch, utterance_sequence, revision)
    }

    /// Poll one event without blocking. Supersession is checked first and stale
    /// queued partials are discarded locally before anything reaches a model.
    pub fn try_recv(&mut self) -> Option<LivePartialEvent> {
        let freshness = self.freshness_snapshot();
        let superseded_through = freshness.superseded_through_utterance_sequence;
        if superseded_through > self.seen_superseded_through {
            self.seen_superseded_through = superseded_through;
            return Some(LivePartialEvent::Superseded(LivePartialSuperseded {
                session_epoch: freshness.session_epoch,
                through_utterance_sequence: superseded_through,
                last_revision: freshness.superseded_last_revision,
                reason: freshness.superseded_reason,
            }));
        }

        while let Some(partial) = self.inner.partials.pop() {
            if self.is_current(&partial) {
                return Some(LivePartialEvent::Partial(partial));
            }
        }
        None
    }
}

pub fn channel(
    session_epoch: u64,
    capacity: usize,
) -> (LivePartialPublisher, LivePartialSubscriber) {
    let inner = Arc::new(ChannelInner {
        epoch: session_epoch,
        partials: ArrayQueue::new(capacity.max(1)),
        freshness_version: AtomicU64::new(0),
        latest_utterance_sequence: AtomicU64::new(0),
        latest_revision: AtomicU64::new(0),
        superseded_through: AtomicU64::new(0),
        superseded_last_revision: AtomicU64::new(0),
        superseded_reason: AtomicU8::new(SupersessionReason::Discarded.encode()),
    });
    (
        LivePartialPublisher {
            inner: Arc::clone(&inner),
            utterance_sequence: 1,
            revision: 0,
            audio_received_at: None,
        },
        LivePartialSubscriber {
            inner,
            seen_superseded_through: 0,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn full_ring_drops_immediately_and_never_waits_for_consumer() {
        let (mut publisher, _subscriber) = channel(1, 1);
        publisher.begin_utterance(Instant::now());
        assert_eq!(
            publisher.try_publish("first".into(), 10),
            PartialPublishOutcome::Published
        );

        let started = Instant::now();
        for revision in 0..50_000 {
            assert_eq!(
                publisher.try_publish(format!("overflow {revision}"), 20),
                PartialPublishOutcome::DroppedFull
            );
        }
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "full-ring try_publish loop unexpectedly delayed the producer"
        );
    }

    #[test]
    fn supersession_remains_visible_when_partial_ring_is_full() {
        let (mut publisher, mut subscriber) = channel(7, 1);
        publisher.begin_utterance(Instant::now());
        assert_eq!(
            publisher.try_publish("Approve".into(), 50),
            PartialPublishOutcome::Published
        );
        assert_eq!(
            publisher.try_publish("Reject".into(), 60),
            PartialPublishOutcome::DroppedFull
        );
        publisher.supersede_current(SupersessionReason::Discarded);

        let LivePartialEvent::Superseded(signal) = subscriber.try_recv().unwrap() else {
            panic!("saturated channel must deliver its supersession watermark");
        };
        assert_eq!(signal.session_epoch, 7);
        assert_eq!(signal.through_utterance_sequence, 1);
        assert_eq!(signal.last_revision, 2);
        assert_eq!(signal.reason, SupersessionReason::Discarded);
        assert!(subscriber.try_recv().is_none());
    }

    #[test]
    fn freshness_snapshot_never_exposes_mixed_utterance_fields() {
        let (mut publisher, subscriber) = channel(11, 1);
        publisher.begin_utterance(Instant::now());
        assert_eq!(
            publisher.try_publish("old".into(), 0),
            PartialPublishOutcome::Published
        );

        let inner = Arc::clone(&subscriber.inner);
        let (half_written_tx, half_written_rx) = mpsc::sync_channel(0);
        let (finish_tx, finish_rx) = mpsc::sync_channel(0);
        let writer = std::thread::spawn(move || {
            inner.begin_freshness_write();
            inner.latest_utterance_sequence.store(2, Ordering::SeqCst);
            half_written_tx.send(()).unwrap();
            finish_rx.recv().unwrap();
            inner.latest_revision.store(7, Ordering::SeqCst);
            inner.superseded_through.store(1, Ordering::SeqCst);
            inner.end_freshness_write();
        });
        half_written_rx.recv().unwrap();

        let (snapshot_tx, snapshot_rx) = mpsc::channel();
        let reader = std::thread::spawn(move || {
            snapshot_tx.send(subscriber.freshness_snapshot()).unwrap();
        });
        assert!(
            snapshot_rx.recv_timeout(Duration::from_millis(25)).is_err(),
            "consumer returned while the freshness tuple was half-written"
        );
        finish_tx.send(()).unwrap();

        let snapshot = snapshot_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(snapshot.session_epoch, 11);
        assert_eq!(snapshot.utterance_sequence, 2);
        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.superseded_through_utterance_sequence, 1);
        assert_eq!(snapshot.latest_identity(), Some((2, 7)));
        writer.join().unwrap();
        reader.join().unwrap();
    }

    #[test]
    fn partial_path_contains_no_event_log_or_lock_operation() {
        let forbidden = [
            ["append", "_event"].concat(),
            ["events", ".lock"].concat(),
            [".lo", "ck()"].concat(),
            ["std::", "fs"].concat(),
            ["normalize", "_live_transcript_text"].concat(),
        ];

        let channel_source = include_str!("live_partials.rs");
        let live_source = include_str!("live_transcript.rs");
        let partial_start = live_source
            .find("if let Some(sr) = streaming.feed")
            .expect("standalone partial producer branch");
        let partial_end = live_source[partial_start..]
            .find("// Force-finalize")
            .map(|offset| partial_start + offset)
            .expect("end of standalone partial producer branch");
        let producer_integration = &live_source[partial_start..partial_end];

        for source in [channel_source, producer_integration] {
            for forbidden in &forbidden {
                assert!(
                    !source.contains(forbidden),
                    "real-time partial path contains forbidden operation: {forbidden}"
                );
            }
        }
    }
}
