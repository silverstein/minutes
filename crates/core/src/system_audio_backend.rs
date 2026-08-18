use crate::config::Config;
use crate::diarize::{DiagnosticConfidence, FailureKind, ObservedSignal};
use crate::error::CaptureError;

#[cfg(feature = "streaming")]
use crate::streaming::{AudioChunk, AudioStream, SourceRole};

/// Receives system-audio chunks from a backend.
#[cfg(feature = "streaming")]
pub type AudioSink = crossbeam_channel::Sender<AudioChunk>;

/// Placeholder sink when streaming support is not compiled in.
#[cfg(not(feature = "streaming"))]
#[derive(Debug, Clone)]
pub struct AudioSink;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDescription {
    pub capture_backend: String,
    pub device_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    Granted,
    Denied,
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProbeResult {
    pub observed_signal: ObservedSignal,
    pub failure_kind: Option<FailureKind>,
    pub diagnostic_confidence: DiagnosticConfidence,
}

pub const CPAL_CAPTURE_BACKEND: &str = "cpal";
pub const CORE_AUDIO_TAP_CAPTURE_BACKEND: &str = "core-audio-tap";
pub const CORE_AUDIO_TAP_ROUTE_NAME: &str = "Core Audio Process Tap";
pub const CORE_AUDIO_TAP_MIN_MACOS: MacOsVersion = MacOsVersion {
    major: 14,
    minor: 4,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureBackendKind {
    Cpal,
    CoreAudioTap,
}

impl CaptureBackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpal => CPAL_CAPTURE_BACKEND,
            Self::CoreAudioTap => CORE_AUDIO_TAP_CAPTURE_BACKEND,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct MacOsVersion {
    pub major: u32,
    pub minor: u32,
}

pub fn parse_macos_version(version: &str) -> Option<MacOsVersion> {
    let mut parts = version.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    Some(MacOsVersion { major, minor })
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn current_macos_version() -> Option<MacOsVersion> {
    let output = crate::engine_process::command("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_macos_version(&String::from_utf8_lossy(&output.stdout))
}

pub fn configured_capture_backend(config: &Config) -> Result<CaptureBackendKind, String> {
    parse_capture_backend(&config.recording.capture_backend)
}

pub fn parse_capture_backend(value: &str) -> Result<CaptureBackendKind, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "cpal" => Ok(CaptureBackendKind::Cpal),
        "core-audio-tap" | "core_audio_tap" | "coreaudio-tap" | "process-tap" => {
            Ok(CaptureBackendKind::CoreAudioTap)
        }
        other => Err(format!(
            "unknown recording.capture_backend '{other}'; expected 'cpal' or 'core-audio-tap'"
        )),
    }
}

pub fn core_audio_tap_source_is_supported(source: &str) -> bool {
    let normalized = source.trim().to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "auto" | "" | CORE_AUDIO_TAP_CAPTURE_BACKEND | "core_audio_tap" | "coreaudio-tap"
    )
}

pub fn system_audio_backend_for_config(
    config: &Config,
    route_hint: String,
) -> Result<Box<dyn SystemAudioBackend>, CaptureError> {
    match configured_capture_backend(config).map_err(capture_config_error)? {
        CaptureBackendKind::Cpal => Ok(Box::new(CpalSystemAudioBackend::new(route_hint))),
        CaptureBackendKind::CoreAudioTap => {
            if !core_audio_tap_source_is_supported(&route_hint) {
                return Err(capture_config_error(format!(
                    "recording.capture_backend = '{}' captures the default system output; set [recording.sources] call = \"auto\" instead of '{route_hint}'",
                    CORE_AUDIO_TAP_CAPTURE_BACKEND
                )));
            }

            #[cfg(target_os = "macos")]
            {
                Ok(Box::new(CoreAudioTapSystemAudioBackend::new()))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(capture_config_error(
                    "recording.capture_backend = 'core-audio-tap' is only available on macOS",
                ))
            }
        }
    }
}

fn capture_config_error(message: impl Into<String>) -> CaptureError {
    CaptureError::Io(std::io::Error::other(message.into()))
}

pub trait SystemAudioBackend {
    fn probe(&self, secs: u32) -> Result<ProbeResult, CaptureError>;
    fn start(&mut self, sink: AudioSink) -> Result<StreamHandle, CaptureError>;
    fn current_route(&self) -> RouteDescription;
    fn permission_status(&self) -> Option<PermissionStatus>;
}

trait SystemAudioStreamHandle: Send {
    fn has_error(&self) -> bool;
    fn route(&self) -> RouteDescription;
}

pub struct StreamHandle {
    inner: Box<dyn SystemAudioStreamHandle>,
}

impl StreamHandle {
    fn new(inner: impl SystemAudioStreamHandle + 'static) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    pub fn has_error(&self) -> bool {
        self.inner.has_error()
    }

