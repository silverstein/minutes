use crate::diarize::{DiagnosticConfidence, FailureKind, ObservedSignal};
use crate::error::CaptureError;
use crate::streaming::{AudioChunk, ChunkAccumulator, SourceRole};
use crate::system_audio_backend::{
    AudioSink, PermissionStatus, ProbeResult, RouteDescription, StreamHandle, SystemAudioBackend,
    SystemAudioStreamHandle, POCKETSTATION_CAPTURE_BACKEND,
};
use pocketstation::{AudioFrameDuration, ProcessId, Session, SessionEventKind, Source};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const POCKETSTATION_SAMPLE_RATE_HZ: u32 = 48_000;
const MINUTES_SAMPLE_RATE_HZ: u32 = 16_000;
const AUDIO_POLL_TIMEOUT_MS: u64 = 50;
const PROBE_QUEUE_CAPACITY_CHUNKS: usize = 64;

#[derive(Debug, Clone)]
enum ApplicationSelection {
    NameOrId(String),
    Process(ProcessId),
}

impl ApplicationSelection {
    fn parse(value: &str) -> Result<Self, CaptureError> {
        let value = value.trim();
        if value.is_empty() || value.eq_ignore_ascii_case("auto") {
            return Err(capture_error(
                "configure PocketStation capture",
                "choose an application name, application ID, or pid:<process id>",
            ));
        }
        let Some(process_id) = value.strip_prefix("pid:") else {
            return Ok(Self::NameOrId(value.to_owned()));
        };
        let process_id = process_id.trim().parse::<u32>().map_err(|_| {
            capture_error(
                "parse PocketStation application",
                format!("'{value}' is not a valid pid:<process id> selector"),
            )
        })?;
        if process_id == 0 {
            return Err(capture_error(
                "parse PocketStation application",
                "process id must be greater than zero",
            ));
        }
        Ok(Self::Process(ProcessId::new(process_id)))
    }

    fn source(&self) -> Source {
        match self {
            Self::NameOrId(value) => Source::application(value.clone()),
            Self::Process(process_id) => Source::application(*process_id),
        }
    }

