//! Checked resource limits for in-process audio decoding.
//!
//! Container byte length is not a useful upper bound for decoded PCM: a tiny
//! low-rate or highly compressed input can expand into an enormous sample
//! vector. These limits are enforced against actual decoded frames and the
//! canonical 16 kHz representation before allocation.

use std::collections::VecDeque;
use std::io::{Error, ErrorKind};
use std::time::{Duration, Instant};
use zeroize::Zeroize;

pub(crate) const CANONICAL_SAMPLE_RATE: u32 = 16_000;
pub(crate) const MAX_AUDIO_SECONDS: u64 = 4 * 60 * 60;
pub(crate) const MAX_CANONICAL_SAMPLES: usize =
    CANONICAL_SAMPLE_RATE as usize * MAX_AUDIO_SECONDS as usize;
pub(crate) const MAX_AUDIO_CHANNELS: usize = 32;
pub(crate) const MAX_AUDIO_SAMPLE_RATE: u32 = 384_000;
pub(crate) const AUDIO_DECODE_DEADLINE: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy)]
pub(crate) struct AudioWorkBudget {
    deadline: Instant,
}

impl AudioWorkBudget {
    pub(crate) fn new() -> Self {
        Self {
            deadline: Instant::now() + AUDIO_DECODE_DEADLINE,
        }
    }

    #[cfg(test)]
    fn already_expired() -> Self {
        Self {
            deadline: Instant::now() - Duration::from_millis(1),
        }
    }

    pub(crate) fn check_deadline(self) -> std::io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(resource_error("audio decode deadline exceeded"));
        }
        Ok(())
    }

    pub(crate) fn validate_stream(self, sample_rate: u32, channels: usize) -> std::io::Result<()> {
        self.check_deadline()?;
        if sample_rate == 0
            || sample_rate > MAX_AUDIO_SAMPLE_RATE
            || channels == 0
            || channels > MAX_AUDIO_CHANNELS
        {
            return Err(resource_error(
                "audio stream dimensions exceed the resource budget",
            ));
        }
        Ok(())
    }

    #[cfg_attr(not(any(test, feature = "diarize")), allow(dead_code))]
    pub(crate) fn check_source_frames(
        self,
        frames: usize,
        sample_rate: u32,
    ) -> std::io::Result<()> {
        self.check_deadline()?;
        let duration_frames = max_source_frames(sample_rate)?;
        if frames as u64 > duration_frames {
            return Err(resource_error("decoded audio exceeds the resource budget"));
        }
        Ok(())
    }

    pub(crate) const fn max_pcm_s16le_bytes() -> u64 {
        (MAX_CANONICAL_SAMPLES as u64).saturating_mul(std::mem::size_of::<i16>() as u64)
    }
}

pub(crate) fn max_source_frames(sample_rate: u32) -> std::io::Result<u64> {
    (sample_rate as u64)
        .checked_mul(MAX_AUDIO_SECONDS)
        .ok_or_else(|| resource_error("audio duration budget overflowed"))
}

pub(crate) fn resource_error(message: &'static str) -> Error {
    Error::new(ErrorKind::OutOfMemory, message)
}

/// Incremental windowed-sinc resampler for decoded mono frames.
///
/// Only the small source window needed by the next output sample is retained;
/// the canonical output is the sole full-duration allocation. This matters for
/// common 44.1/48 kHz meeting WAVs: retaining the entire high-rate source before
/// resampling either imposed a rate-dependent duration cap or required several
/// gigabytes for a multi-hour recording.
pub(crate) struct StreamingMonoResampler {
    from_rate: u32,
    to_rate: u32,
    budget: AudioWorkBudget,
    max_output_samples: usize,
    source: VecDeque<f32>,
    source_base: u64,
    source_frames: u64,
    output_index: usize,
    output: Vec<f32>,
}

impl StreamingMonoResampler {
    const HALF_WIDTH: i64 = 16;

