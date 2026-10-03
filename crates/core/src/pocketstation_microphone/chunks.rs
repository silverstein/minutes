use super::capture_error;
use crate::error::CaptureError;
use crate::streaming::{AudioChunk, AudioChunkLineage, ChunkAccumulator, SourceRole};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const POCKETSTATION_SAMPLE_RATE_HZ: u32 = 48_000;
const MINUTES_SAMPLE_RATE_HZ: u32 = 16_000;
pub(super) const MAX_SEQUENCE_GAP_DURATION: Duration = Duration::from_secs(2);
const MAX_TIMESTAMP_JITTER: Duration = Duration::from_millis(2);
pub(super) const SOURCE_FRAME_DURATION_NS: u64 = 10_000_000;
pub(super) const SOURCE_FRAME_SAMPLES: usize =
    (MINUTES_SAMPLE_RATE_HZ as usize * SOURCE_FRAME_DURATION_NS as usize) / 1_000_000_000;

#[derive(Default)]
pub(super) struct MicrophoneAudioChunkWriter {
    downsample_phase_samples: usize,
    resampled_samples: Vec<f32>,
    chunks: ChunkAccumulator,
    pending_lineage: Option<AudioChunkLineage>,
    latest_source_frame: Option<AudioChunkLineage>,
}

impl MicrophoneAudioChunkWriter {
    pub(super) fn reset_for_discontinuity(&mut self) {
        self.downsample_phase_samples = 0;
        self.resampled_samples.clear();
        self.chunks.clear();
        self.pending_lineage = None;
        self.latest_source_frame = None;
    }

    pub(super) fn write_frame(
        &mut self,
        frame: pocketstation::PolledAudioFrame<'_>,
        sink: &crossbeam_channel::Sender<AudioChunk>,
        dropped_chunks_total: &AtomicU64,
    ) -> Result<(), CaptureError> {
        if frame.sample_rate_hz() != POCKETSTATION_SAMPLE_RATE_HZ {
            return Err(capture_error(
                "read PocketStation microphone",
                format!(
                    "expected {POCKETSTATION_SAMPLE_RATE_HZ} Hz canonical audio, received {} Hz",
                    frame.sample_rate_hz()
                ),
            ));
        }
        let channel_count = usize::from(frame.channels());
        if channel_count == 0 || !frame.samples().len().is_multiple_of(channel_count) {
            return Err(capture_error(
                "read PocketStation microphone",
                "received an invalid channel layout",
            ));
        }

        let lineage = frame.lineage();
        let frame_lineage = AudioChunkLineage {
            session_id: lineage.session_id().get(),
            source_id: lineage.source_id().get(),
            stem_id: lineage.stem_id().get(),
            clock_id: lineage.clock_id().get(),
            first_sequence_number: lineage.sequence_number(),
            last_sequence_number: lineage.sequence_number(),
            missing_sequence_count: 0,
            inserted_silence_samples: 0,
            timestamp_start_ns: lineage.timestamp_start_ns(),
            duration_ns: lineage.duration_ns(),
            source_generation: lineage.source_generation(),
            discontinuity_epoch: lineage.discontinuity_epoch(),
            permission_epoch: lineage.permission_epoch(),
            observed_at_ns: frame.route_received_at_ns(),
            polled_at_ns: frame.polled_at_ns(),
        };
        self.resampled_samples.clear();
        let downsample_ratio = (POCKETSTATION_SAMPLE_RATE_HZ / MINUTES_SAMPLE_RATE_HZ) as usize;
        for samples in frame.samples().chunks_exact(channel_count) {
            let mono = samples.iter().copied().sum::<f32>() / channel_count as f32;
            if self.downsample_phase_samples == 0 {
                self.resampled_samples.push(mono);
            }
            self.downsample_phase_samples = (self.downsample_phase_samples + 1) % downsample_ratio;
        }

        let resampled_samples = std::mem::take(&mut self.resampled_samples);
        let result = self.push_source_frame_samples(
            &resampled_samples,
            frame_lineage,
            sink,
            dropped_chunks_total,
        );
        self.resampled_samples = resampled_samples;
        result
    }

    pub(super) fn push_source_frame_samples(
        &mut self,
        samples: &[f32],
        frame_lineage: AudioChunkLineage,
        sink: &crossbeam_channel::Sender<AudioChunk>,
        dropped_chunks_total: &AtomicU64,
    ) -> Result<(), CaptureError> {
        if let Some(previous) = self.latest_source_frame {
            if !same_source_interval(previous, frame_lineage) {
                self.reset_for_discontinuity();
            } else if !contiguous_source_interval(previous, frame_lineage) {
                let missing_frames = missing_source_frames(previous, frame_lineage)?;
                for offset in 0..missing_frames {
                    let sequence_number = previous
                        .last_sequence_number
                        .saturating_add(offset)
                        .saturating_add(1);
                    let missing_lineage = AudioChunkLineage {
                        first_sequence_number: sequence_number,
                        last_sequence_number: sequence_number,
                        missing_sequence_count: 1,
                        inserted_silence_samples: SOURCE_FRAME_SAMPLES as u64,
                        timestamp_start_ns: previous
                            .timestamp_end_ns()
                            .saturating_add(offset.saturating_mul(SOURCE_FRAME_DURATION_NS)),
                        duration_ns: SOURCE_FRAME_DURATION_NS,
                        observed_at_ns: frame_lineage.observed_at_ns,
                        polled_at_ns: frame_lineage.polled_at_ns,
                        ..frame_lineage
                    };
                    self.push_samples(
                        &[0.0; SOURCE_FRAME_SAMPLES],
                        missing_lineage,
                        sink,
                        dropped_chunks_total,
                    )?;
                }
            }
        }

        self.push_samples(samples, frame_lineage, sink, dropped_chunks_total)?;
        self.latest_source_frame = Some(frame_lineage);
        Ok(())
    }

