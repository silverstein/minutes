use super::chunks::{
    contiguous_source_interval, deliver_chunk, merge_source_interval, missing_source_frames,
    same_source_interval, MicrophoneAudioChunkWriter, MAX_SEQUENCE_GAP_DURATION,
    SOURCE_FRAME_DURATION_NS, SOURCE_FRAME_SAMPLES,
};
use super::health::{
    classify_evaluations, evaluate_state, LowSignalTracker, MicrophoneObservationEvaluator,
    MicrophoneObservations, MicrophoneSignalState, ObservationContinuityTracker,
    SignalWindowContinuity, EXACT_ZERO_TIMEOUT, FIRST_FRAME_TIMEOUT, MINIMUM_PEAK_DBFS,
    MINIMUM_RMS_DBFS, STALL_TIMEOUT, UNUSABLE_LOW_SIGNAL_TIMEOUT,
};
use super::recovery::{replacement_continuity_reached, MicrophoneReplacementOutcome};
use super::selection::{
    choose_recovery_fallback, select_discovered_microphone, MicrophoneDiagnosticIdentity,
    MicrophoneSelection,
};
use super::worker::{
    opened_native_format_diagnostic_key, opened_native_format_log_entry, stop_cancel_and_join,
};
use crate::streaming::{AudioChunk, AudioChunkLineage, SourceRole};
use pocketstation::{
    CaptureNativeFormat, DeviceId, DeviceSelector, SessionSourceActivityObservations,
    SessionSourceActivityPolicy, SessionSourceActivityState, SessionSourceReplacementObservations,
    SessionSourceSignalEvaluation, SessionSourceSignalPolicy, SessionSourceSignalState, StemId,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn selection(device_id: &str, display_name: &str) -> MicrophoneSelection {
    MicrophoneSelection {
        selector: DeviceSelector::id(DeviceId::new(device_id)),
        device_id: device_id.to_owned(),
        display_name: display_name.to_owned(),
    }
}

fn activity(
    observed_at_ns: u64,
    first_frame_received_at_ns: Option<u64>,
    latest_frame_received_at_ns: Option<u64>,
) -> SessionSourceActivityObservations {
    SessionSourceActivityObservations {
        session_started_at_ns: 1,
        observed_at_ns,
        first_frame_received_at_ns,
        latest_frame_received_at_ns,
        frames_received_total: u64::from(first_frame_received_at_ns.is_some()),
    }
}

fn policies() -> (SessionSourceActivityPolicy, SessionSourceSignalPolicy) {
    (
        SessionSourceActivityPolicy::new(FIRST_FRAME_TIMEOUT, STALL_TIMEOUT).unwrap(),
        SessionSourceSignalPolicy::new(MINIMUM_PEAK_DBFS, MINIMUM_RMS_DBFS, EXACT_ZERO_TIMEOUT)
            .unwrap(),
    )
}

#[test]
fn activity_facts_distinguish_waiting_no_frames_active_and_stalled() {
    let (activity_policy, signal_policy) = policies();
    let mut evaluator = MicrophoneObservationEvaluator::new(activity_policy, signal_policy);
    assert_eq!(
        evaluator.evaluate_observations(
            Some(activity(1_000_000_000, None, None)),
            None,
            1,
            0,
            false,
        ),
        MicrophoneSignalState::AwaitingFirstFrame
    );
    assert_eq!(
        evaluator.evaluate_observations(
            Some(activity(3_000_000_000, None, None)),
            None,
            1,
            0,
            false,
        ),
        MicrophoneSignalState::NoFrames
    );
    assert_eq!(
        evaluator.evaluate_observations(
            Some(activity(
                1_500_000_000,
                Some(500_000_000),
                Some(1_000_000_000),
            )),
            None,
            1,
            0,
            false,
        ),
        MicrophoneSignalState::Active
    );
    assert_eq!(
        evaluator.evaluate_observations(
            Some(activity(
                4_000_000_000,
                Some(500_000_000),
                Some(1_000_000_000),
            )),
            None,
            1,
            0,
            false,
        ),
        MicrophoneSignalState::Stalled
    );
}

#[test]
fn source_failure_is_not_relabelled_as_silence() {
    let (activity_policy, signal_policy) = policies();
    let mut evaluator = MicrophoneObservationEvaluator::new(activity_policy, signal_policy);
    assert_eq!(
        evaluator.evaluate_observations(None, None, 1, 0, true),
        MicrophoneSignalState::SourceFailed
    );
}

#[test]
fn duplicate_default_display_name_resolves_by_exact_device_identity() {
    let selected = select_discovered_microphone(
        vec![
            selection("other-uid", "MacBook Pro Microphone"),
            selection("default-uid", "MacBook Pro Microphone"),
        ],
        "MacBook Pro Microphone",
        Some("default-uid"),
    )
    .unwrap();

    assert_eq!(selected.device_id, "default-uid");
}

#[test]
fn explicit_ambiguous_display_name_remains_rejected() {
    let result = select_discovered_microphone(
        vec![
            selection("first-uid", "Duplicate Microphone"),
            selection("second-uid", "Duplicate Microphone"),
        ],
        "Duplicate Microphone",
        None,
    );

    assert!(result.is_err());
}

#[test]
fn fallback_follows_an_os_default_that_changed_physical_device() {
    let fallback = choose_recovery_fallback(
        Some("headset-uid"),
        Some(selection("builtin-uid", "MacBook Microphone")),
    )
    .unwrap();

    assert_eq!(fallback.device_id, "builtin-uid");
    assert_eq!(fallback.display_name, "MacBook Microphone");
}

#[test]
fn fallback_does_not_invent_an_alternative_when_os_default_is_unchanged() {
    assert!(choose_recovery_fallback(
        Some("headset-uid"),
        Some(selection("headset-uid", "Logi HFP")),
    )
    .is_none());
    assert!(
        choose_recovery_fallback(None, Some(selection("builtin-uid", "Built-in Microphone")),)
            .is_none()
    );
}

#[test]
fn active_zero_samples_are_distinct_from_no_frames_and_useful_signal() {
    assert_eq!(
        classify_evaluations(
            Some(SessionSourceActivityState::Active),
            Some(SessionSourceSignalState::SustainedExactDigitalZero),
            false,
        ),
        MicrophoneSignalState::DigitallySilent
    );
    assert_eq!(
        classify_evaluations(
            Some(SessionSourceActivityState::Active),
            Some(SessionSourceSignalState::MeetsCallerThresholds),
            false,
        ),
        MicrophoneSignalState::SignalObserved
    );
}

#[test]
fn reported_hfp_noise_envelope_requires_bounded_low_signal_warning() {
    let mut tracker = LowSignalTracker::default();
    let timeout_ns = UNUSABLE_LOW_SIGNAL_TIMEOUT.as_nanos() as u64;

    assert!(!tracker.observe(1, 1, 0, Some(-78.3), Some(-91.0)));
    assert!(!tracker.observe(timeout_ns, 1, 0, Some(-78.3), Some(-91.0)));
    assert!(tracker.observe(timeout_ns + 1, 1, 0, Some(-78.3), Some(-91.0)));

    // Ordinary quiet audio and a new physical source both reset the
    // bounded interval rather than inheriting an earlier failure.
    assert!(!tracker.observe(timeout_ns + 2, 1, 0, Some(-60.0), Some(-68.0)));
    assert!(!tracker.observe(timeout_ns * 3, 2, 1, Some(-78.3), Some(-91.0)));
}

#[test]
fn reported_hfp_envelope_transitions_only_after_timeout_and_resets_on_source_change() {
    let mut tracker = LowSignalTracker::default();
    let timeout_ns = UNUSABLE_LOW_SIGNAL_TIMEOUT.as_nanos() as u64;
    let low_signal = Some(SessionSourceSignalEvaluation {
        state: SessionSourceSignalState::BelowCallerThresholds,
        peak_dbfs: Some(-78.3),
        rms_dbfs: Some(-91.0),
        consecutive_exact_zero_duration_ns: 0,
    });
    let evaluate = |observed_at_ns, generation, discontinuity, tracker: &mut LowSignalTracker| {
        evaluate_state(
            Some(SessionSourceActivityState::Active),
            low_signal,
            observed_at_ns,
            tracker,
            generation,
            discontinuity,
            false,
        )
    };

    assert_eq!(
        evaluate(1, 1, 0, &mut tracker),
        MicrophoneSignalState::BelowThresholds
    );
    assert_eq!(
        evaluate(timeout_ns, 1, 0, &mut tracker),
        MicrophoneSignalState::BelowThresholds
    );
    assert_eq!(
        evaluate(timeout_ns + 1, 1, 0, &mut tracker),
        MicrophoneSignalState::SustainedLowSignal
    );

    assert_eq!(
        evaluate(timeout_ns * 3, 2, 0, &mut tracker),
        MicrophoneSignalState::BelowThresholds,
        "a new source generation starts a new bounded interval"
    );
    assert_eq!(
        evaluate(timeout_ns * 5, 2, 1, &mut tracker),
        MicrophoneSignalState::BelowThresholds,
        "a discontinuity starts a new bounded interval"
    );
}

#[test]
fn sustained_low_signal_warns_but_does_not_authorize_source_recovery() {
    let observations = MicrophoneObservations {
        state: MicrophoneSignalState::SustainedLowSignal,
        ..MicrophoneObservations::default()
    };

    assert!(!observations.requires_recovery());
    assert!(!observations.confirms_recovery());
}

#[test]
fn stale_good_window_cannot_confirm_a_replacement_without_a_current_frame() {
    let (activity_policy, _) = policies();
    let mut continuity = ObservationContinuityTracker::default();
    let old_activity = SessionSourceActivityObservations {
        session_started_at_ns: 1,
        observed_at_ns: 1_000_000_000,
        first_frame_received_at_ns: Some(900_000_000),
        latest_frame_received_at_ns: Some(990_000_000),
        frames_received_total: 1,
    };
    let old_good_window = SignalWindowContinuity {
        source_generation: 1,
        discontinuity_epoch: 0,
        samples_observed_total: 480,
    };

    let (initial_activity, initial_signal_is_current) =
        continuity.project_activity(Some(old_activity), Some(old_good_window), 1, 0);
    assert!(initial_signal_is_current);
    assert_eq!(
        initial_activity.unwrap().evaluate(activity_policy).state,
        SessionSourceActivityState::Active
    );
    assert!(MicrophoneObservations {
        state: MicrophoneSignalState::SignalObserved,
        source_generation: 1,
        discontinuity_epoch: 0,
        ..MicrophoneObservations::default()
    }
    .confirms_recovery());

    let (replacement_activity, replacement_signal_is_current) = continuity.project_activity(
        Some(SessionSourceActivityObservations {
            observed_at_ns: 1_100_000_000,
            ..old_activity
        }),
        Some(old_good_window),
        2,
        1,
    );
    assert!(!replacement_signal_is_current);
    let replacement_state = classify_evaluations(
        replacement_activity.map(|entry| entry.evaluate(activity_policy).state),
        None,
        false,
    );
    assert_eq!(replacement_state, MicrophoneSignalState::AwaitingFirstFrame);
    assert!(!MicrophoneObservations {
        state: replacement_state,
        source_generation: 2,
        discontinuity_epoch: 1,
        ..MicrophoneObservations::default()
    }
    .confirms_recovery());

    let (current_activity, current_signal_is_current) = continuity.project_activity(
        Some(SessionSourceActivityObservations {
            observed_at_ns: 1_200_000_000,
            latest_frame_received_at_ns: Some(1_190_000_000),
            frames_received_total: 2,
            ..old_activity
        }),
        Some(SignalWindowContinuity {
            source_generation: 2,
            discontinuity_epoch: 1,
            samples_observed_total: 960,
        }),
        2,
        1,
    );
    assert!(current_signal_is_current);
    assert_eq!(
        current_activity.unwrap().evaluate(activity_policy).state,
        SessionSourceActivityState::Active
    );
    assert!(MicrophoneObservations {
        state: MicrophoneSignalState::SignalObserved,
        source_generation: 2,
        discontinuity_epoch: 1,
        ..MicrophoneObservations::default()
    }
    .confirms_recovery());
}

#[test]
fn opened_format_diagnostic_is_content_free_and_names_the_physical_format() {
    let identity = MicrophoneDiagnosticIdentity {
        device_id: "device-uid".to_owned(),
        display_name: "Logi HFP".to_owned(),
    };
    let native_format = CaptureNativeFormat {
        sample_rate_hz: 16_000,
        channel_count: 1,
        sample_representation: pocketstation::CaptureSampleRepresentation::SignedInteger16,
    };
    let entry = opened_native_format_log_entry(&identity, native_format, 2, 1);

    assert_eq!(entry["step"], "pocketstation_microphone_opened_format");
    assert_eq!(entry["device_id"], "device-uid");
    assert_eq!(entry["sample_rate_hz"], 16_000);
    assert_eq!(entry["channel_count"], 1);
    assert_eq!(entry["sample_representation"], "SignedInteger16");
    assert_eq!(entry["source_generation"], 2);
    assert_eq!(entry["discontinuity_epoch"], 1);
    assert!(entry.get("samples").is_none());
    assert!(entry.get("audio").is_none());

    let observations = MicrophoneObservations {
        native_format: Some(native_format),
        source_generation: 2,
        discontinuity_epoch: 1,
        ..MicrophoneObservations::default()
    };
    let key = opened_native_format_diagnostic_key(observations, &identity);
    assert_eq!(
        opened_native_format_diagnostic_key(observations, &identity),
        key.clone(),
        "unchanged observations are de-duplicated by the same key"
    );
    assert_eq!(
        opened_native_format_diagnostic_key(
            MicrophoneObservations {
                discontinuity_epoch: 2,
                ..observations
            },
            &identity,
        ),
        key.clone(),
        "a discontinuity alone does not duplicate the attachment diagnostic"
    );
    assert_ne!(
        opened_native_format_diagnostic_key(
            MicrophoneObservations {
                source_generation: 3,
                ..observations
            },
            &identity,
        ),
        key.clone(),
        "a new source generation gets one new diagnostic"
    );
    assert_ne!(
        opened_native_format_diagnostic_key(
            observations,
            &MicrophoneDiagnosticIdentity {
                device_id: "fallback-uid".to_owned(),
                display_name: "Built-in Microphone".to_owned(),
            },
        ),
        key,
        "a different physical device gets one new diagnostic"
    );
}

#[test]
fn aggregate_lineage_preserves_bounds_and_rejects_generation_merging() {
    let first = AudioChunkLineage {
        session_id: 1,
        source_id: 202,
        stem_id: 7,
        clock_id: 9,
        first_sequence_number: 20,
        last_sequence_number: 20,
        missing_sequence_count: 0,
        inserted_silence_samples: 0,
        timestamp_start_ns: 1_000_000_000,
        duration_ns: 10_000_000,
        source_generation: 1,
        discontinuity_epoch: 0,
        permission_epoch: 3,
        observed_at_ns: 1_010_000_000,
        polled_at_ns: 1_011_000_000,
    };
    let mut aggregate = first;
    for sequence in 21..=29 {
        let next = AudioChunkLineage {
            first_sequence_number: sequence,
            last_sequence_number: sequence,
            timestamp_start_ns: 1_000_000_000 + (sequence - 20) * 10_000_000,
            observed_at_ns: 1_010_000_000 + (sequence - 20) * 10_000_000,
            polled_at_ns: 1_011_000_000 + (sequence - 20) * 10_000_000,
            ..first
        };
        assert!(contiguous_source_interval(aggregate, next));
        aggregate = merge_source_interval(Some(aggregate), next);
    }

    assert_eq!(aggregate.first_sequence_number, 20);
    assert_eq!(aggregate.last_sequence_number, 29);
    assert_eq!(aggregate.timestamp_start_ns, 1_000_000_000);
    assert_eq!(aggregate.duration_ns, 100_000_000);
    assert_eq!(aggregate.source_generation, 1);
    assert_eq!(aggregate.discontinuity_epoch, 0);

    let replacement = AudioChunkLineage {
        source_id: 204,
        first_sequence_number: 0,
        last_sequence_number: 0,
        timestamp_start_ns: 1_200_000_000,
        source_generation: 2,
        discontinuity_epoch: 1,
        ..first
    };
    assert!(!same_source_interval(aggregate, replacement));

    let sequence_gap = AudioChunkLineage {
        first_sequence_number: 31,
        last_sequence_number: 31,
        timestamp_start_ns: aggregate.timestamp_end_ns() + 10_000_000,
        ..aggregate
    };
    assert!(!contiguous_source_interval(aggregate, sequence_gap));
}

#[test]
fn source_gap_is_bounded_by_sequence_while_preserving_native_timestamps() {
    let first = AudioChunkLineage {
        session_id: 1,
        source_id: 202,
        stem_id: 7,
        clock_id: 9,
        first_sequence_number: 20,
        last_sequence_number: 20,
        missing_sequence_count: 0,
        inserted_silence_samples: 0,
        timestamp_start_ns: 1_000_000_000,
        duration_ns: SOURCE_FRAME_DURATION_NS,
        source_generation: 1,
        discontinuity_epoch: 0,
        permission_epoch: 3,
        observed_at_ns: 1_010_000_000,
        polled_at_ns: 1_011_000_000,
    };
    let one_missing_frame = AudioChunkLineage {
        first_sequence_number: 22,
        last_sequence_number: 22,
        timestamp_start_ns: 1_020_000_000,
        ..first
    };
    assert_eq!(missing_source_frames(first, one_missing_frame).unwrap(), 1);

    let inconsistent = AudioChunkLineage {
        first_sequence_number: 23,
        last_sequence_number: 23,
        ..one_missing_frame
    };
    assert!(missing_source_frames(first, inconsistent).is_err());

    let jittered_gap = AudioChunkLineage {
        first_sequence_number: 23,
        last_sequence_number: 23,
        timestamp_start_ns: 1_030_000_000 - 106_167,
        ..first
    };
    assert_eq!(missing_source_frames(first, jittered_gap).unwrap(), 2);

    let jittered = AudioChunkLineage {
        first_sequence_number: 21,
        last_sequence_number: 21,
        timestamp_start_ns: first.timestamp_end_ns().saturating_sub(106_167),
        ..first
    };
    assert!(contiguous_source_interval(first, jittered));

    let repeated_start = AudioChunkLineage {
        first_sequence_number: 21,
        last_sequence_number: 21,
        timestamp_start_ns: first.timestamp_start_ns,
        ..first
    };
    assert!(!contiguous_source_interval(first, repeated_start));
    assert!(missing_source_frames(first, repeated_start).is_err());

    let unbounded_timestamp = AudioChunkLineage {
        first_sequence_number: 21,
        last_sequence_number: 21,
        timestamp_start_ns: first
            .timestamp_end_ns()
            .saturating_add(MAX_SEQUENCE_GAP_DURATION.as_nanos() as u64)
            .saturating_add(1),
        ..first
    };
    assert!(!contiguous_source_interval(first, unbounded_timestamp));
    assert!(missing_source_frames(first, unbounded_timestamp).is_err());

    let adjacent_but_far_away = AudioChunkLineage {
        first_sequence_number: 21,
        last_sequence_number: 21,
        timestamp_start_ns: first.timestamp_end_ns().saturating_add(1_000_000_000),
        ..first
    };
    assert!(!contiguous_source_interval(first, adjacent_but_far_away));
    assert!(missing_source_frames(first, adjacent_but_far_away).is_err());

    let unbounded = AudioChunkLineage {
        first_sequence_number: 222,
        last_sequence_number: 222,
        ..first
    };
    assert!(missing_source_frames(first, unbounded).is_err());
}

#[test]
fn inserted_gap_silence_is_written_and_recorded_in_chunk_lineage() {
    let (sender, receiver) = crossbeam_channel::bounded(2);
    let dropped = AtomicU64::new(0);
    let mut writer = MicrophoneAudioChunkWriter::default();
    let template = AudioChunkLineage {
        session_id: 1,
        source_id: 202,
        stem_id: 7,
        clock_id: 9,
        first_sequence_number: 0,
        last_sequence_number: 0,
        missing_sequence_count: 0,
        inserted_silence_samples: 0,
        timestamp_start_ns: 1_000_000_000,
        duration_ns: SOURCE_FRAME_DURATION_NS,
        source_generation: 1,
        discontinuity_epoch: 0,
        permission_epoch: 3,
        observed_at_ns: 1_010_000_000,
        polled_at_ns: 1_011_000_000,
    };

    for sequence in 0..10 {
        let lineage = AudioChunkLineage {
            first_sequence_number: sequence,
            last_sequence_number: sequence,
            timestamp_start_ns: template.timestamp_start_ns + sequence * SOURCE_FRAME_DURATION_NS,
            ..template
        };
        writer
            .push_source_frame_samples(&[0.5; SOURCE_FRAME_SAMPLES], lineage, &sender, &dropped)
            .unwrap();
    }
    let first_chunk = receiver.try_recv().unwrap();
    assert_eq!(first_chunk.lineage.unwrap().last_sequence_number, 9);

    // Skip sequence 10 and its 10 ms timestamp interval exactly at an
    // emitted 100 ms chunk boundary. Gap detection must use the separate
    // latest-source-frame cursor rather than pending chunk aggregation.
    for sequence in 11..20 {
        let lineage = AudioChunkLineage {
            first_sequence_number: sequence,
            last_sequence_number: sequence,
            timestamp_start_ns: template.timestamp_start_ns + sequence * SOURCE_FRAME_DURATION_NS,
            ..template
        };
        writer
            .push_source_frame_samples(&[0.5; SOURCE_FRAME_SAMPLES], lineage, &sender, &dropped)
            .unwrap();
    }

    let chunk = receiver.try_recv().unwrap();
    assert_eq!(chunk.samples.len(), crate::streaming::CHUNK_SAMPLES);
    assert!(chunk.samples[..SOURCE_FRAME_SAMPLES]
        .iter()
        .all(|sample| *sample == 0.0));
    assert!(chunk.samples[SOURCE_FRAME_SAMPLES..]
        .iter()
        .all(|sample| *sample == 0.5));
    let lineage = chunk.lineage.unwrap();
    assert_eq!(lineage.first_sequence_number, 10);
    assert_eq!(lineage.last_sequence_number, 19);
    assert_eq!(lineage.missing_sequence_count, 1);
    assert_eq!(
        lineage.inserted_silence_samples,
        SOURCE_FRAME_SAMPLES as u64
    );
    assert_eq!(lineage.duration_ns, 10 * SOURCE_FRAME_DURATION_NS);
}

#[test]
fn drop_order_requests_stop_then_observes_cancel_before_join_returns() {
    let stop = Arc::new(AtomicBool::new(false));
    let cancel_completed = Arc::new(AtomicBool::new(false));
    let cancel_succeeded = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_cancel_completed = Arc::clone(&cancel_completed);
    let worker_cancel_succeeded = Arc::clone(&cancel_succeeded);
    let (finished_sender, finished) = crossbeam_channel::bounded(1);
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        worker_cancel_succeeded.store(true, Ordering::Release);
        worker_cancel_completed.store(true, Ordering::Release);
        finished_sender.send(()).unwrap();
    });
    let started = Instant::now();

    assert!(stop_cancel_and_join(
        &stop,
        Some(worker),
        &cancel_completed,
        &cancel_succeeded,
        &finished,
        Duration::from_secs(1),
    ));
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(stop.load(Ordering::Acquire));
    assert!(cancel_completed.load(Ordering::Acquire));
    assert!(cancel_succeeded.load(Ordering::Acquire));
}