    fn route_name(&self) -> String {
        match self {
            Self::NameOrId(value) => format!("PocketStation application: {value}"),
            Self::Process(process_id) => {
                format!("PocketStation application process: {}", process_id.get())
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PocketStationCapture {
    application: ApplicationSelection,
    route: RouteDescription,
}

impl PocketStationCapture {
    pub(crate) fn new(value: String) -> Result<Self, CaptureError> {
        let application = ApplicationSelection::parse(&value)?;
        let route = RouteDescription {
            capture_backend: POCKETSTATION_CAPTURE_BACKEND.into(),
            device_name: Some(application.route_name()),
        };
        Ok(Self { application, route })
    }

    fn start_session(&self) -> Result<pocketstation::RunningSession, CaptureError> {
        let session = Session::builder()
            .audio_frame_duration(AudioFrameDuration::Ms10)
            .build();
        let application = session
            .capture(self.application.source())
            .map_err(|error| capture_error("select PocketStation application", error))?;
        let output = session
            .polled_audio()
            .map_err(|error| capture_error("open PocketStation audio output", error))?;
        application
            .send(output)
            .map_err(|error| capture_error("connect PocketStation application audio", error))?;
        session
            .start()
            .map_err(|error| capture_error("start PocketStation capture", error))
    }
}

impl SystemAudioBackend for PocketStationCapture {
    fn probe(&self, duration_s: u32) -> Result<ProbeResult, CaptureError> {
        let (sender, receiver) = crossbeam_channel::bounded(PROBE_QUEUE_CAPACITY_CHUNKS);
        let stream =
            PocketStationCaptureStream::start(self.start_session()?, sender, self.route.clone())?;
        let deadline = Instant::now() + Duration::from_secs(u64::from(duration_s));
        let mut frames_captured = 0usize;
        let mut sum_square = 0.0f64;
        let mut max_rms = 0.0f32;

        while Instant::now() < deadline {
            let timeout = deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(100));
            match receiver.recv_timeout(timeout) {
                Ok(chunk) => {
                    frames_captured += chunk.samples.len();
                    max_rms = max_rms.max(chunk.rms);
                    sum_square += chunk
                        .samples
                        .iter()
                        .map(|sample| f64::from(*sample) * f64::from(*sample))
                        .sum::<f64>();
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
        }

        let stream_failed = stream.has_error();
        drop(stream);
        let avg_rms = if frames_captured == 0 {
            0.0
        } else {
            (sum_square / frames_captured as f64).sqrt() as f32
        };
        let failure_kind = if stream_failed {
            Some(FailureKind::StreamError)
        } else if frames_captured == 0 {
            Some(FailureKind::SourceStarved)
        } else if max_rms <= 0.001 {
            Some(FailureKind::Silent)
        } else {
            None
        };

        Ok(ProbeResult {
            observed_signal: ObservedSignal {
                frames_captured,
                max_rms,
                avg_rms,
            },
            failure_kind,
            diagnostic_confidence: DiagnosticConfidence::High,
        })
    }

    fn start(&mut self, sink: AudioSink) -> Result<StreamHandle, CaptureError> {
        PocketStationCaptureStream::start(self.start_session()?, sink, self.route.clone())
            .map(StreamHandle::new)
    }

    fn current_route(&self) -> RouteDescription {
        self.route.clone()
    }

    fn permission_status(&self) -> Option<PermissionStatus> {
        None
    }
}

struct PocketStationCaptureStream {
    stop: Arc<AtomicBool>,
    failed: Arc<AtomicBool>,
    dropped_chunks_total: Arc<AtomicU64>,
    worker: Option<JoinHandle<()>>,
    route: RouteDescription,
}

impl PocketStationCaptureStream {
    fn start(
        running: pocketstation::RunningSession,
        sink: AudioSink,
        route: RouteDescription,
    ) -> Result<Self, CaptureError> {
        let stop = Arc::new(AtomicBool::new(false));
        let failed = Arc::new(AtomicBool::new(false));
        let dropped_chunks_total = Arc::new(AtomicU64::new(0));
        let worker = Self::spawn(
            running,
            sink,
            Arc::clone(&stop),
            Arc::clone(&failed),
            Arc::clone(&dropped_chunks_total),
        )?;
        Ok(Self {
            stop,
            failed,
            dropped_chunks_total,
            worker: Some(worker),
            route,
        })
    }

    fn spawn(
        mut running: pocketstation::RunningSession,
        sink: AudioSink,
        stop: Arc<AtomicBool>,
        failed: Arc<AtomicBool>,
        dropped_chunks_total: Arc<AtomicU64>,
    ) -> Result<JoinHandle<()>, CaptureError> {
        std::thread::Builder::new()
            .name("minutes-pocketstation-capture".into())
            .spawn(move || {
                let mut writer = CallAudioChunkWriter::default();
                while !stop.load(Ordering::Relaxed) {
                    match running.wait_audio(Duration::from_millis(AUDIO_POLL_TIMEOUT_MS)) {
                        Ok(Some(batch)) => {
                            for frame_index in 0..batch.len() {
                                let Some(frame) = batch.frame(frame_index) else {
                                    failed.store(true, Ordering::Relaxed);
                                    break;
                                };
                                if let Err(error) = writer.write_frame(
                                    frame.sample_rate_hz(),
                                    frame.channels(),
                                    frame.samples(),
                                    &sink,
                                    &dropped_chunks_total,
                                ) {
                                    tracing::error!(error = %error, "PocketStation capture stopped");
                                    failed.store(true, Ordering::Relaxed);
                                    break;
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => {
                            tracing::error!(error = %error, "PocketStation audio polling failed");
                            failed.store(true, Ordering::Relaxed);
                        }
                    }
                    while let pocketstation::SessionEventReceive::Event(event) =
                        running.try_recv_event()
                    {
                        if Self::event_failed(event.kind()) {
                            tracing::error!(event = ?event, "PocketStation capture failed");
                            failed.store(true, Ordering::Relaxed);
                        }
                    }
                    if failed.load(Ordering::Relaxed) {
                        break;
                    }
                }
                if !running.stop().is_success() {
                    failed.store(true, Ordering::Relaxed);
                }
            })
            .map_err(|error| capture_error("start PocketStation capture worker", error))
    }

    fn event_failed(event: &SessionEventKind) -> bool {
        match event {
            SessionEventKind::Source(_)
            | SessionEventKind::Endpoint(_)
            | SessionEventKind::Rollback(_)
            | SessionEventKind::Finalization(_)
            | SessionEventKind::Lifecycle(pocketstation::SessionLifecycleState::Failed) => true,
            SessionEventKind::Terminal(outcome) => {
                outcome.state() == pocketstation::SessionTerminalState::Failed
            }
            _ => false,
        }
    }
}

impl SystemAudioStreamHandle for PocketStationCaptureStream {
    fn has_error(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    fn route(&self) -> RouteDescription {
        self.route.clone()
    }
}

/// Drop invariant — request Session shutdown before joining its owning thread,
/// report delivery loss, and never panic.
impl Drop for PocketStationCaptureStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if self
            .worker
            .take()
            .is_some_and(|worker| worker.join().is_err())
        {
            self.failed.store(true, Ordering::Relaxed);
            tracing::error!("PocketStation capture worker panicked");
        }
        let dropped_chunks_total = self.dropped_chunks_total.load(Ordering::Relaxed);
        if dropped_chunks_total > 0 {
            tracing::warn!(
                dropped_chunks_total,
                "Minutes dropped PocketStation audio because its call-audio queue was full"
            );
        }
    }
}

#[derive(Default)]
struct CallAudioChunkWriter {
    downsample_phase_samples: usize,
    resampled_samples: Vec<f32>,
    chunks: ChunkAccumulator,
}

impl CallAudioChunkWriter {
    fn write_frame(
        &mut self,
        sample_rate_hz: u32,
        channels: u8,
        samples: &[f32],
        sink: &AudioSink,
        dropped_chunks_total: &AtomicU64,
    ) -> Result<(), CaptureError> {
        if sample_rate_hz != POCKETSTATION_SAMPLE_RATE_HZ {
            return Err(capture_error(
                "read PocketStation audio",
                format!(
                    "expected {POCKETSTATION_SAMPLE_RATE_HZ} Hz audio, received {sample_rate_hz} Hz"
                ),
            ));
        }
        let channel_count = usize::from(channels);
        if channel_count == 0 || !samples.len().is_multiple_of(channel_count) {
            return Err(capture_error(
                "read PocketStation audio",
                "received an invalid channel layout",
            ));
        }

        self.resampled_samples.clear();
        let downsample_ratio = (POCKETSTATION_SAMPLE_RATE_HZ / MINUTES_SAMPLE_RATE_HZ) as usize;
        for frame in samples.chunks_exact(channel_count) {
            let mono = frame.iter().copied().sum::<f32>() / channel_count as f32;
            if self.downsample_phase_samples == 0 {
                self.resampled_samples.push(mono);
            }
            self.downsample_phase_samples = (self.downsample_phase_samples + 1) % downsample_ratio;
        }

        let mut sink_disconnected = false;
        self.chunks.push(&self.resampled_samples, |index, samples| {
            let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
                / samples.len() as f32)
                .sqrt();
            let chunk = AudioChunk {
                samples,
                rms,
                timestamp: Instant::now(),
                index,
                source: SourceRole::Call,
            };
            match sink.try_send(chunk) {
                Ok(()) => {}
                Err(crossbeam_channel::TrySendError::Full(_)) => {
                    dropped_chunks_total.fetch_add(1, Ordering::Relaxed);
                }
                Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                    sink_disconnected = true;
                }
            }
        });
        if sink_disconnected {
            return Err(capture_error(
                "deliver PocketStation audio",
                "Minutes stopped receiving call audio",
            ));
        }
        Ok(())
    }
}

fn capture_error(operation: &str, error: impl std::fmt::Display) -> CaptureError {
    CaptureError::Io(std::io::Error::other(format!("{operation}: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn given_supported_application_values_when_parsed_then_selection_succeeds() {
        assert!(ApplicationSelection::parse("Zoom").is_ok());
        assert!(ApplicationSelection::parse("us.zoom.xos").is_ok());
        assert!(ApplicationSelection::parse("pid:1234").is_ok());
    }

    #[test]
    fn given_automatic_or_invalid_process_values_when_parsed_then_selection_fails() {
        assert!(ApplicationSelection::parse("auto").is_err());
        assert!(ApplicationSelection::parse("pid:0").is_err());
        assert!(ApplicationSelection::parse("pid:not-a-number").is_err());
    }

    #[test]
    fn given_variable_frame_sizes_when_written_then_minutes_chunks_are_complete() {
        let (sender, receiver) = crossbeam_channel::bounded(4);
        let dropped_chunks_total = AtomicU64::new(0);
        let mut writer = CallAudioChunkWriter::default();
        let samples = vec![0.25; POCKETSTATION_SAMPLE_RATE_HZ as usize / 10];

        for frame_size_samples in [127usize, 480, 961, 3232] {
            let mut offset_samples = 0;
            while offset_samples < samples.len() {
                let end_samples = (offset_samples + frame_size_samples).min(samples.len());
                writer
                    .write_frame(
                        POCKETSTATION_SAMPLE_RATE_HZ,
                        1,
                        &samples[offset_samples..end_samples],
                        &sender,
                        &dropped_chunks_total,
                    )
                    .unwrap();
                offset_samples = end_samples;
            }
        }

        let chunks: Vec<_> = receiver.try_iter().collect();
        assert_eq!(chunks.len(), 4);
        for (index, chunk) in chunks.iter().enumerate() {
            assert_eq!(chunk.samples.len(), crate::streaming::CHUNK_SAMPLES);
            assert_eq!(chunk.index, index as u64);
            assert_eq!(chunk.source, SourceRole::Call);
            assert!((chunk.rms - 0.25).abs() < f32::EPSILON);
        }
        assert_eq!(dropped_chunks_total.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn given_unexpected_audio_format_when_written_then_capture_fails() {
        let (sender, _receiver) = crossbeam_channel::bounded(1);
        let dropped_chunks_total = AtomicU64::new(0);
        let mut writer = CallAudioChunkWriter::default();

        assert!(writer
            .write_frame(44_100, 1, &[0.0; 441], &sender, &dropped_chunks_total)
            .is_err());
        assert!(writer
            .write_frame(48_000, 0, &[0.0; 480], &sender, &dropped_chunks_total)
            .is_err());
    }

    #[test]
    fn given_closed_minutes_receiver_when_frame_is_written_then_capture_fails() {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        drop(receiver);
        let dropped_chunks_total = AtomicU64::new(0);
        let mut writer = CallAudioChunkWriter::default();
        let samples = vec![0.25; POCKETSTATION_SAMPLE_RATE_HZ as usize / 10];

        let error = writer
            .write_frame(
                POCKETSTATION_SAMPLE_RATE_HZ,
                1,
                &samples,
                &sender,
                &dropped_chunks_total,
            )
            .unwrap_err();

        assert!(error.to_string().contains("stopped receiving call audio"));
        assert_eq!(dropped_chunks_total.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn given_full_minutes_queue_when_frame_is_written_then_drop_is_counted() {
        let (sender, _receiver) = crossbeam_channel::bounded(1);
        let dropped_chunks_total = AtomicU64::new(0);
        let mut writer = CallAudioChunkWriter::default();
        let samples = vec![0.25; POCKETSTATION_SAMPLE_RATE_HZ as usize / 10];

        writer
            .write_frame(
                POCKETSTATION_SAMPLE_RATE_HZ,
                1,
                &samples,
                &sender,
                &dropped_chunks_total,
            )
            .unwrap();
        writer
            .write_frame(
                POCKETSTATION_SAMPLE_RATE_HZ,
                1,
                &samples,
                &sender,
                &dropped_chunks_total,
            )
            .unwrap();

        assert_eq!(dropped_chunks_total.load(Ordering::Relaxed), 1);
    }
}