    pub(crate) fn new(
        from_rate: u32,
        to_rate: u32,
        budget: AudioWorkBudget,
        max_output_samples: usize,
    ) -> std::io::Result<Self> {
        budget.validate_stream(from_rate, 1)?;
        budget.validate_stream(to_rate, 1)?;
        Ok(Self {
            from_rate,
            to_rate,
            budget,
            max_output_samples: max_output_samples.min(MAX_CANONICAL_SAMPLES),
            source: VecDeque::with_capacity((Self::HALF_WIDTH as usize) * 2 + 2),
            source_base: 0,
            source_frames: 0,
            output_index: 0,
            output: Vec::new(),
        })
    }

    pub(crate) fn push_mono_sample(&mut self, sample: f32) -> std::io::Result<()> {
        self.push_mono_sample_with_cancel(sample, || Ok(()))
    }

    pub(crate) fn source_frames(&self) -> u64 {
        self.source_frames
    }

    pub(crate) fn push_mono_sample_with_cancel<F>(
        &mut self,
        sample: f32,
        mut check_cancelled: F,
    ) -> std::io::Result<()>
    where
        F: FnMut() -> std::io::Result<()>,
    {
        let result = (|| {
            if self.source_frames & 0x0fff == 0 {
                self.budget
                    .check_deadline()
                    .and_then(|_| check_cancelled())?;
            }
            let next_frames = self
                .source_frames
                .checked_add(1)
                .ok_or_else(|| resource_error("decoded audio sample count overflowed"))?;
            if next_frames > max_source_frames(self.from_rate)? {
                return Err(resource_error("decoded audio exceeds the resource budget"));
            }
            self.source_frames = next_frames;

            if self.from_rate == self.to_rate {
                return self.push_output(sample);
            }

            self.source.push_back(sample);
            self.produce_ready_output(false)
        })();
        if result.is_err() {
            self.zeroize_buffers();
        }
        result
    }

    pub(crate) fn finish(mut self) -> std::io::Result<Vec<f32>> {
        self.budget.check_deadline()?;
        if self.from_rate == self.to_rate {
            return Ok(std::mem::take(&mut self.output));
        }
        self.produce_ready_output(true)?;
        let output = std::mem::take(&mut self.output);
        Ok(output)
    }

    fn target_output_len(&self) -> std::io::Result<usize> {
        let output_len = (self.source_frames as u128)
            .checked_mul(self.to_rate as u128)
            .ok_or_else(|| resource_error("resampled audio length overflowed"))?
            / self.from_rate as u128;
        let output_len = usize::try_from(output_len)
            .map_err(|_| resource_error("resampled audio length overflowed"))?;
        if output_len > self.max_output_samples {
            return Err(resource_error(
                "resampled audio exceeds the resource budget",
            ));
        }
        Ok(output_len)
    }

    fn produce_ready_output(&mut self, finishing: bool) -> std::io::Result<()> {
        let final_output_len = finishing.then(|| self.target_output_len()).transpose()?;
        loop {
            if let Some(final_output_len) = final_output_len {
                if self.output_index >= final_output_len {
                    break;
                }
            } else if !self.next_output_has_full_lookahead() {
                break;
            }
            self.produce_one()?;
            self.evict_consumed_source();
        }
        Ok(())
    }

    fn next_output_has_full_lookahead(&self) -> bool {
        let source_position = self.output_index as f64 * self.ratio();
        let source_center = source_position as i64;
        source_center + Self::HALF_WIDTH < self.source_frames as i64
    }