#[test]
fn completed_but_unsuccessful_cancellation_is_not_reported_clean() {
    let stop = Arc::new(AtomicBool::new(false));
    let cancel_completed = Arc::new(AtomicBool::new(false));
    let cancel_succeeded = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker_cancel_completed = Arc::clone(&cancel_completed);
    let (finished_sender, finished) = crossbeam_channel::bounded(1);
    let worker = std::thread::spawn(move || {
        while !worker_stop.load(Ordering::Acquire) {
            std::thread::yield_now();
        }
        worker_cancel_completed.store(true, Ordering::Release);
        finished_sender.send(()).unwrap();
    });

    assert!(!stop_cancel_and_join(
        &stop,
        Some(worker),
        &cancel_completed,
        &cancel_succeeded,
        &finished,
        Duration::from_secs(1),
    ));
    assert!(cancel_completed.load(Ordering::Acquire));
    assert!(!cancel_succeeded.load(Ordering::Acquire));
}

#[test]
fn shutdown_wait_is_bounded_when_worker_does_not_finish() {
    let stop = Arc::new(AtomicBool::new(false));
    let cancel_completed = Arc::new(AtomicBool::new(false));
    let cancel_succeeded = Arc::new(AtomicBool::new(false));
    let (_finished_sender, finished) = crossbeam_channel::bounded(1);
    let worker = std::thread::spawn(|| std::thread::sleep(Duration::from_millis(200)));
    let started = Instant::now();

    assert!(!stop_cancel_and_join(
        &stop,
        Some(worker),
        &cancel_completed,
        &cancel_succeeded,
        &finished,
        Duration::from_millis(10),
    ));
    assert!(started.elapsed() < Duration::from_millis(100));
}

