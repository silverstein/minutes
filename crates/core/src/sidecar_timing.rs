//! Fixed-size worker diagnostics. No utterance text, audio, paths, or per-job log.

use serde::Serialize;
use std::time::{Duration, Instant};

#[derive(Debug, Default, Serialize)]
pub(crate) struct TimingAggregate {
    count: u64,
    total_us: u64,
    max_us: u64,
}

impl TimingAggregate {
    pub(crate) fn observe(&mut self, elapsed: Duration) {
        let micros = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        self.count = self.count.saturating_add(1);
        self.total_us = self.total_us.saturating_add(micros);
        self.max_us = self.max_us.max(micros);
    }

    pub(crate) fn measure<T>(&mut self, operation: impl FnOnce() -> T) -> T {
        let started = Instant::now();
        let result = operation();
        self.observe(started.elapsed());
        result
    }
}

/// Owned by the inference worker and returned on join. A failed join produces
/// null telemetry rather than falsely reporting a complete set of zeroes.
#[derive(Debug, Default, Serialize)]
pub(crate) struct SidecarWorkerTimings {
    pub(crate) whisper_model_load: TimingAggregate,
    pub(crate) final_queue_wait: TimingAggregate,
    pub(crate) final_inference: TimingAggregate,
    pub(crate) draft_inference: TimingAggregate,
    pub(crate) writer_lock_wait: TimingAggregate,
    pub(crate) writer_write: TimingAggregate,
    pub(crate) finals_skipped_at_stop: u64,
    pub(crate) drafts_taken: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_retains_counts_totals_and_maximum_without_samples() {
        let mut timing = TimingAggregate::default();
        for micros in [200, 900, 100] {
            timing.observe(Duration::from_micros(micros));
        }
        assert_eq!(timing.count, 3);
        assert_eq!(timing.total_us, 1200);
        assert_eq!(timing.max_us, 900);
        timing.observe(Duration::MAX);
        timing.observe(Duration::from_micros(1));
        assert_eq!(timing.count, 5);
        assert_eq!(timing.total_us, u64::MAX);
        assert_eq!(timing.max_us, u64::MAX);
    }

    #[test]
    fn failed_operations_are_measured_without_logging_their_result() {
        let mut timings = SidecarWorkerTimings::default();
        let result: Result<(), &str> = timings
            .whisper_model_load
            .measure(|| Err("private model path and meeting content"));
        assert_eq!(result, Err("private model path and meeting content"));
        let json = serde_json::to_value(&timings).unwrap();
        assert_eq!(json["whisper_model_load"]["count"], 1);
        assert_eq!(json["final_inference"]["count"], 0);
        assert!(!json.to_string().contains("private"));
    }

    #[test]
    fn queue_model_inference_and_writer_delays_remain_separate() {
        let mut timings = SidecarWorkerTimings::default();
        timings.final_queue_wait.observe(Duration::from_millis(80));
        timings
            .whisper_model_load
            .observe(Duration::from_millis(40));
        timings.final_inference.observe(Duration::from_millis(20));
        timings.draft_inference.observe(Duration::from_millis(10));
        timings.writer_lock_wait.observe(Duration::from_millis(5));
        timings.writer_write.observe(Duration::from_millis(2));
        let json = serde_json::to_value(&timings).unwrap();
        for (stage, total) in [
            ("final_queue_wait", 80_000),
            ("whisper_model_load", 40_000),
            ("final_inference", 20_000),
            ("draft_inference", 10_000),
            ("writer_lock_wait", 5_000),
            ("writer_write", 2_000),
        ] {
            assert_eq!(json[stage]["count"], 1);
            assert_eq!(json[stage]["total_us"], total);
        }
        assert_eq!(json["finals_skipped_at_stop"], 0);
        assert_eq!(json["drafts_taken"], 0);
    }
}
