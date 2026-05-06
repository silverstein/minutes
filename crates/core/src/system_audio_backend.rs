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