    fn produce_one(&mut self) -> std::io::Result<()> {
        if self.output_index & 0x0fff == 0 {
            self.budget.check_deadline()?;
        }
        let source_position = self.output_index as f64 * self.ratio();
        let source_center = source_position as i64;
        let cutoff = if self.to_rate < self.from_rate {
            self.to_rate as f64 / self.from_rate as f64
        } else {
            1.0
        };
        let mut sum = 0.0_f64;
        let mut weight_sum = 0.0_f64;
        for source_index in
            (source_center - Self::HALF_WIDTH + 1)..=(source_center + Self::HALF_WIDTH)
        {
            if source_index < 0 || source_index as u64 >= self.source_frames {
                continue;
            }
            let Some(sample) = self.source_sample(source_index as u64) else {
                return Err(resource_error(
                    "streaming resampler source window underflowed",
                ));
            };
            let delta = source_position - source_index as f64;
            let sinc = if delta.abs() < 1e-10 {
                cutoff
            } else {
                let x = std::f64::consts::PI * delta * cutoff;
                (x.sin() / (std::f64::consts::PI * delta)) * cutoff
            };
            let window_position = (delta / Self::HALF_WIDTH as f64 + 1.0) * 0.5;
            let window = if (0.0..=1.0).contains(&window_position) {
                0.5 * (1.0 - (2.0 * std::f64::consts::PI * window_position).cos())
            } else {
                0.0
            };
            let weight = sinc * window;
            sum += sample as f64 * weight;
            weight_sum += weight;
        }
        let output = if weight_sum.abs() > 1e-10 {
            (sum / weight_sum) as f32
        } else {
            0.0
        };
        self.push_output(output)?;
        self.output_index += 1;
        Ok(())
    }

    fn source_sample(&self, source_index: u64) -> Option<f32> {
        let offset = source_index.checked_sub(self.source_base)?;
        self.source.get(usize::try_from(offset).ok()?).copied()
    }

    fn evict_consumed_source(&mut self) {
        let next_center = (self.output_index as f64 * self.ratio()) as i64;
        let retain_from = (next_center - Self::HALF_WIDTH + 1).max(0) as u64;
        while self.source_base < retain_from {
            if let Some(mut sample) = self.source.pop_front() {
                sample.zeroize();
            } else {
                break;
            }
            self.source_base += 1;
        }
    }

    fn push_output(&mut self, sample: f32) -> std::io::Result<()> {
        let next_len = self
            .output
            .len()
            .checked_add(1)
            .ok_or_else(|| resource_error("resampled audio sample count overflowed"))?;
        if next_len > self.max_output_samples {
            return Err(resource_error(
                "resampled audio exceeds the resource budget",
            ));
        }
        if self.output.len() == self.output.capacity() {
            self.output
                .try_reserve(16_384.min(self.max_output_samples - self.output.len()))
                .map_err(|_| resource_error("resampled audio allocation failed"))?;
        }
        self.output.push(sample);
        Ok(())
    }

    fn ratio(&self) -> f64 {
        self.from_rate as f64 / self.to_rate as f64
    }

    fn zeroize_buffers(&mut self) {
        for sample in &mut self.source {
            sample.zeroize();
        }
        self.source.clear();
        self.output.zeroize();
        self.output.clear();
    }
}

impl Drop for StreamingMonoResampler {
    fn drop(&mut self) {
        self.zeroize_buffers();
    }
}

/// Fallible copy of the project's windowed-sinc resampler with checked output
/// arithmetic and periodic deadline enforcement. Keeping it in minutes-core
/// avoids changing the independently published whisper-guard crate.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn resample_with_budget(
    samples: &[f32],
    from_rate: u32,
    to_rate: u32,
    budget: AudioWorkBudget,
) -> std::io::Result<Vec<f32>> {
    resample_with_budget_and_limit(
        samples,
        from_rate,
        to_rate,
        budget,
        MAX_CANONICAL_SAMPLES,
        || Ok(()),
    )
}

