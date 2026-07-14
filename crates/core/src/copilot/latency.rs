use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::time::Instant;

pub(crate) const MAX_LATENCY_RECORDS: usize = 64;

/// Producer/bridge timestamps captured before a partial enters the model lane.
/// `Instant` keeps this instrumentation monotonic and process-local.
#[derive(Debug, Clone, Copy)]
pub struct PartialLatencySeed {
    pub session_epoch: u64,
    pub utterance_sequence: u64,
    pub utterance_revision: u64,
    pub audio_received_at: Instant,
    pub partial_published_at: Instant,
    pub trigger_at: Instant,
    pub context_ready_at: Instant,
}

/// One bounded, in-memory latency timeline. Stage values are microseconds from
/// the first received audio for this utterance; missing stages remain `None`
/// when evidence is dropped, cancelled, or retracted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatencyRecord {
    pub session_epoch: u64,
    pub evidence_revision: u64,
    pub utterance_sequence: u64,
    pub utterance_revision: u64,
    pub audio_received_us: u64,
    pub partial_published_us: u64,
    pub trigger_us: u64,
    pub context_ready_us: u64,
    pub model_request_us: Option<u64>,
    pub first_token_us: Option<u64>,
    pub nudge_us: Option<u64>,
}

#[derive(Debug)]
struct Timeline {
    origin: Instant,
    record: LatencyRecord,
}

#[derive(Debug, Default)]
pub(crate) struct LatencyTracker {
    timelines: VecDeque<Timeline>,
}

impl LatencyTracker {
    pub(crate) fn begin(&mut self, evidence_revision: u64, seed: PartialLatencySeed) {
        self.timelines.push_back(Timeline {
            origin: seed.audio_received_at,
            record: LatencyRecord {
                session_epoch: seed.session_epoch,
                evidence_revision,
                utterance_sequence: seed.utterance_sequence,
                utterance_revision: seed.utterance_revision,
                audio_received_us: 0,
                partial_published_us: elapsed_us(seed.audio_received_at, seed.partial_published_at),
                trigger_us: elapsed_us(seed.audio_received_at, seed.trigger_at),
                context_ready_us: elapsed_us(seed.audio_received_at, seed.context_ready_at),
                model_request_us: None,
                first_token_us: None,
                nudge_us: None,
            },
        });
        while self.timelines.len() > MAX_LATENCY_RECORDS {
            self.timelines.pop_front();
        }
    }

    pub(crate) fn mark_model_request(&mut self, epoch: u64, revision: u64, at: Instant) {
        if let Some(timeline) = self.find_mut(epoch, revision) {
            timeline.record.model_request_us = Some(elapsed_us(timeline.origin, at));
        }
    }

    pub(crate) fn mark_first_token(&mut self, epoch: u64, revision: u64, at: Instant) {
        if let Some(timeline) = self.find_mut(epoch, revision) {
            if timeline.record.first_token_us.is_none() {
                timeline.record.first_token_us = Some(elapsed_us(timeline.origin, at));
            }
        }
    }

    pub(crate) fn mark_nudge(&mut self, epoch: u64, revision: u64, at: Instant) {
        if let Some(timeline) = self.find_mut(epoch, revision) {
            timeline.record.nudge_us = Some(elapsed_us(timeline.origin, at));
        }
    }

    pub(crate) fn records(&self) -> Vec<LatencyRecord> {
        self.timelines
            .iter()
            .map(|timeline| timeline.record.clone())
            .collect()
    }

    fn find_mut(&mut self, epoch: u64, revision: u64) -> Option<&mut Timeline> {
        self.timelines.iter_mut().rev().find(|timeline| {
            timeline.record.session_epoch == epoch && timeline.record.evidence_revision == revision
        })
    }
}

fn elapsed_us(origin: Instant, at: Instant) -> u64 {
    at.checked_duration_since(origin)
        .unwrap_or_default()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn latency_ring_keeps_only_newest_64_of_65_records() {
        let mut tracker = LatencyTracker::default();
        let origin = Instant::now();
        for revision in 1..=65 {
            let offset = Duration::from_micros(revision);
            tracker.begin(
                revision,
                PartialLatencySeed {
                    session_epoch: 1,
                    utterance_sequence: revision,
                    utterance_revision: 1,
                    audio_received_at: origin,
                    partial_published_at: origin + offset,
                    trigger_at: origin + offset,
                    context_ready_at: origin + offset,
                },
            );
        }

        let records = tracker.records();
        assert_eq!(records.len(), MAX_LATENCY_RECORDS);
        assert_eq!(records.first().unwrap().evidence_revision, 2);
        assert_eq!(records.last().unwrap().evidence_revision, 65);
    }
}
