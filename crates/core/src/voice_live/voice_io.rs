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

use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use coreaudio::audio_unit::audio_format::LinearPcmFlags;
use coreaudio::audio_unit::macos_helpers::{get_default_device_id, get_device_name};
use coreaudio::audio_unit::render_callback::{data, Args};
use coreaudio::audio_unit::{AudioUnit, Element, IOType, SampleFormat, Scope, StreamFormat};
use crossbeam_channel::{bounded, Receiver, Sender};
use objc2_audio_toolbox::{
    kAudioOutputUnitProperty_EnableIO, kAudioOutputUnitProperty_SetInputCallback,
    kAudioUnitProperty_ShouldAllocateBuffer, AURenderCallbackStruct, AudioUnitRender,
    AudioUnitRenderActionFlags,
};
use objc2_core_audio_types::{AudioBuffer, AudioBufferList, AudioTimeStamp};

use super::audio_out::OutputQueue;
use super::decimate::Decimator;
use super::protocol::OUTPUT_SAMPLE_RATE;
use super::VoiceLiveError;
use crate::streaming::{AudioChunk, SourceRole};

/// Microphone chunk rate the provider expects.
const MIC_RATE: u32 = 16_000;
/// 100 ms at 16 kHz, matching `AudioStream`.
const CHUNK_SAMPLES: usize = 1_600;
static UNIT_OWNED: AtomicBool = AtomicBool::new(false);

struct UnitLease;

impl UnitLease {
    fn acquire() -> Result<Self, VoiceLiveError> {
        UNIT_OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| {
                VoiceLiveError::Audio(
                    "previous voice-processing unit is still opening or shutting down".into(),
                )
            })
    }
}

impl Drop for UnitLease {
    fn drop(&mut self) {
        UNIT_OWNED.store(false, Ordering::Release);
    }
}

/// One voice-processing unit: echo-cancelled mic chunks out, speaker samples in.
pub struct VoiceIo {
    resources: Option<AudioResources>,
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

// Field order keeps the callback allocation alive until the unit has stopped
// and disposed, including when teardown is still running after its deadline.
struct AudioResources {
    unit: AudioUnit,
    _input: Box<InputCallback>,
    _lease: UnitLease,
}

unsafe impl Send for AudioResources {}

struct InputCallback {
    unit: objc2_audio_toolbox::AudioUnit,
    stop: Arc<AtomicBool>,
    tx: Sender<AudioChunk>,
    decimator: Decimator,
    pending: Vec<f32>,
    index: u64,
}

impl InputCallback {
    fn consume(&mut self, channel: &[f32]) {
        self.decimator.push(channel, &mut self.pending);
        while self.pending.len() >= CHUNK_SAMPLES {
            let samples: Vec<f32> = self.pending.drain(..CHUNK_SAMPLES).collect();
            let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
            let chunk = AudioChunk {
                samples,
                rms,
                timestamp: Instant::now(),
                index: self.index,
                source: SourceRole::Voice,
            };
            self.index += 1;
            let _ = self.tx.try_send(chunk);
        }
    }
}

// The configured client format is fixed mono f32. Ask AudioUnitRender for its
// cycle-owned buffer instead of querying stream properties when frame counts
// change. A property query here deadlocked with VoiceProcessingIO teardown.
unsafe extern "C-unwind" fn input_callback(
    context: NonNull<c_void>,
    flags: NonNull<AudioUnitRenderActionFlags>,
    timestamp: NonNull<AudioTimeStamp>,
    bus: u32,
    frames: u32,
    _data: *mut AudioBufferList,
) -> i32 {
    let input = unsafe { &mut *context.cast::<InputCallback>().as_ptr() };
    if input.stop.load(Ordering::Acquire) || frames == 0 {
        return 0;
    }
    let Some(bytes) = frames.checked_mul(std::mem::size_of::<f32>() as u32) else {
        return -50;
    };
    let mut buffers = AudioBufferList {
        mNumberBuffers: 1,
        mBuffers: [AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: bytes,
            mData: std::ptr::null_mut(),
        }],
    };
    let status = unsafe {
        AudioUnitRender(
            input.unit,
            flags.as_ptr(),
            timestamp,
            bus,
            frames,
            NonNull::from(&mut buffers),
        )
    };
    if status != 0 {
        return status;
    }
    let buffer = &buffers.mBuffers[0];
    if buffers.mNumberBuffers != 1
        || buffer.mNumberChannels != 1
        || buffer.mData.is_null()
        || buffer.mDataByteSize < bytes
        || !(buffer.mData as usize).is_multiple_of(std::mem::align_of::<f32>())
    {
        return -50;
    }
    // The returned storage belongs to this render cycle and is never retained.
    let samples =
        unsafe { std::slice::from_raw_parts(buffer.mData.cast::<f32>(), frames as usize) };
    input.consume(samples);
    0
}

