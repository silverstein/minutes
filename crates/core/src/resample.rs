use crate::error::CaptureError;
use cpal::traits::{DeviceTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Metadata about the audio device configuration used to build a resampled stream.
#[allow(dead_code)]
pub struct StreamConfig {
    /// Overloads are counted on the audio thread and reported by its owner.
    pub diagnostics: Arc<InputStreamDiagnostics>,
    /// Native sample rate of the device (e.g., 44100, 48000).
    pub native_sample_rate: u32,
    /// Number of channels on the device.
    pub channels: u16,
    /// Decimation ratio (native_rate / 16000.0).
    pub ratio: f64,
}

/// Per-stream overload evidence. Reporting belongs on the capture/control thread.
#[derive(Default)]
pub struct InputStreamDiagnostics {
    xruns: AtomicU64,
}

impl InputStreamDiagnostics {
    fn handle_error(&self, error: cpal::Error, fatal: &AtomicBool) {
        if error.kind() == cpal::ErrorKind::Xrun {
            // CoreAudio delivers overloads from its realtime thread. Do not
            // format, log, lock, or request a destructive restart here (#940).
            self.xruns.fetch_add(1, Ordering::Relaxed);
        } else {
            tracing::error!("audio stream error: {}", error);
            fatal.store(true, Ordering::Relaxed);
        }
    }

    pub fn report_pending(&self) {
        self.report_pending_with(|entry| {
            // The desktop has no tracing subscriber. Persist content-free
            // diagnostics here, never from the realtime audio callback.
            if let Err(error) = crate::logging::append_log(&entry) {
                tracing::warn!(%error, "failed to write audio overload diagnostic");
            }
        });
    }

    fn report_pending_with(&self, mut report: impl FnMut(serde_json::Value)) {
        let xruns = self.xruns.swap(0, Ordering::Relaxed);
        if xruns > 0 {
            let message = "audio input overload: possible audio gaps; continuing the existing stream; review the saved recording";
            tracing::warn!(xruns, message);
            report(serde_json::json!({
                "ts": chrono::Local::now().to_rfc3339(),
                "level": "warn",
                "step": "audio_input_overload",
                "xruns": xruns,
                "message": message,
            }));
        }
    }
}

/// Build a cpal input stream that captures audio, mixes to mono, and resamples
/// to 16 kHz. Resampled f32 mono samples are delivered in batches to `on_samples`.
///
/// The callback receives a slice of resampled mono f32 samples (normalized to
/// roughly -1.0..1.0). The caller is responsible for converting, writing, or
/// forwarding the samples as needed.
///
/// Returns the running cpal stream, the device name, and configuration metadata.
pub fn build_resampled_input_stream<F>(
    device: &cpal::Device,
    stop_flag: &Arc<AtomicBool>,
    err_flag: &Arc<AtomicBool>,
    on_samples: F,
) -> Result<(cpal::Stream, String, StreamConfig), CaptureError>
where
    F: FnMut(&[f32]) + Send + 'static,
{
    let device_name = device
        .description()
        .map_or_else(|_| "unknown".to_string(), |d| d.name().to_string());

    let supported_config = device
        .default_input_config()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("input config: {}", e))))?;

    let sample_rate = supported_config.sample_rate();
    let channels = supported_config.channels();
    let ratio = sample_rate as f64 / 16000.0;

    tracing::info!(
        sample_rate,
        channels,
        format = ?supported_config.sample_format(),
        "audio capture config"
    );

    let diagnostics = Arc::new(InputStreamDiagnostics::default());
    let stream_config = StreamConfig {
        diagnostics: Arc::clone(&diagnostics),
        native_sample_rate: sample_rate,
        channels,
        ratio,
    };

    // Use Option::take to move the callback into exactly one match branch.
    // Only one branch runs (F32 or I16), so the callback is consumed once.
    let mut on_samples = Some(on_samples);

    let stream = match supported_config.sample_format() {
        cpal::SampleFormat::F32 => {
            let mut on_samples = on_samples.take().unwrap();
            let ch = channels as usize;
            let mut resample_pos: f64 = 0.0;
            let mut input_samples: Vec<f32> = Vec::new();
            // Hoisted out of the callback to avoid heap alloc/free on every cpal
            // tick (~100×/sec). `clear()` keeps the capacity. The callback below
            // owns this buffer exclusively and consumes `on_samples(&resampled)`
            // synchronously, so no slice escapes the call.
            let mut resampled: Vec<f32> = Vec::new();
            let stop_clone = Arc::clone(stop_flag);
            let err_flag_clone = Arc::clone(err_flag);
            let diagnostics = Arc::clone(&diagnostics);

            device
                .build_input_stream(
                    supported_config.into(),
                    move |data: &[f32], _: &cpal::InputCallbackInfo| {
                        if stop_clone.load(Ordering::Relaxed) {
                            return;
                        }

                        // Mix to mono
                        for chunk in data.chunks(ch) {
                            let mono: f32 = chunk.iter().sum::<f32>() / ch as f32;
                            input_samples.push(mono);
                        }

                        // Decimate to 16kHz — reuse the hoisted buffer.
                        resampled.clear();
                        while resample_pos < input_samples.len() as f64 {
                            let idx = resample_pos as usize;
                            if idx < input_samples.len() {
                                resampled.push(input_samples[idx]);
                            }
                            resample_pos += ratio;
                        }

                        // Keep remainder for next callback
                        let consumed = (resample_pos as usize).min(input_samples.len());
                        if consumed > 0 {
                            input_samples.drain(..consumed);
                            resample_pos -= consumed as f64;
                        }

                        if !resampled.is_empty() {
                            on_samples(&resampled);
                        }
                    },
                    move |err| {
                        diagnostics.handle_error(err, &err_flag_clone);
                    },
                    None,
                )
                .map_err(|e| {
                    CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                })?
        }
        cpal::SampleFormat::I16 => {
            let mut on_samples = on_samples.take().unwrap();
            let ch = channels as usize;
            let mut resample_pos: f64 = 0.0;
            let mut input_samples: Vec<f32> = Vec::new();
            // Hoisted — see F32 branch comment above. Separate buffer per branch
            // (only one branch ever runs for a given stream).
            let mut resampled: Vec<f32> = Vec::new();
            let stop_clone = Arc::clone(stop_flag);
            let err_flag_clone = Arc::clone(err_flag);
            let diagnostics = Arc::clone(&diagnostics);

            device
                .build_input_stream(
                    supported_config.into(),
                    move |data: &[i16], _: &cpal::InputCallbackInfo| {
                        if stop_clone.load(Ordering::Relaxed) {
                            return;
                        }

                        // Mix to mono (convert i16 → f32 first)
                        for chunk in data.chunks(ch) {
                            let mono: f32 =
                                chunk.iter().map(|&s| s as f32 / 32768.0).sum::<f32>() / ch as f32;
                            input_samples.push(mono);
                        }

                        // Decimate to 16kHz — reuse the hoisted buffer.
                        resampled.clear();
                        while resample_pos < input_samples.len() as f64 {
                            let idx = resample_pos as usize;
                            if idx < input_samples.len() {
                                resampled.push(input_samples[idx]);
                            }
                            resample_pos += ratio;
                        }

                        // Keep remainder for next callback
                        let consumed = (resample_pos as usize).min(input_samples.len());
                        if consumed > 0 {
                            input_samples.drain(..consumed);
                            resample_pos -= consumed as f64;
                        }

                        if !resampled.is_empty() {
                            on_samples(&resampled);
                        }
                    },
                    move |err| {
                        diagnostics.handle_error(err, &err_flag_clone);
                    },
                    None,
                )
                .map_err(|e| {
                    CaptureError::Io(std::io::Error::other(format!("build stream: {}", e)))
                })?
        }
        format => {
            return Err(CaptureError::Io(std::io::Error::other(format!(
                "unsupported sample format: {:?}",
                format
            ))));
        }
    };

    stream
        .play()
        .map_err(|e| CaptureError::Io(std::io::Error::other(format!("stream play: {}", e))))?;

    Ok((stream, device_name, stream_config))
}

