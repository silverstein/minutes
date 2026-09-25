use pocketstation::{
    CaptureNativeFormat, SessionSourceActivityObservations, SessionSourceActivityPolicy,
    SessionSourceActivityState, SessionSourceReplacementObservations,
    SessionSourceSignalEvaluation, SessionSourceSignalObservations, SessionSourceSignalPolicy,
    SessionSourceSignalState, StemId,
};
use std::sync::Mutex;
use std::time::Duration;

pub(super) const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const STALL_TIMEOUT: Duration = Duration::from_secs(2);
pub(super) const EXACT_ZERO_TIMEOUT: Duration = Duration::from_millis(1_500);
pub(super) const MINIMUM_PEAK_DBFS: f64 = -45.0;
pub(super) const MINIMUM_RMS_DBFS: f64 = -55.0;
// #1057's failed Logi HFP stem was not bit-exact zero: its measured peak was
// -78.3 dBFS and its mean level was -91 dBFS. Keep this observation threshold
// well below ordinary quiet speech, and require it to persist before warning
// the user. Amplitude alone never authorizes source recovery.
const UNUSABLE_LOW_SIGNAL_MAXIMUM_PEAK_DBFS: f64 = -70.0;
const UNUSABLE_LOW_SIGNAL_MAXIMUM_RMS_DBFS: f64 = -80.0;
pub(super) const UNUSABLE_LOW_SIGNAL_TIMEOUT: Duration = Duration::from_millis(1_500);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MicrophoneSignalState {
    AwaitingFirstFrame,
    Active,
    NoFrames,
    Stalled,
    ExactDigitalZeroPending,
    DigitallySilent,
    BelowThresholds,
    SustainedLowSignal,
    SignalObserved,
    NonFiniteSamples,
    SourceFailed,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct MicrophoneObservations {
    pub(crate) state: MicrophoneSignalState,
    pub(crate) native_format: Option<CaptureNativeFormat>,
    pub(crate) activity: Option<SessionSourceActivityObservations>,
    pub(crate) signal: Option<SessionSourceSignalObservations>,
    pub(crate) replacement: Option<SessionSourceReplacementObservations>,
    pub(crate) source_generation: u32,
    pub(crate) discontinuity_epoch: u64,
}

impl Default for MicrophoneObservations {
    fn default() -> Self {
        Self {
            state: MicrophoneSignalState::AwaitingFirstFrame,
            native_format: None,
            activity: None,
            signal: None,
            replacement: None,
            source_generation: 1,
            discontinuity_epoch: 0,
        }
    }
}

impl MicrophoneObservations {
    pub(crate) fn requires_recovery(self) -> bool {
        matches!(
            self.state,
            MicrophoneSignalState::NoFrames
                | MicrophoneSignalState::Stalled
                | MicrophoneSignalState::DigitallySilent
                | MicrophoneSignalState::NonFiniteSamples
                | MicrophoneSignalState::SourceFailed
        )
    }

    pub(crate) fn confirms_recovery(self) -> bool {
        self.state == MicrophoneSignalState::SignalObserved
    }
}

#[derive(Debug, Default)]
pub(super) struct LowSignalTracker {
    started_at_ns: Option<u64>,
    source_generation: u32,
    discontinuity_epoch: u64,
}

impl LowSignalTracker {
    pub(super) fn observe(
        &mut self,
        observed_at_ns: u64,
        source_generation: u32,
        discontinuity_epoch: u64,
        peak_dbfs: Option<f64>,
        rms_dbfs: Option<f64>,
    ) -> bool {
        let source_changed = self.source_generation != source_generation
            || self.discontinuity_epoch != discontinuity_epoch;
        let unusably_low = peak_dbfs.is_some_and(|peak| {
            peak <= UNUSABLE_LOW_SIGNAL_MAXIMUM_PEAK_DBFS
                && rms_dbfs.is_some_and(|rms| rms <= UNUSABLE_LOW_SIGNAL_MAXIMUM_RMS_DBFS)
        });

        if source_changed || !unusably_low {
            self.started_at_ns = None;
            self.source_generation = source_generation;
            self.discontinuity_epoch = discontinuity_epoch;
        }
        if !unusably_low {
            return false;
        }

        let started_at_ns = *self.started_at_ns.get_or_insert(observed_at_ns);
        observed_at_ns.saturating_sub(started_at_ns)
            >= UNUSABLE_LOW_SIGNAL_TIMEOUT.as_nanos() as u64
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct SignalWindowContinuity {
    pub(super) source_generation: u32,
    pub(super) discontinuity_epoch: u64,
    pub(super) samples_observed_total: u64,
}

impl From<SessionSourceSignalObservations> for SignalWindowContinuity {
    fn from(observations: SessionSourceSignalObservations) -> Self {
        Self {
            source_generation: observations.window_source_generation,
            discontinuity_epoch: observations.window_discontinuity_epoch,
            samples_observed_total: observations.samples_observed_total,
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ObservationContinuityTracker {
    continuity: Option<(u32, u64)>,
    started_at_ns: u64,
    activity_frames_baseline: u64,
    signal_samples_baseline: u64,
}

impl ObservationContinuityTracker {
    pub(super) fn project_activity(
        &mut self,
        activity: Option<SessionSourceActivityObservations>,
        signal: Option<SignalWindowContinuity>,
        source_generation: u32,
        discontinuity_epoch: u64,
    ) -> (Option<SessionSourceActivityObservations>, bool) {
        let continuity = (source_generation, discontinuity_epoch);
        if self.continuity != Some(continuity) {
            let first_attachment = self.continuity.is_none();
            self.continuity = Some(continuity);
            self.started_at_ns = activity.map_or(0, |entry| {
                if first_attachment {
                    entry.session_started_at_ns
                } else {
                    entry.observed_at_ns
                }
            });
            self.activity_frames_baseline = if first_attachment {
                0
            } else {
                activity.map_or(0, |entry| entry.frames_received_total)
            };
            self.signal_samples_baseline = if first_attachment {
                0
            } else {
                signal.map_or(0, |entry| entry.samples_observed_total)
            };
        }
        if self.started_at_ns == 0 {
            self.started_at_ns = activity.map_or(0, |entry| entry.observed_at_ns);
        }

        let current_signal_observed = signal.is_some_and(|entry| {
            entry.source_generation == source_generation
                && entry.discontinuity_epoch == discontinuity_epoch
                && entry.samples_observed_total > self.signal_samples_baseline
        });
        let projected_activity = activity.map(|entry| {
            let frames_received_total = entry
                .frames_received_total
                .saturating_sub(self.activity_frames_baseline);
            let current_frame_observed = current_signal_observed && frames_received_total > 0;
            let latest_frame_received_at_ns = if current_frame_observed {
                entry.latest_frame_received_at_ns
            } else {
                None
            };
            SessionSourceActivityObservations {
                session_started_at_ns: self.started_at_ns,
                observed_at_ns: entry.observed_at_ns,
                first_frame_received_at_ns: latest_frame_received_at_ns,
                latest_frame_received_at_ns,
                frames_received_total: if current_frame_observed {
                    frames_received_total
                } else {
                    0
                },
            }
        });
        (projected_activity, current_signal_observed)
    }
}

pub(super) struct MicrophoneObservationEvaluator {
    activity_policy: SessionSourceActivityPolicy,
    signal_policy: SessionSourceSignalPolicy,
    low_signal: LowSignalTracker,
    continuity: ObservationContinuityTracker,
}

impl MicrophoneObservationEvaluator {
    pub(super) fn new(
        activity_policy: SessionSourceActivityPolicy,
        signal_policy: SessionSourceSignalPolicy,
    ) -> Self {
        Self {
            activity_policy,
            signal_policy,
            low_signal: LowSignalTracker::default(),
            continuity: ObservationContinuityTracker::default(),
        }
    }

    pub(super) fn update_observations(
        &mut self,
        running: &pocketstation::RunningSession,
        stem_id: StemId,
        source_failed: bool,
        observations: &Mutex<MicrophoneObservations>,
    ) -> Option<MicrophoneObservations> {
        let snapshot = running.metrics_snapshot().ok()?;
        let source_index = (0..snapshot.source_count()).find(|&index| {
            snapshot
                .source(index)
                .is_some_and(|source| source.stem_id == stem_id)
        })?;
        let raw_activity = snapshot.source_activity(source_index).copied();
        let raw_signal = snapshot.source_signal(source_index).copied();
        let replacement = snapshot.source_replacement(source_index).copied();
        let native_format = snapshot
            .source_native_format(source_index)
            .and_then(|entry| entry.opened_native_format);
        let source_generation = replacement.map_or(1, |entry| entry.source_generation);
        let discontinuity_epoch = replacement.map_or(0, |entry| entry.discontinuity_epoch);
        let (activity, current_signal_observed) = self.continuity.project_activity(
            raw_activity,
            raw_signal.map(SignalWindowContinuity::from),
            source_generation,
            discontinuity_epoch,
        );
        let signal = current_signal_observed.then_some(raw_signal).flatten();
        let state = self.evaluate_observations(
            activity,
            signal,
            source_generation,
            discontinuity_epoch,
            source_failed,
        );
        let observation_snapshot = MicrophoneObservations {
            state,
            native_format,
            activity,
            signal,
            replacement,
            source_generation,
            discontinuity_epoch,
        };
        if let Ok(mut current) = observations.lock() {
            *current = observation_snapshot;
        }
        Some(observation_snapshot)
    }

    pub(super) fn evaluate_observations(
        &mut self,
        activity: Option<SessionSourceActivityObservations>,
        signal: Option<SessionSourceSignalObservations>,
        source_generation: u32,
        discontinuity_epoch: u64,
        source_failed: bool,
    ) -> MicrophoneSignalState {
        let activity_state = activity.map(|activity| activity.evaluate(self.activity_policy).state);
        let signal_evaluation = signal.map(|signal| signal.evaluate(self.signal_policy));
        evaluate_state(
            activity_state,
            signal_evaluation,
            signal.map_or_else(
                || activity.map_or(0, |entry| entry.observed_at_ns),
                |entry| entry.observed_at_ns,
            ),
            &mut self.low_signal,
            source_generation,
            discontinuity_epoch,
            source_failed,
        )
    }
}

pub(super) fn evaluate_state(
    activity_state: Option<SessionSourceActivityState>,
    signal_evaluation: Option<SessionSourceSignalEvaluation>,
    observed_at_ns: u64,
    low_signal_tracker: &mut LowSignalTracker,
    source_generation: u32,
    discontinuity_epoch: u64,
    source_failed: bool,
) -> MicrophoneSignalState {
    if source_failed {
        low_signal_tracker.started_at_ns = None;
        return MicrophoneSignalState::SourceFailed;
    }
    let sustained_low_signal = signal_evaluation.is_some_and(|evaluation| {
        evaluation.state == SessionSourceSignalState::BelowCallerThresholds
            && low_signal_tracker.observe(
                observed_at_ns,
                source_generation,
                discontinuity_epoch,
                evaluation.peak_dbfs,
                evaluation.rms_dbfs,
            )
    });
    if !matches!(
        signal_evaluation,
        Some(evaluation) if evaluation.state == SessionSourceSignalState::BelowCallerThresholds
    ) {
        low_signal_tracker.started_at_ns = None;
    }
    classify_evaluations(
        activity_state,
        signal_evaluation.map(|evaluation| evaluation.state),
        sustained_low_signal,
    )
}

pub(super) fn classify_evaluations(
    activity: Option<SessionSourceActivityState>,
    signal: Option<SessionSourceSignalState>,
    sustained_low_signal: bool,
) -> MicrophoneSignalState {
    match activity {
        None | Some(SessionSourceActivityState::AwaitingFirstFrame) => {
            return MicrophoneSignalState::AwaitingFirstFrame;
        }
        Some(SessionSourceActivityState::FirstFrameTimedOut) => {
            return MicrophoneSignalState::NoFrames;
        }
        Some(SessionSourceActivityState::Stalled) => {
            return MicrophoneSignalState::Stalled;
        }
        Some(SessionSourceActivityState::Active) => {}
    }
    match signal {
        None | Some(SessionSourceSignalState::NoSamplesObserved) => MicrophoneSignalState::Active,
        Some(SessionSourceSignalState::ExactDigitalZeroPending) => {
            MicrophoneSignalState::ExactDigitalZeroPending
        }
        Some(SessionSourceSignalState::SustainedExactDigitalZero) => {
            MicrophoneSignalState::DigitallySilent
        }
        Some(SessionSourceSignalState::BelowCallerThresholds) if sustained_low_signal => {
            MicrophoneSignalState::SustainedLowSignal
        }
        Some(SessionSourceSignalState::BelowCallerThresholds) => {
            MicrophoneSignalState::BelowThresholds
        }
        Some(SessionSourceSignalState::MeetsCallerThresholds) => {
            MicrophoneSignalState::SignalObserved
        }
        Some(SessionSourceSignalState::NonFiniteSamplesObserved) => {
            MicrophoneSignalState::NonFiniteSamples
        }
    }
}