    pub fn route(&self) -> RouteDescription {
        self.inner.route()
    }
}

#[derive(Debug, Clone)]
pub struct CpalSystemAudioBackend {
    device_override: String,
    current_route: RouteDescription,
}

impl CpalSystemAudioBackend {
    pub fn new(device_override: String) -> Self {
        Self {
            current_route: RouteDescription {
                capture_backend: "cpal".into(),
                device_name: Some(device_override.clone()),
            },
            device_override,
        }
    }
}

impl SystemAudioBackend for CpalSystemAudioBackend {
    fn probe(&self, secs: u32) -> Result<ProbeResult, CaptureError> {
        cpal_probe(&self.device_override, secs)
    }

    fn start(&mut self, sink: AudioSink) -> Result<StreamHandle, CaptureError> {
        let handle = cpal_start_stream(&self.device_override, sink)?;
        self.current_route = handle.route();
        Ok(StreamHandle::new(handle))
    }

    fn current_route(&self) -> RouteDescription {
        self.current_route.clone()
    }

    fn permission_status(&self) -> Option<PermissionStatus> {
        None
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct CoreAudioTapSystemAudioBackend {
    current_route: RouteDescription,
}

#[cfg(target_os = "macos")]
impl CoreAudioTapSystemAudioBackend {
    pub fn new() -> Self {
        Self {
            current_route: RouteDescription {
                capture_backend: CORE_AUDIO_TAP_CAPTURE_BACKEND.into(),
                device_name: Some(CORE_AUDIO_TAP_ROUTE_NAME.into()),
            },
        }
    }
}

#[cfg(target_os = "macos")]
impl Default for CoreAudioTapSystemAudioBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "macos")]
impl SystemAudioBackend for CoreAudioTapSystemAudioBackend {
    fn probe(&self, secs: u32) -> Result<ProbeResult, CaptureError> {
        core_audio_tap_probe(secs)
    }

    fn start(&mut self, sink: AudioSink) -> Result<StreamHandle, CaptureError> {
        let handle = core_audio_tap_start_stream(sink)?;
        self.current_route = handle.route();
        Ok(StreamHandle::new(handle))
    }

    fn current_route(&self) -> RouteDescription {
        self.current_route.clone()
    }

    fn permission_status(&self) -> Option<PermissionStatus> {
        Some(PermissionStatus::Unknown)
    }
}

#[cfg(feature = "streaming")]
struct CpalSystemAudioStreamHandle {
    stream: AudioStream,
    stop_forwarding: std::sync::Arc<std::sync::atomic::AtomicBool>,
    forward_thread: Option<std::thread::JoinHandle<()>>,
    route: RouteDescription,
}

#[cfg(feature = "streaming")]
impl SystemAudioStreamHandle for CpalSystemAudioStreamHandle {
    fn has_error(&self) -> bool {
        self.stream.has_error()
    }