    fn push_samples(
        &mut self,
        samples: &[f32],
        lineage: AudioChunkLineage,
        sink: &crossbeam_channel::Sender<AudioChunk>,
        dropped_chunks_total: &AtomicU64,
    ) -> Result<(), CaptureError> {
        self.pending_lineage = Some(merge_source_interval(self.pending_lineage, lineage));
        let mut sink_disconnected = false;
        let pending_lineage = &mut self.pending_lineage;
        self.chunks.push(samples, |index, samples| {
            let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
                / samples.len() as f32)
                .sqrt();
            let chunk = AudioChunk {
                samples,
                rms,
                timestamp: Instant::now(),
                index,
                source: SourceRole::Voice,
                lineage: pending_lineage.take(),
            };
            if deliver_chunk(sink, chunk, dropped_chunks_total).is_err() {
                sink_disconnected = true;
            }
        });
        if sink_disconnected {
            return Err(capture_error(
                "deliver PocketStation microphone",
                "Minutes stopped receiving microphone audio",
            ));
        }
        Ok(())
    }
}

pub(super) fn deliver_chunk(
    sink: &crossbeam_channel::Sender<AudioChunk>,
    chunk: AudioChunk,
    dropped_chunks_total: &AtomicU64,
) -> Result<(), ()> {
    match sink.try_send(chunk) {
        Ok(()) => Ok(()),
        Err(crossbeam_channel::TrySendError::Full(_)) => {
            dropped_chunks_total.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
        Err(crossbeam_channel::TrySendError::Disconnected(_)) => Err(()),
    }
}

pub(super) fn same_source_interval(left: AudioChunkLineage, right: AudioChunkLineage) -> bool {
    left.session_id == right.session_id
        && left.source_id == right.source_id
        && left.stem_id == right.stem_id
        && left.clock_id == right.clock_id
        && left.source_generation == right.source_generation
        && left.discontinuity_epoch == right.discontinuity_epoch
        && left.permission_epoch == right.permission_epoch
}

pub(super) fn contiguous_source_interval(
    left: AudioChunkLineage,
    right: AudioChunkLineage,
) -> bool {
    same_source_interval(left, right)
        && right.first_sequence_number == left.last_sequence_number.saturating_add(1)
        && source_timestamps_plausible(left, right)
}

fn source_timestamps_plausible(left: AudioChunkLineage, right: AudioChunkLineage) -> bool {
    if right.timestamp_start_ns <= left.timestamp_start_ns {
        return false;
    }
    let Some(missing_sequences) = right
        .first_sequence_number
        .checked_sub(left.last_sequence_number.saturating_add(1))
    else {
        return false;
    };
    let expected_start_ns = left
        .timestamp_end_ns()
        .saturating_add(missing_sequences.saturating_mul(SOURCE_FRAME_DURATION_NS));
    right.timestamp_start_ns.abs_diff(expected_start_ns) <= MAX_TIMESTAMP_JITTER.as_nanos() as u64
}

pub(super) fn missing_source_frames(
    left: AudioChunkLineage,
    right: AudioChunkLineage,
) -> Result<u64, CaptureError> {
    if !source_timestamps_plausible(left, right) {
        return Err(capture_error(
            "preserve PocketStation microphone timeline",
            "source timestamps did not advance within the bounded repair window",
        ));
    }
    let missing_sequences = right
        .first_sequence_number
        .checked_sub(left.last_sequence_number.saturating_add(1))
        .ok_or_else(|| {
            capture_error(
                "preserve PocketStation microphone timeline",
                "source sequence moved backwards or overlapped",
            )
        })?;
    let missing_duration_ns = missing_sequences.saturating_mul(SOURCE_FRAME_DURATION_NS);
    if missing_duration_ns > MAX_SEQUENCE_GAP_DURATION.as_nanos() as u64 {
        return Err(capture_error(
            "preserve PocketStation microphone timeline",
            format!(
                "source gap of {missing_duration_ns} ns exceeds the bounded {} ns repair window",
                MAX_SEQUENCE_GAP_DURATION.as_nanos()
            ),
        ));
    }
    Ok(missing_sequences)
}

pub(super) fn merge_source_interval(
    current: Option<AudioChunkLineage>,
    next: AudioChunkLineage,
) -> AudioChunkLineage {
    let Some(mut current) = current else {
        return next;
    };
    debug_assert!(same_source_interval(current, next));
    current.last_sequence_number = next.last_sequence_number;
    current.missing_sequence_count = current
        .missing_sequence_count
        .saturating_add(next.missing_sequence_count);
    current.inserted_silence_samples = current
        .inserted_silence_samples
        .saturating_add(next.inserted_silence_samples);
    current.duration_ns = next
        .timestamp_end_ns()
        .saturating_sub(current.timestamp_start_ns);
    current.observed_at_ns = current.observed_at_ns.min(next.observed_at_ns);
    current.polled_at_ns = current.polled_at_ns.max(next.polled_at_ns);
    current
}
