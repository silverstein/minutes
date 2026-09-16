//! Echo-cancelled microphone and speaker through one Voice Processing IO unit.
//!
//! A plain input stream hears everything the speakers play. On open mic that
//! means the assistant's own voice trips the provider's speech detection and it
//! interrupts itself. Apple's VoiceProcessingIO unit captures and renders
//! together and subtracts what it rendered from what it captured: the acoustic
//! echo cancellation FaceTime uses, and what a browser gives a web client through
//! `getUserMedia({ echoCancellation: true })`. Both directions run at the
//! provider's 24 kHz mono float; the microphone side is decimated to 16 kHz
//! chunks shaped like [`AudioChunk`], so the session treats it exactly like
//! `AudioStream`.
//!
//! The unit follows the system default input and output devices.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::macos_helpers::{get_default_device_id, get_device_name};
use coreaudio::audio_unit::render_callback::{data, Args};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};
use crossbeam_channel::{bounded, Receiver};
use objc2_audio_toolbox::kAudioOutputUnitProperty_EnableIO;

use super::audio_out::OutputQueue;
use super::decimate::Decimator;
use super::protocol::OUTPUT_SAMPLE_RATE;
use super::VoiceLiveError;
use crate::streaming::{AudioChunk, SourceRole};

/// Microphone chunk rate the provider expects.
const MIC_RATE: u32 = 16_000;
/// 100 ms at 16 kHz, matching `AudioStream`.
const CHUNK_SAMPLES: usize = 1_600;

/// One voice-processing unit: echo-cancelled mic chunks out, speaker samples in.
pub struct VoiceIo {
    unit: AudioUnit,
    queue: Arc<Mutex<OutputQueue>>,
    stop: Arc<AtomicBool>,
    /// Echo-cancelled 16 kHz microphone chunks.
    pub receiver: Receiver<AudioChunk>,
    /// Name of the default input device at start.
    pub input_name: String,
    /// Name of the default output device at start.
    pub output_name: String,
}

// The audio unit holds raw Core Audio handles. The session thread is the sole
// owner after `start` returns, matching the `Playback` precedent.
unsafe impl Send for VoiceIo {}

fn audio_err(what: &str) -> impl Fn(coreaudio::Error) -> VoiceLiveError + '_ {
    move |e| VoiceLiveError::Audio(format!("voice processing {what}: {e}"))
}

fn device_label(input: bool) -> String {
    get_default_device_id(input)
        .and_then(|id| get_device_name(id).ok())
        .unwrap_or_else(|| {
            if input {
                "input".into()
            } else {
                "output".into()
            }
        })
}

impl VoiceIo {
    /// Open the unit on the default devices and start both directions.
    pub fn start() -> Result<Self, VoiceLiveError> {
        let mut unit =
            AudioUnit::new_uninitialized(IOType::VoiceProcessingIO).map_err(audio_err("create"))?;
        let enable = 1u32;
        unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Input,
            Element::Input,
            Some(&enable),
        )
        .map_err(audio_err("enable input"))?;
        unit.set_property(
            kAudioOutputUnitProperty_EnableIO,
            Scope::Output,
            Element::Output,
            Some(&enable),
        )
        .map_err(audio_err("enable output"))?;

        let format = StreamFormat {
            sample_rate: OUTPUT_SAMPLE_RATE as f64,
            sample_format: SampleFormat::F32,
            flags: LinearPcmFlags::IS_FLOAT
                | LinearPcmFlags::IS_PACKED
                | LinearPcmFlags::IS_NON_INTERLEAVED,
            channels: 1,
        };
        // What we render into the speaker side, and what we read from the mic side.
        unit.set_stream_format(format, Scope::Input, Element::Output)
            .map_err(audio_err("speaker format"))?;
        unit.set_stream_format(format, Scope::Output, Element::Input)
            .map_err(audio_err("microphone format"))?;

        let queue = Arc::new(Mutex::new(OutputQueue::new(OUTPUT_SAMPLE_RATE)));
        let render_queue = Arc::clone(&queue);
        let mut scratch: Vec<f32> = Vec::new();
        unit.set_render_callback(move |mut args: Args<data::NonInterleaved<f32>>| {
            scratch.clear();
            scratch.resize(args.num_frames, 0.0);
            // try_lock: never stall the render thread behind the session thread.
            if let Ok(mut q) = render_queue.try_lock() {
                for slot in scratch.iter_mut() {
                    match q.pop_front() {
                        Some(s) => *slot = s,
                        None => break,
                    }
                }
            }
            for channel in args.data.channels_mut() {
                for (out, s) in channel.iter_mut().zip(&scratch) {
                    *out = *s;
                }
            }
            Ok(())
        })
        .map_err(audio_err("render callback"))?;

        let (tx, receiver) = bounded::<AudioChunk>(64);
        let stop = Arc::new(AtomicBool::new(false));
        let stop_in_callback = Arc::clone(&stop);
        let mut decimator = Decimator::new(OUTPUT_SAMPLE_RATE, MIC_RATE);
        let mut pending: Vec<f32> = Vec::with_capacity(CHUNK_SAMPLES * 2);
        let mut index: u64 = 0;
        unit.set_input_callback(move |args: Args<data::NonInterleaved<f32>>| {
            if stop_in_callback.load(Ordering::Relaxed) {
                return Ok(());
            }
            let Some(channel) = args.data.channels().next() else {
                return Ok(());
            };
            decimator.push(channel, &mut pending);
            while pending.len() >= CHUNK_SAMPLES {
                let samples: Vec<f32> = pending.drain(..CHUNK_SAMPLES).collect();
                let rms =
                    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
                let chunk = AudioChunk {
                    samples,
                    rms,
                    timestamp: Instant::now(),
                    index,
                    source: SourceRole::Voice,
                };
                index += 1;
                // A slow consumer drops audio rather than stalling the audio thread.
                let _ = tx.try_send(chunk);
            }
            Ok(())
        })
        .map_err(audio_err("input callback"))?;

        unit.initialize().map_err(audio_err("initialize"))?;
        unit.start().map_err(audio_err("start"))?;

        Ok(Self {
            unit,
            queue,
            stop,
            receiver,
            input_name: device_label(true),
            output_name: device_label(false),
        })
    }

    /// Queue one chunk of little-endian PCM16 at the provider's 24 kHz.
    pub fn push_pcm16(&self, bytes: &[u8]) {
        let mut q = self.queue.lock().unwrap_or_else(|p| p.into_inner());
        q.extend(
            bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0),
        );
    }

    pub fn push_music(&self, bytes: &[u8]) {
        let samples = bytes
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0)
            .collect();
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
    pub fn flush(&self) {
        self.queue.lock().unwrap_or_else(|p| p.into_inner()).clear();
    }

    /// True when nothing is left to play.
    pub fn is_idle(&self) -> bool {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .is_empty()
    }

    /// Stop both directions. Dropping the unit releases it.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.unit.stop();
    }
}