#[cfg(test)]
mod overload_tests {
    use super::*;

    #[test]
    fn overloads_do_not_request_reconnect_and_remain_observable() {
        let diagnostics = InputStreamDiagnostics::default();
        let fatal = AtomicBool::new(false);
        for _ in 0..3 {
            diagnostics.handle_error(cpal::ErrorKind::Xrun.into(), &fatal);
        }
        assert!(!fatal.load(Ordering::Relaxed));
        assert_eq!(diagnostics.xruns.load(Ordering::Relaxed), 3);
        let mut reports = Vec::new();
        diagnostics.report_pending_with(|entry| reports.push(entry));
        assert_eq!(diagnostics.xruns.load(Ordering::Relaxed), 0);
        diagnostics.report_pending_with(|entry| reports.push(entry));
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["level"], "warn");
        assert_eq!(reports[0]["step"], "audio_input_overload");
        assert_eq!(reports[0]["xruns"], 3);
        assert!(reports[0]["message"]
            .as_str()
            .unwrap()
            .contains("possible audio gaps"));
    }

    #[test]
    fn overload_does_not_clear_a_real_stream_failure() {
        for kind in [
            cpal::ErrorKind::DeviceNotAvailable,
            cpal::ErrorKind::StreamInvalidated,
            cpal::ErrorKind::BackendError,
        ] {
            let diagnostics = InputStreamDiagnostics::default();
            let fatal = AtomicBool::new(false);
            diagnostics.handle_error(kind.into(), &fatal);
            diagnostics.handle_error(cpal::ErrorKind::Xrun.into(), &fatal);
            assert!(fatal.load(Ordering::Relaxed));
            assert_eq!(diagnostics.xruns.load(Ordering::Relaxed), 1);
        }
    }
}