#[test]
fn saturated_minutes_consumer_drops_without_blocking_microphone_worker() {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let dropped = AtomicU64::new(0);
    let chunk = || AudioChunk {
        samples: vec![0.25; crate::streaming::CHUNK_SAMPLES],
        rms: 0.25,
        timestamp: Instant::now(),
        index: 0,
        source: SourceRole::Voice,
        lineage: None,
    };

    assert_eq!(deliver_chunk(&sender, chunk(), &dropped), Ok(()));
    let started = Instant::now();
    assert_eq!(deliver_chunk(&sender, chunk(), &dropped), Ok(()));
    assert!(started.elapsed() < Duration::from_millis(100));
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert_eq!(receiver.len(), 1);
}

#[test]
fn replacement_timeout_remains_pending_until_new_continuity_is_observed() {
    let outcome = MicrophoneReplacementOutcome::Pending {
        source_generation: 2,
        discontinuity_epoch: 1,
        timeout_ms: 1_000,
    };
    assert!(outcome.is_pending());
    assert_eq!(outcome.continuity(), (2, 1));

    let before = MicrophoneObservations {
        source_generation: 1,
        discontinuity_epoch: 0,
        ..MicrophoneObservations::default()
    };
    assert!(!replacement_continuity_reached(before, 2, 1));

    let after = MicrophoneObservations {
        source_generation: 2,
        discontinuity_epoch: 1,
        replacement: Some(SessionSourceReplacementObservations {
            stem_id: StemId::new(1),
            attempts_total: 1,
            completed_total: 0,
            failed_before_attach_total: 0,
            response_timeouts_total: 1,
            attached_source_id: None,
            source_generation: 2,
            discontinuity_epoch: 1,
            latest_completed_at_ns: None,
        }),
        ..MicrophoneObservations::default()
    };
    assert!(!replacement_continuity_reached(after, 2, 1));

    let attached = MicrophoneObservations {
        replacement: Some(SessionSourceReplacementObservations {
            attached_source_id: Some(pocketstation::SourceId::new(7)),
            completed_total: 1,
            latest_completed_at_ns: Some(5),
            ..after.replacement.expect("replacement observations")
        }),
        ..after
    };
    assert!(replacement_continuity_reached(attached, 2, 1));

    let later_attachment = MicrophoneObservations {
        source_generation: 3,
        discontinuity_epoch: 2,
        replacement: Some(SessionSourceReplacementObservations {
            source_generation: 3,
            discontinuity_epoch: 2,
            ..attached.replacement.expect("replacement observations")
        }),
        ..attached
    };
    assert!(
        !replacement_continuity_reached(later_attachment, 2, 1),
        "a later replacement must not be labelled with an older pending device identity"
    );
}
