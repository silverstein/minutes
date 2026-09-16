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

/// Speech and generated music share a device, never a FIFO. A short tail keeps
/// music silent between adjacent speech packets instead of chattering over them.
pub(super) struct OutputQueue {
    speech: VecDeque<f32>,
    music: Option<VecDeque<f32>>,
    paused: bool,
    speech_tail: usize,
    tail_samples: usize,
}

impl OutputQueue {
    pub fn new(rate: u32) -> Self {
        Self {
            speech: VecDeque::new(),
            music: None,
            paused: false,
            speech_tail: 0,
            tail_samples: rate as usize / 4,
        }
    }

    pub fn extend(&mut self, samples: impl IntoIterator<Item = f32>) {
        self.speech.extend(samples);
    }

    pub fn replace_music(&mut self, samples: Vec<f32>) {
        self.music = Some(samples.into());
        self.paused = false;
    }

    pub fn control_music(&mut self, action: &str) -> Option<Result<&'static str, &'static str>> {
        let music = self.music.as_mut()?;
        Some(match action {
            "pause" => {
                self.paused = true;
                Ok("paused generated music")
            }
            "stop" => {
                music.clear();
                self.paused = true;
                Ok("stopped generated music")
            }
            "play" if !music.is_empty() => {
                self.paused = false;
                Ok("resumed generated music")
            }
            "play" => {
                Err("The generated song has ended or was stopped; request a new song to play.")
            }
            _ => Err("Generated music supports play, pause and stop, not playlist skipping."),
        })
    }

    pub fn pop_front(&mut self) -> Option<f32> {
        if let Some(sample) = self.speech.pop_front() {
            self.speech_tail = self.tail_samples;
            return Some(sample);
        }
        if self.speech_tail > 0 {
            self.speech_tail -= 1;
            return Some(0.0);
        }
        if self.paused {
            None
        } else {
            self.music.as_mut().and_then(VecDeque::pop_front)
        }
    }

    /// Barge-in discards assistant speech, not the independently controlled song.
    pub fn clear(&mut self) {
        self.speech.clear();
        self.speech_tail = 0;
    }

    pub fn len(&self) -> usize {
        self.speech.len()
            + self.speech_tail
            + if self.paused {
                0
            } else {
                self.music.as_ref().map_or(0, VecDeque::len)
            }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Shared queue plus the stream that drains it.
pub struct Playback {
    queue: Arc<Mutex<OutputQueue>>,
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
        let queue = Arc::new(Mutex::new(OutputQueue::new(device_rate)));
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

    pub fn push_music(&mut self, bytes: &[u8]) {
        let samples: Vec<f32> = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
        let samples = resample_linear(
            &samples,
            OUTPUT_SAMPLE_RATE,
            self.device_rate,
            &mut 0,
            &mut 0.0,
        );
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .replace_music(samples);
    }

    pub fn control_music(&self, action: &str) -> Option<Result<&'static str, &'static str>> {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .control_music(action)
    }

    /// Drop queued assistant speech (barge-in).
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
    queue: &Arc<Mutex<OutputQueue>>,
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
    fn speech_preempts_music_and_music_resumes_after_speech_tail() {
        let mut q = OutputQueue::new(8);
        q.replace_music(vec![0.1, 0.2]);
        assert_eq!(q.pop_front(), Some(0.1));
        q.extend([0.8, 0.9]);
        assert_eq!(q.pop_front(), Some(0.8));
        assert_eq!(q.pop_front(), Some(0.9));
        assert_eq!(q.pop_front(), Some(0.0));
        assert_eq!(q.pop_front(), Some(0.0));
        assert_eq!(q.pop_front(), Some(0.2));
        assert!(q.is_empty());
    }

    #[test]
    fn music_pause_resume_stop_do_not_discard_speech() {
        let mut q = OutputQueue::new(1);
        assert!(q.control_music("pause").is_none());
        q.replace_music(vec![0.1, 0.2]);
        assert!(q.control_music("pause").unwrap().is_ok());
        assert_eq!(q.pop_front(), None);
        assert!(q.is_empty());
        q.extend([0.8]);
        assert_eq!(q.pop_front(), Some(0.8));
        assert!(q.control_music("play").unwrap().is_ok());
        assert_eq!(q.pop_front(), Some(0.1));
        q.extend([0.9]);
        assert!(q.control_music("stop").unwrap().is_ok());
        assert_eq!(q.pop_front(), Some(0.9));
        assert_eq!(q.pop_front(), None);
        assert!(q.control_music("play").unwrap().is_err());
        assert!(q.control_music("next").unwrap().is_err());
    }

    #[test]
    fn barge_in_clears_speech_without_losing_music_control() {
        let mut q = OutputQueue::new(8);
        q.replace_music(vec![0.1]);
        q.extend([0.9]);
        q.clear();
        assert!(q.control_music("pause").unwrap().is_ok());
        assert_eq!(q.pop_front(), None);
        q.replace_music(vec![0.2]);
        assert_eq!(q.pop_front(), Some(0.2));
    }

    #[test]
    fn output_callback_plays_stop_confirmation_without_waiting_for_song() {
        let queue = Arc::new(Mutex::new(OutputQueue::new(24000)));
        queue.lock().unwrap().replace_music(vec![0.1; 24000 * 180]);
        let mut output = [0.0; 4];
        fill(&queue, &mut output, 2, |s| s);
        assert_eq!(output, [0.1; 4]);
        {
            let mut q = queue.lock().unwrap();
            q.control_music("stop").unwrap().unwrap();
            q.extend([0.8, 0.9]);
        }
        fill(&queue, &mut output, 2, |s| s);
        assert_eq!(output, [0.8, 0.8, 0.9, 0.9]);
    }

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