/// Resample with a caller-specific output cap and cooperative cancellation.
/// Optional consumers such as diarization have tighter duration limits than
/// the transcription-wide budget and must not allocate their full output
/// before discovering that limit or a caller timeout.
#[cfg_attr(not(any(test, feature = "diarize")), allow(dead_code))]
pub(crate) fn resample_with_budget_and_limit<F>(
    samples: &[f32],
    from_rate: u32,
    to_rate: u32,
    budget: AudioWorkBudget,
    max_output_samples: usize,
    mut check_cancelled: F,
) -> std::io::Result<Vec<f32>>
where
    F: FnMut() -> std::io::Result<()>,
{
    budget.validate_stream(from_rate, 1)?;
    budget.validate_stream(to_rate, 1)?;
    budget.check_source_frames(samples.len(), from_rate)?;
    check_cancelled()?;

    let output_len = (samples.len() as u128)
        .checked_mul(to_rate as u128)
        .ok_or_else(|| resource_error("resampled audio length overflowed"))?
        / from_rate as u128;
    let output_len = usize::try_from(output_len)
        .map_err(|_| resource_error("resampled audio length overflowed"))?;
    if output_len > MAX_CANONICAL_SAMPLES || output_len > max_output_samples {
        return Err(resource_error(
            "resampled audio exceeds the resource budget",
        ));
    }
    if from_rate == to_rate {
        let mut output = Vec::new();
        output
            .try_reserve_exact(samples.len())
            .map_err(|_| resource_error("resampled audio allocation failed"))?;
        output.extend_from_slice(samples);
        return Ok(output);
    }

    let mut output = Vec::new();
    output
        .try_reserve_exact(output_len)
        .map_err(|_| resource_error("resampled audio allocation failed"))?;
    let ratio = from_rate as f64 / to_rate as f64;
    let cutoff = if to_rate < from_rate {
        to_rate as f64 / from_rate as f64
    } else {
        1.0
    };
    const HALF_WIDTH: i64 = 16;

    for index in 0..output_len {
        if index & 0x0fff == 0 {
            budget.check_deadline()?;
            check_cancelled()?;
        }
        let source_position = index as f64 * ratio;
        let source_center = source_position as i64;
        let mut sum = 0.0_f64;
        let mut weight_sum = 0.0_f64;
        for source_index in (source_center - HALF_WIDTH + 1)..=(source_center + HALF_WIDTH) {
            if source_index < 0 || source_index as usize >= samples.len() {
                continue;
            }
            let delta = source_position - source_index as f64;
            let sinc = if delta.abs() < 1e-10 {
                cutoff
            } else {
                let x = std::f64::consts::PI * delta * cutoff;
                (x.sin() / (std::f64::consts::PI * delta)) * cutoff
            };
            let window_position = (delta / HALF_WIDTH as f64 + 1.0) * 0.5;
            let window = if (0.0..=1.0).contains(&window_position) {
                0.5 * (1.0 - (2.0 * std::f64::consts::PI * window_position).cos())
            } else {
                0.0
            };
            let weight = sinc * window;
            sum += samples[source_index as usize] as f64 * weight;
            weight_sum += weight;
        }
        output.push(if weight_sum.abs() > 1e-10 {
            (sum / weight_sum) as f32
        } else {
            0.0
        });
    }
    Ok(output)
}