    fn route(&self) -> RouteDescription {
        self.route.clone()
    }
}

#[cfg(feature = "streaming")]
impl Drop for CpalSystemAudioStreamHandle {
    fn drop(&mut self) {
        self.stop_forwarding
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.stream.stop();
        if let Some(handle) = self.forward_thread.take() {
            handle.join().ok();
        }
    }
}

#[cfg(feature = "streaming")]
fn cpal_start_stream(
    device_override: &str,
    sink: AudioSink,
) -> Result<CpalSystemAudioStreamHandle, CaptureError> {
    let stream = AudioStream::start(Some(device_override))?;
    let receiver = stream.receiver.clone();
    let stop_forwarding = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_thread = std::sync::Arc::clone(&stop_forwarding);
    let forward_thread = std::thread::Builder::new()
        .name("system-audio-backend-cpal".into())
        .spawn(move || {
            while !stop_thread.load(std::sync::atomic::Ordering::Relaxed) {
                match receiver.recv_timeout(std::time::Duration::from_millis(50)) {
                    Ok(mut chunk) => {
                        chunk.source = SourceRole::Call;
                        let _ = sink.send_timeout(chunk, std::time::Duration::from_millis(50));
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| CaptureError::Io(std::io::Error::other(e.to_string())))?;

    let route = RouteDescription {
        capture_backend: "cpal".into(),
        device_name: Some(stream.device_name.clone()),
    };

    Ok(CpalSystemAudioStreamHandle {
        stream,
        stop_forwarding,
        forward_thread: Some(forward_thread),
        route,
    })
}

#[cfg(not(feature = "streaming"))]
struct UnsupportedSystemAudioStreamHandle {
    route: RouteDescription,
}

#[cfg(not(feature = "streaming"))]
impl SystemAudioStreamHandle for UnsupportedSystemAudioStreamHandle {
    fn has_error(&self) -> bool {
        true
    }

    fn route(&self) -> RouteDescription {
        self.route.clone()
    }
}

#[cfg(not(feature = "streaming"))]
fn cpal_start_stream(
    device_override: &str,
    _sink: AudioSink,
) -> Result<UnsupportedSystemAudioStreamHandle, CaptureError> {
    Err(CaptureError::Io(std::io::Error::other(format!(
        "system audio backend requires the streaming feature for '{}'",
        device_override
    ))))
}

#[cfg(feature = "streaming")]
fn cpal_probe(device_override: &str, secs: u32) -> Result<ProbeResult, CaptureError> {
    let stream = AudioStream::start(Some(device_override))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs as u64);
    let mut frames_captured = 0usize;
    let mut sum_square = 0.0f64;
    let mut max_rms = 0.0f32;

    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let timeout = remaining.min(std::time::Duration::from_millis(100));
        match stream.receiver.recv_timeout(timeout) {
            Ok(chunk) => {
                frames_captured += chunk.samples.len();
                max_rms = max_rms.max(chunk.rms);
                sum_square += chunk
                    .samples
                    .iter()
                    .map(|sample| (*sample as f64) * (*sample as f64))
                    .sum::<f64>();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    drop(stream);

    let avg_rms = if frames_captured > 0 {
        (sum_square / frames_captured as f64).sqrt() as f32
    } else {
        0.0
    };
    let observed_signal = ObservedSignal {
        frames_captured,
        max_rms,
        avg_rms,
    };
    let failure_kind = if frames_captured == 0 {
        Some(FailureKind::SourceStarved)
    } else if max_rms <= 0.001 {
        Some(FailureKind::Silent)
    } else {
        None
    };

    Ok(ProbeResult {
        observed_signal,
        failure_kind,
        diagnostic_confidence: DiagnosticConfidence::High,
    })
}

#[cfg(not(feature = "streaming"))]
fn cpal_probe(_device_override: &str, _secs: u32) -> Result<ProbeResult, CaptureError> {
    Ok(ProbeResult {
        observed_signal: ObservedSignal {
            frames_captured: 0,
            max_rms: 0.0,
            avg_rms: 0.0,
        },
        failure_kind: Some(FailureKind::BackendUnavailable),
        diagnostic_confidence: DiagnosticConfidence::Inferred,
    })
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
struct CoreAudioTapSystemAudioStreamHandle {
    _started_device: cidre::core_audio::hardware::StartedDevice<cidre::core_audio::AggregateDevice>,
    _tap: cidre::core_audio::TapGuard,
    _ctx: Box<CoreAudioTapCallbackContext>,
    err_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    route: RouteDescription,
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
impl SystemAudioStreamHandle for CoreAudioTapSystemAudioStreamHandle {
    fn has_error(&self) -> bool {
        self.err_flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn route(&self) -> RouteDescription {
        self.route.clone()
    }
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
struct CoreAudioTapCallbackContext {
    sink: AudioSink,
    err_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ratio: f64,
    channels: usize,
    interleaved: bool,
    resample_pos: f64,
    input_samples: Vec<f32>,
    /// Resampled samples not yet emitted, held until a whole chunk is ready.
    ///
    /// Consumers treat one AudioChunk as one 100 ms slot regardless of how
    /// many samples it carries, so emitting a short chunk does not shorten the
    /// slot, it zero-fills the rest of it and advances time by a full slot
    /// anyway. A Core Audio callback delivering 512 frames at 48 kHz yields
    /// about 171 samples, which is a slot that is 89% silence and 9.4x longer
    /// than the audio in it (issue #792, bugs 2 and 3).
    pending_output: Vec<f32>,
    chunk_index: u64,
}

/// Samples in one emitted chunk: 100 ms at 16 kHz.
///
/// This matches the microphone path in `streaming.rs`, which is the contract
/// the dual-source slot writer is built around.
#[cfg(feature = "streaming")]
const TAP_CHUNK_SAMPLES: usize = 1600;

/// Take whole `size`-sample chunks off the front of `pending`, leaving any
/// partial tail behind for the next callback to complete.
///
/// Returning the tail rather than padding it is the whole point. A consumer
/// treats one chunk as one slot however many samples it holds, so a short
/// chunk is not a short slot, it is a full slot mostly filled with silence.
#[cfg(feature = "streaming")]
fn drain_whole_chunks(pending: &mut Vec<f32>, size: usize) -> Vec<Vec<f32>> {
    if size == 0 {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    while pending.len() >= size {
        chunks.push(pending.drain(..size).collect());
    }
    chunks
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
impl CoreAudioTapCallbackContext {
    fn new(
        sink: AudioSink,
        err_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
        asbd: cidre::cat::AudioStreamBasicDesc,
    ) -> Self {
        Self {
            sink,
            err_flag,
            ratio: asbd.sample_rate / 16_000.0,
            channels: asbd.channels_per_frame.max(1) as usize,
            interleaved: asbd.is_interleaved(),
            resample_pos: 0.0,
            input_samples: Vec::new(),
            pending_output: Vec::new(),
            chunk_index: 0,
        }
    }

    fn ingest(&mut self, input_data: &cidre::cat::AudioBufList<1>) {
        let Some(mono) = mono_f32_samples(input_data, self.channels, self.interleaved) else {
            self.err_flag
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return;
        };
        self.input_samples.extend(mono);

        let mut resampled = Vec::new();
        while self.resample_pos < self.input_samples.len() as f64 {
            let idx = self.resample_pos as usize;
            if let Some(sample) = self.input_samples.get(idx) {
                resampled.push(*sample);
            }
            self.resample_pos += self.ratio;
        }

        let consumed = (self.resample_pos as usize).min(self.input_samples.len());
        if consumed > 0 {
            self.input_samples.drain(..consumed);
            self.resample_pos -= consumed as f64;
        }

        if resampled.is_empty() {
            return;
        }

        // Emit whole chunks only. A partial tail stays here until the next
        // callback fills it, so every chunk downstream is a full slot of real
        // audio rather than a short one padded out with silence.
        self.pending_output.extend(resampled);
        for samples in drain_whole_chunks(&mut self.pending_output, TAP_CHUNK_SAMPLES) {
            let rms = (samples
                .iter()
                .map(|sample| (*sample as f64) * (*sample as f64))
                .sum::<f64>()
                / samples.len() as f64)
                .sqrt() as f32;
            let index = self.chunk_index;
            self.chunk_index += 1;
            let _ = self.sink.try_send(AudioChunk {
                samples,
                rms,
                timestamp: std::time::Instant::now(),
                index,
                source: SourceRole::Call,
            });
        }
    }
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn mono_f32_samples(
    input_data: &cidre::cat::AudioBufList<1>,
    channels: usize,
    interleaved: bool,
) -> Option<Vec<f32>> {
    let buffer_count = input_data.number_buffers as usize;
    if buffer_count == 0 {
        return Some(Vec::new());
    }
    let buffers = unsafe { std::slice::from_raw_parts(input_data.buffers.as_ptr(), buffer_count) };

    if interleaved {
        let buffer = buffers.first()?;
        if buffer.data.is_null() || buffer.data_bytes_size == 0 {
            return Some(Vec::new());
        }
        let channels = (buffer.number_channels as usize).max(channels).max(1);
        let sample_count = buffer.data_bytes_size as usize / std::mem::size_of::<f32>();
        let samples =
            unsafe { std::slice::from_raw_parts(buffer.data as *const f32, sample_count) };
        let mut mono = Vec::with_capacity(sample_count / channels);
        for frame in samples.chunks(channels) {
            mono.push(frame.iter().copied().sum::<f32>() / frame.len() as f32);
        }
        return Some(mono);
    }

    let frame_count = buffers
        .iter()
        .filter(|buffer| !buffer.data.is_null())
        .map(|buffer| buffer.data_bytes_size as usize / std::mem::size_of::<f32>())
        .min()
        .unwrap_or(0);
    let mut mono = Vec::with_capacity(frame_count);
    for frame in 0..frame_count {
        let mut sum = 0.0f32;
        let mut count = 0usize;
        for buffer in buffers {
            if buffer.data.is_null() {
                continue;
            }
            let samples = unsafe {
                std::slice::from_raw_parts(
                    buffer.data as *const f32,
                    buffer.data_bytes_size as usize / std::mem::size_of::<f32>(),
                )
            };
            if let Some(sample) = samples.get(frame) {
                sum += *sample;
                count += 1;
            }
        }
        if count > 0 {
            mono.push(sum / count as f32);
        }
    }
    Some(mono)
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
extern "C" fn core_audio_tap_io_proc(
    _device: cidre::core_audio::Device,
    _now: &cidre::cat::AudioTimeStamp,
    input_data: &cidre::cat::AudioBufList<1>,
    _input_time: &cidre::cat::AudioTimeStamp,
    _output_data: &mut cidre::cat::AudioBufList<1>,
    _output_time: &cidre::cat::AudioTimeStamp,
    ctx: Option<&mut CoreAudioTapCallbackContext>,
) -> cidre::os::Status {
    if let Some(ctx) = ctx {
        ctx.ingest(input_data);
    }
    Default::default()
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn core_audio_tap_start_stream(
    sink: AudioSink,
) -> Result<CoreAudioTapSystemAudioStreamHandle, CaptureError> {
    use cidre::{cf, core_audio as ca, ns};

    ensure_core_audio_tap_runtime()?;

    let output_device = default_core_audio_output_device()?;
    let output_uid = output_device
        .uid()
        .map_err(|error| core_audio_error("default output device UID", error))?;
    let output_name = output_device
        .name()
        .ok()
        .map(|name| name.to_string())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "default output".into());

    let sub_device = cf::DictionaryOf::with_keys_values(
        &[ca::sub_device_keys::uid()],
        &[output_uid.as_type_ref()],
    );

    let mut tap_desc = ca::TapDesc::with_stereo_global_tap_excluding_processes(&ns::Array::new());
    tap_desc.set_name(Some(ns::str!(c"Minutes System Audio")));
    tap_desc.set_private(true);
    tap_desc.set_mute_behavior(ca::TapMuteBehavior::Unmuted);
    let tap = tap_desc
        .create_process_tap()
        .map_err(|error| core_audio_error("create process tap", error))?;
    let asbd = tap
        .asbd()
        .map_err(|error| core_audio_error("read process tap format", error))?;
    if !tap_asbd_is_supported(&asbd) {
        return Err(capture_config_error(format!(
            "Core Audio Process Tap returned unsupported format: {asbd:?}"
        )));
    }

    let sub_tap = cf::DictionaryOf::with_keys_values(
        &[ca::sub_device_keys::uid()],
        &[tap
            .uid()
            .map_err(|error| core_audio_error("process tap UID", error))?
            .as_type_ref()],
    );
    let aggregate_desc = cf::DictionaryOf::with_keys_values(
        &[
            ca::aggregate_device_keys::is_private(),
            ca::aggregate_device_keys::is_stacked(),
            ca::aggregate_device_keys::tap_auto_start(),
            ca::aggregate_device_keys::name(),
            ca::aggregate_device_keys::main_sub_device(),
            ca::aggregate_device_keys::uid(),
            ca::aggregate_device_keys::sub_device_list(),
            ca::aggregate_device_keys::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::str!(c"Minutes Process Tap"),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );
    let aggregate_device = ca::AggregateDevice::with_desc(&aggregate_desc)
        .map_err(|error| core_audio_error("create private aggregate tap device", error))?;

    let err_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut ctx = Box::new(CoreAudioTapCallbackContext::new(
        sink,
        std::sync::Arc::clone(&err_flag),
        asbd,
    ));
    let proc_id = aggregate_device
        .create_io_proc_id(core_audio_tap_io_proc, Some(ctx.as_mut()))
        .map_err(|error| core_audio_error("create process tap IOProc", error))?;
    let started_device = ca::device_start(aggregate_device, Some(proc_id))
        .map_err(|error| core_audio_error("start process tap device", error))?;

    Ok(CoreAudioTapSystemAudioStreamHandle {
        _started_device: started_device,
        _tap: tap,
        _ctx: ctx,
        err_flag,
        route: RouteDescription {
            capture_backend: CORE_AUDIO_TAP_CAPTURE_BACKEND.into(),
            device_name: Some(format!("{CORE_AUDIO_TAP_ROUTE_NAME} ({output_name})")),
        },
    })
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn core_audio_tap_probe(secs: u32) -> Result<ProbeResult, CaptureError> {
    let (tx, rx) = crossbeam_channel::bounded(64);
    let handle = core_audio_tap_start_stream(tx)?;
    let route = handle.route();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs as u64);
    let mut frames_captured = 0usize;
    let mut sum_square = 0.0f64;
    let mut max_rms = 0.0f32;

    while std::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        let timeout = remaining.min(std::time::Duration::from_millis(100));
        match rx.recv_timeout(timeout) {
            Ok(chunk) => {
                frames_captured += chunk.samples.len();
                max_rms = max_rms.max(chunk.rms);
                sum_square += chunk
                    .samples
                    .iter()
                    .map(|sample| (*sample as f64) * (*sample as f64))
                    .sum::<f64>();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(handle);

    let avg_rms = if frames_captured > 0 {
        (sum_square / frames_captured as f64).sqrt() as f32
    } else {
        0.0
    };
    let observed_signal = ObservedSignal {
        frames_captured,
        max_rms,
        avg_rms,
    };
    let failure_kind = if frames_captured == 0 {
        Some(FailureKind::SourceStarved)
    } else if max_rms <= 0.001 {
        Some(FailureKind::Silent)
    } else {
        None
    };
    tracing::debug!(route = ?route, ?observed_signal, "Core Audio Process Tap probe completed");

    Ok(ProbeResult {
        observed_signal,
        failure_kind,
        diagnostic_confidence: DiagnosticConfidence::High,
    })
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn tap_asbd_is_supported(asbd: &cidre::cat::AudioStreamBasicDesc) -> bool {
    asbd.format == cidre::cat::AudioFormat::LINEAR_PCM
        && asbd.frames_per_packet == 1
        && asbd
            .format_flags
            .contains(cidre::cat::AudioFormatFlags::IS_FLOAT)
        && asbd.bits_per_channel == 32
        && asbd.sample_rate > 0.0
        && asbd.channels_per_frame > 0
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn ensure_core_audio_tap_runtime() -> Result<(), CaptureError> {
    let Some(version) = current_macos_version() else {
        return Err(capture_config_error(
            "could not determine macOS version for Core Audio Process Tap",
        ));
    };
    if version < CORE_AUDIO_TAP_MIN_MACOS {
        return Err(capture_config_error(format!(
            "Core Audio Process Tap requires macOS {}.{} or newer; this Mac reports {}.{}",
            CORE_AUDIO_TAP_MIN_MACOS.major,
            CORE_AUDIO_TAP_MIN_MACOS.minor,
            version.major,
            version.minor
        )));
    }
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn default_core_audio_output_device() -> Result<cidre::core_audio::Device, CaptureError> {
    use cidre::core_audio as ca;

    if let Ok(device) = ca::System::default_output_device() {
        if !device.is_unknown() {
            return Ok(device);
        }
    }
    if let Ok(device) = ca::System::default_sys_output_device() {
        if !device.is_unknown() {
            return Ok(device);
        }
    }

    Err(capture_config_error(
        "Core Audio Process Tap requires a default macOS output device, but none is available",
    ))
}

#[cfg(all(target_os = "macos", feature = "streaming"))]
fn core_audio_error(context: &str, error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Io(std::io::Error::other(format!("{context}: {error}")))
}

#[cfg(all(target_os = "macos", not(feature = "streaming")))]
fn core_audio_tap_probe(_secs: u32) -> Result<ProbeResult, CaptureError> {
    Ok(ProbeResult {
        observed_signal: ObservedSignal {
            frames_captured: 0,
            max_rms: 0.0,
            avg_rms: 0.0,
        },
        failure_kind: Some(FailureKind::BackendUnavailable),
        diagnostic_confidence: DiagnosticConfidence::Inferred,
    })
}

#[cfg(all(target_os = "macos", not(feature = "streaming")))]
fn core_audio_tap_start_stream(
    _sink: AudioSink,
) -> Result<UnsupportedSystemAudioStreamHandle, CaptureError> {
    Err(capture_config_error(
        "Core Audio Process Tap backend requires the streaming feature",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpal_backend_reports_configured_route_without_permission_api() {
        let backend = CpalSystemAudioBackend::new("BlackHole 2ch".into());

        assert_eq!(
            backend.current_route(),
            RouteDescription {
                capture_backend: "cpal".into(),
                device_name: Some("BlackHole 2ch".into()),
            }
        );
        assert_eq!(backend.permission_status(), None);
    }

    #[test]
    fn capture_backend_parser_accepts_default_and_core_audio_tap_aliases() {
        assert_eq!(parse_capture_backend(""), Ok(CaptureBackendKind::Cpal));
        assert_eq!(parse_capture_backend("cpal"), Ok(CaptureBackendKind::Cpal));
        assert_eq!(
            parse_capture_backend("core-audio-tap"),
            Ok(CaptureBackendKind::CoreAudioTap)
        );
        assert_eq!(
            parse_capture_backend("core_audio_tap"),
            Ok(CaptureBackendKind::CoreAudioTap)
        );
        assert!(parse_capture_backend("screencapturekit").is_err());
    }

    #[test]
    fn parses_macos_major_minor_versions() {
        assert_eq!(
            parse_macos_version("14.4.1"),
            Some(MacOsVersion {
                major: 14,
                minor: 4
            })
        );
        assert_eq!(
            parse_macos_version("15"),
            Some(MacOsVersion {
                major: 15,
                minor: 0
            })
        );
        assert_eq!(parse_macos_version("not-a-version"), None);
    }

    #[test]
    fn core_audio_tap_source_requires_auto_style_route() {
        assert!(core_audio_tap_source_is_supported("auto"));
        assert!(core_audio_tap_source_is_supported("core-audio-tap"));
        assert!(!core_audio_tap_source_is_supported("BlackHole 2ch"));
    }

    #[test]
    fn backend_factory_defaults_to_cpal_route() {
        let config = Config::default();
        let backend = system_audio_backend_for_config(&config, "BlackHole 2ch".into()).unwrap();

        assert_eq!(
            backend.current_route(),
            RouteDescription {
                capture_backend: "cpal".into(),
                device_name: Some("BlackHole 2ch".into()),
            }
        );
    }

    #[test]
    fn probe_result_preserves_backend_agnostic_signal_fields() {
        let result = ProbeResult {
            observed_signal: ObservedSignal {
                frames_captured: 1600,
                max_rms: 0.02,
                avg_rms: 0.01,
            },
            failure_kind: None,
            diagnostic_confidence: DiagnosticConfidence::High,
        };

        assert_eq!(result.observed_signal.frames_captured, 1600);
        assert_eq!(result.failure_kind, None);
        assert_eq!(result.diagnostic_confidence, DiagnosticConfidence::High);
    }

    #[cfg(not(feature = "streaming"))]
    #[test]
    fn cpal_probe_reports_unavailable_without_streaming_feature() {
        let backend = CpalSystemAudioBackend::new("BlackHole 2ch".into());
        let result = backend.probe(1).unwrap();

        assert_eq!(result.observed_signal.frames_captured, 0);
        assert_eq!(result.failure_kind, Some(FailureKind::BackendUnavailable));
        assert_eq!(result.diagnostic_confidence, DiagnosticConfidence::Inferred);
    }
}

#[cfg(all(test, feature = "streaming"))]
mod chunking_tests {
    use super::{drain_whole_chunks, TAP_CHUNK_SAMPLES};

    #[test]
    fn short_callbacks_accumulate_instead_of_emitting_short_chunks() {
        // A Core Audio callback of 512 frames at 48 kHz resamples to about 171
        // samples. Emitting that as a chunk gave the dual-source writer a slot
        // that was 89% silence and advanced the file by a full 100 ms anyway,
        // which is issue #792 bugs 2 and 3 in one line of code.
        let mut pending = Vec::new();
        let callback = vec![0.5f32; 171];

        let mut emitted = Vec::new();
        for _ in 0..9 {
            pending.extend_from_slice(&callback);
            emitted.extend(drain_whole_chunks(&mut pending, TAP_CHUNK_SAMPLES));
        }
        assert!(
            emitted.is_empty(),
            "9 short callbacks are 1539 samples, not yet a whole chunk"
        );

        pending.extend_from_slice(&callback);
        emitted.extend(drain_whole_chunks(&mut pending, TAP_CHUNK_SAMPLES));
        assert_eq!(emitted.len(), 1, "the tenth callback completes one chunk");
        assert_eq!(emitted[0].len(), TAP_CHUNK_SAMPLES);
        assert_eq!(
            pending.len(),
            110,
            "the remainder is kept for the next callback, not padded or dropped"
        );
    }

    #[test]
    fn emitted_chunks_carry_the_samples_unchanged_and_in_order() {
        // The failure this guards against writes real samples followed by
        // silence, so a chunk that is the right length is not enough: it has
        // to be the right length *of audio*.
        let mut pending: Vec<f32> = (0..TAP_CHUNK_SAMPLES * 2 + 7)
            .map(|index| index as f32)
            .collect();
        let chunks = drain_whole_chunks(&mut pending, TAP_CHUNK_SAMPLES);

        assert_eq!(chunks.len(), 2);
        assert_eq!(pending.len(), 7);
        for (position, sample) in chunks[0].iter().enumerate() {
            assert_eq!(*sample, position as f32);
        }
        for (position, sample) in chunks[1].iter().enumerate() {
            assert_eq!(*sample, (TAP_CHUNK_SAMPLES + position) as f32);
        }
        // The position-by-position equality above is the real guarantee: every
        // emitted sample is the source sample that belongs at that offset, so
        // nothing was inserted, dropped, or reordered.
    }

    #[test]
    fn a_zero_size_never_loops_forever() {
        let mut pending = vec![1.0f32; 10];
        assert!(drain_whole_chunks(&mut pending, 0).is_empty());
        assert_eq!(pending.len(), 10);
    }
}
