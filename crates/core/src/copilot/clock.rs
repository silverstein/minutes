use chrono::{DateTime, Utc};
use std::time::Instant;

/// Time source used by the copilot runner.
///
/// Production uses [`SystemCopilotClock`]. Deterministic replay supplies a
/// logical clock so policy timestamps and every latency stage share one
/// reproducible timeline.
pub trait CopilotClock: Send + Sync + 'static {
    fn monotonic_now(&self) -> Instant;
    fn utc_now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemCopilotClock;

impl CopilotClock for SystemCopilotClock {
    fn monotonic_now(&self) -> Instant {
        Instant::now()
    }

    fn utc_now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}