pub(crate) fn normalize_in_place(samples: &mut [f32]) {
    let peak = samples
        .iter()
        .map(|sample| sample.abs())
        .fold(0.0_f32, f32::max);
    const TARGET_PEAK: f32 = 0.5;
    const QUIET_THRESHOLD: f32 = 0.1;
    const NOISE_FLOOR: f32 = 0.0001;
    if peak < QUIET_THRESHOLD && peak > NOISE_FLOOR {
        let gain = (TARGET_PEAK / peak).min(100.0);
        for sample in samples {
            *sample = (*sample * gain).clamp(-1.0, 1.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_and_extreme_resample_budgets_fail_before_allocation() {
        let expired = resample_with_budget(
            &[0.0; 32],
            16_000,
            16_000,
            AudioWorkBudget::already_expired(),
        )
        .unwrap_err();
        assert_eq!(expired.kind(), ErrorKind::OutOfMemory);

        let extreme =
            resample_with_budget(&[0.0; 16_001], 1, 384_000, AudioWorkBudget::new()).unwrap_err();
        assert_eq!(extreme.kind(), ErrorKind::OutOfMemory);
    }

    #[test]
    fn bounded_resampler_preserves_expected_length_and_finite_samples() {
        let source = (0..48_000)
            .map(|index| ((index as f32 / 48_000.0) * std::f32::consts::TAU * 440.0).sin())
            .collect::<Vec<_>>();
        let output = resample_with_budget(&source, 48_000, 16_000, AudioWorkBudget::new()).unwrap();
        assert_eq!(output.len(), 16_000);
        assert!(output.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn caller_limit_and_cancellation_fail_before_resample_allocation() {
        let limited = resample_with_budget_and_limit(
            &[0.0; 8_000],
            8_000,
            16_000,
            AudioWorkBudget::new(),
            15_999,
            || Ok(()),
        )
        .unwrap_err();
        assert_eq!(limited.kind(), ErrorKind::OutOfMemory);

        let cancelled = resample_with_budget_and_limit(
            &[0.0; 8_000],
            8_000,
            16_000,
            AudioWorkBudget::new(),
            16_000,
            || Err(Error::new(ErrorKind::TimedOut, "cancelled")),
        )
        .unwrap_err();
        assert_eq!(cancelled.kind(), ErrorKind::TimedOut);
    }

    #[test]
    fn one_hour_high_rate_sources_fit_the_duration_budget_without_allocation() {
        for sample_rate in [44_100_u32, 48_000_u32] {
            let one_hour_frames = u64::from(sample_rate) * 60 * 60;
            assert!(max_source_frames(sample_rate).unwrap() >= one_hour_frames);
            let canonical_samples =
                one_hour_frames * u64::from(CANONICAL_SAMPLE_RATE) / u64::from(sample_rate);
            assert_eq!(canonical_samples, 60 * 60 * 16_000);
            assert!(canonical_samples <= MAX_CANONICAL_SAMPLES as u64);
        }
    }

    #[test]
    fn streaming_resampler_matches_batch_windowed_sinc() {
        let source = (0..48_000)
            .map(|index| ((index as f32 / 48_000.0) * std::f32::consts::TAU * 440.0).sin())
            .collect::<Vec<_>>();
        let expected =
            resample_with_budget(&source, 48_000, 16_000, AudioWorkBudget::new()).unwrap();
        let mut streaming = StreamingMonoResampler::new(
            48_000,
            16_000,
            AudioWorkBudget::new(),
            MAX_CANONICAL_SAMPLES,
        )
        .unwrap();
        let mut max_retained_source = 0;
        for sample in source {
            streaming.push_mono_sample(sample).unwrap();
            max_retained_source = max_retained_source.max(streaming.source.len());
        }
        assert!(max_retained_source <= 34);
        let actual = streaming.finish().unwrap();
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
        }
    }

    #[test]
    fn streaming_resampler_zeroizes_retained_audio_on_cancellation() {
        let mut streaming = StreamingMonoResampler::new(
            48_000,
            16_000,
            AudioWorkBudget::new(),
            MAX_CANONICAL_SAMPLES,
        )
        .unwrap();
        for _ in 0..8_192 {
            streaming.push_mono_sample(0.25).unwrap();
        }
        let error = streaming
            .push_mono_sample_with_cancel(0.25, || {
                Err(Error::new(ErrorKind::Interrupted, "cancelled"))
            })
            .unwrap_err();
        assert_eq!(error.kind(), ErrorKind::Interrupted);
        assert!(streaming.source.is_empty());
        assert!(streaming.output.is_empty());
    }

    #[test]
    fn streaming_resampler_zeroizes_retained_audio_on_budget_error() {
        let mut streaming =
            StreamingMonoResampler::new(48_000, 16_000, AudioWorkBudget::new(), 0).unwrap();
        let error = loop {
            if let Err(error) = streaming.push_mono_sample(0.25) {
                break error;
            }
        };
        assert_eq!(error.kind(), ErrorKind::OutOfMemory);
        assert!(streaming.source.is_empty());
        assert!(streaming.output.is_empty());
    }
}