fn teardown_with_deadline(work: impl FnOnce() + Send + 'static, deadline: Duration) -> bool {
    let (tx, rx) = bounded(1);
    std::thread::spawn(move || {
        work();
        let _ = tx.send(());
    });
    rx.recv_timeout(deadline).is_ok()
}

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
    pub(super) fn mark_response(&self) {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .mark_response();
    }
    pub(super) fn take_render_events(&self) -> Vec<(&'static str, std::time::Instant)> {
        self.queue
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take_render_events()
    }
    /// Open the unit on the default devices and start both directions.
    pub fn start() -> Result<Self, VoiceLiveError> {
        let lease = UnitLease::acquire()?;
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
        let stop = Arc::new(AtomicBool::new(false));
        let render_stop = Arc::clone(&stop);
        let render_queue = Arc::clone(&queue);
        let mut scratch: Vec<f32> = Vec::new();
        unit.set_render_callback(move |mut args: Args<data::NonInterleaved<f32>>| {
            scratch.clear();
            scratch.resize(args.num_frames, 0.0);
            // try_lock: never stall the render thread behind the session thread.
            if let Ok(mut q) = render_queue.try_lock() {
                for slot in scratch.iter_mut() {
                    if render_stop.load(Ordering::Acquire) {
                        break;
                    }
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
        unit.set_property(
            kAudioUnitProperty_ShouldAllocateBuffer,
            Scope::Output,
            Element::Input,
            Some(&enable),
        )
        .map_err(audio_err("input buffer allocation"))?;
        let input = Box::new(InputCallback {
            unit: *unit.as_ref(),
            stop: Arc::clone(&stop),
            tx,
            decimator: Decimator::new(OUTPUT_SAMPLE_RATE, MIC_RATE),
            pending: Vec::with_capacity(CHUNK_SAMPLES * 2),
            index: 0,
        });
        let mut resources = AudioResources {
            unit,
            _input: input,
            _lease: lease,
        };
        let callback = AURenderCallbackStruct {
            inputProc: Some(input_callback),
            inputProcRefCon: std::ptr::from_mut(resources._input.as_mut()).cast(),
        };
        resources
            .unit
            .set_property(
                kAudioOutputUnitProperty_SetInputCallback,
                Scope::Global,
                Element::Output,
                Some(&callback),
            )
            .map_err(audio_err("input callback"))?;
        resources
            .unit
            .initialize()
            .map_err(audio_err("initialize"))?;
        resources.unit.start().map_err(audio_err("start"))?;

        Ok(Self {
            resources: Some(resources),
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

    /// Stop capture immediately; bound potentially blocked platform teardown.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(resources) = self.resources.take() {
            if !teardown_with_deadline(move || drop(resources), Duration::from_secs(2)) {
                eprintln!("[audio] CoreAudio shutdown exceeded 2s; callbacks are disabled, native cleanup is still pending");
            }
        }
    }
}

impl Drop for VoiceIo {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn teardown_deadline_does_not_drop_live_callback_ownership() {
        let (release, wait) = bounded::<()>(1);
        let finished = Arc::new(AtomicBool::new(false));
        let observed = Arc::clone(&finished);
        assert!(!teardown_with_deadline(
            move || {
                let _ = wait.recv();
                observed.store(true, Ordering::SeqCst);
            },
            Duration::from_millis(10)
        ));
        assert!(!finished.load(Ordering::SeqCst));
        release.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        while !finished.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(finished.load(Ordering::SeqCst));
    }

    #[test]
    fn successful_teardown_completes_before_deadline() {
        assert!(teardown_with_deadline(|| {}, Duration::from_secs(1)));
    }

    #[test]
    #[ignore = "requires an explicitly authorized native microphone session"]
    fn native_echo_capture_restarts_and_stops() {
        for _ in 0..3 {
            let mut io = VoiceIo::start().unwrap();
            let chunk = io.receiver.recv_timeout(Duration::from_secs(4)).unwrap();
            assert_eq!(chunk.samples.len(), CHUNK_SAMPLES);
            assert!(chunk.samples.iter().all(|sample| sample.is_finite()));
            let start = Instant::now();
            io.stop();
            assert!(start.elapsed() < Duration::from_secs(3));
        }
    }
}
