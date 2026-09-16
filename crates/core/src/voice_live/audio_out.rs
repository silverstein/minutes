//! Speaker playback for model audio.
//!
//! The provider sends 24 kHz PCM16 mono. This module opens the default output
//! device through cpal (already a dependency), keeps a queue of samples at the
//! device rate, and fills the device callback from it. `flush` empties the queue
//! for barge-in. Linear interpolation is enough for speech at these rates.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use super::protocol::OUTPUT_SAMPLE_RATE;
use super::VoiceLiveError;

/// Shared queue plus the stream that drains it.
pub struct Playback {
    queue: Arc<Mutex<VecDeque<f32>>>,
    _stream: Stream,
    device_rate: u32,
    channels: usize,
    /// Read position carried across chunks for the upsampler, in units of
    /// 1/device_rate of an input sample (integer so chunking never changes output).
    phase: u64,
    last_sample: f32,
    pub device_name: String,
}

// cpal::Stream is !Send on some hosts. The session thread owns Playback and never
// shares it, so hand-marking Send is sound for our single-owner usage.
unsafe impl Send for Playback {}

impl Playback {
    /// Open the default output device.
    pub fn open() -> Result<Self, VoiceLiveError> {
        let host = crate::capture::cached_default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| VoiceLiveError::Audio("no default output device".into()))?;
        let device_name = device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "output".into());
        let supported = device
            .default_output_config()
            .map_err(|e| VoiceLiveError::Audio(format!("output config: {e}")))?;
        let sample_format = supported.sample_format();
        let config: StreamConfig = supported.config();
        let device_rate = config.sample_rate;
        let channels = config.channels as usize;
        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::with_capacity(
            device_rate as usize * 4,
        )));
        let err_fn = |e| tracing::warn!(error = %e, "voice playback stream error");

        let stream = match sample_format {
            SampleFormat::F32 => {
                let q = Arc::clone(&queue);
                device
                    .build_output_stream(
                        config,
                        move |data: &mut [f32], _| fill(&q, data, channels, |s| s),
                        err_fn,
                        None,
                    )
                    .map_err(|e| VoiceLiveError::Audio(format!("build output stream: {e}")))?
            }
            SampleFormat::I16 => {
                let q = Arc::clone(&queue);
                device
                    .build_output_stream(
                        config,
                        move |data: &mut [i16], _| {
                            fill(&q, data, channels, |s| (s * 32767.0) as i16)
                        },
                        err_fn,
                        None,
                    )
                    .map_err(|e| VoiceLiveError::Audio(format!("build output stream: {e}")))?
            }
            other => {
                return Err(VoiceLiveError::Audio(format!(
                    "unsupported output sample format {other:?}"
                )))
            }
        };
        stream
            .play()
            .map_err(|e| VoiceLiveError::Audio(format!("start output stream: {e}")))?;
        Ok(Self {
            queue,
            _stream: stream,
            device_rate,
            channels,
            phase: 0,
            last_sample: 0.0,
            device_name,
        })
    }

    /// Queue one chunk of little-endian PCM16 at 24 kHz.
    pub fn push_pcm16(&mut self, bytes: &[u8]) {
        let samples: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        let resampled = self.upsample(&samples);
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.extend(resampled);
    }

    /// Drop everything queued (barge-in).
    pub fn flush(&mut self) {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.clear();
        self.phase = 0;
        self.last_sample = 0.0;
    }

    /// Seconds of audio still queued.
    pub fn queued_seconds(&self) -> f32 {
        let q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.len() as f32 / self.device_rate as f32
    }

    /// True when nothing is left to play.
    pub fn is_idle(&self) -> bool {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
    }

    fn upsample(&mut self, input: &[f32]) -> Vec<f32> {
        resample_linear(
            input,
            OUTPUT_SAMPLE_RATE,
            self.device_rate,
            &mut self.phase,
            &mut self.last_sample,
        )
    }

    /// Output channel count, exposed for diagnostics.
    pub fn channels(&self) -> usize {
        self.channels
    }
}

fn fill<T: Copy>(
    queue: &Arc<Mutex<VecDeque<f32>>>,
    data: &mut [T],
    channels: usize,
    convert: impl Fn(f32) -> T,
) {
    let mut q = queue.lock().unwrap_or_else(|p| p.into_inner());
    for frame in data.chunks_mut(channels.max(1)) {
        let s = q.pop_front().unwrap_or(0.0);
        for out in frame.iter_mut() {
            *out = convert(s);
        }
    }
}

/// Linear interpolation from `from_rate` to `to_rate`.
///
/// `phase` is the read position in units of `1/to_rate` input samples and `last`
/// is the final input sample of the previous chunk. Both carry across calls, and
/// the arithmetic is integer, so resampling a stream chunk by chunk produces
/// exactly the samples resampling it whole would.
pub fn resample_linear(
    input: &[f32],
    from_rate: u32,
    to_rate: u32,
    phase: &mut u64,
    last: &mut f32,
) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if from_rate == to_rate {
        *last = input[input.len() - 1];
        return input.to_vec();
    }
    let from = from_rate as u64;
    let to = to_rate as u64;
    let len = input.len();
    let mut out = Vec::with_capacity(len * to as usize / from as usize + 2);
    // Virtual input: previous last sample followed by this chunk, so index 0 is `last`.
    loop {
        let i = (*phase / to) as usize;
        if i >= len {
            break;
        }
        let t = (*phase % to) as f32 / to as f32;
        let a = if i == 0 { *last } else { input[i - 1] };
        let b = input[i];
        out.push(a + (b - a) * t);
        *phase += from;
    }
    *phase -= len as u64 * to;
    *last = input[len - 1];
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_rate_is_identity() {
        let (mut f, mut l) = (0u64, 0.0f32);
        assert_eq!(
            resample_linear(&[0.1, 0.2], 24000, 24000, &mut f, &mut l),
            vec![0.1, 0.2]
        );
    }

    #[test]
    fn upsampling_doubles_length_within_one_sample() {
        let (mut f, mut l) = (0u64, 0.0f32);
        let input: Vec<f32> = (0..240).map(|i| i as f32 / 240.0).collect();
        let out = resample_linear(&input, 24000, 48000, &mut f, &mut l);
        assert!((out.len() as i64 - 480).abs() <= 1, "len {}", out.len());
        // Monotonic ramp in, monotonic ramp out.
        assert!(out.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn chunk_boundaries_are_continuous() {
        let (mut f, mut l) = (0u64, 0.0f32);
        let input: Vec<f32> = (0..480).map(|i| (i as f32 * 0.05).sin()).collect();
        let whole = resample_linear(&input, 24000, 44100, &mut f.clone(), &mut l.clone());
        let mut split = resample_linear(&input[..200], 24000, 44100, &mut f, &mut l);
        split.extend(resample_linear(&input[200..], 24000, 44100, &mut f, &mut l));
        assert_eq!(whole.len(), split.len());
        for (a, b) in whole.iter().zip(&split) {
            assert_eq!(a, b);
        }
    }
}
