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

const RESAMPLE_TAPS: usize = 95;
const RESAMPLE_PHASES: usize = 256;

/// Streaming, windowed-sinc polyphase resampling to 16 kHz. Coefficients and
/// history are allocated before opening the device. Every output costs a fixed
/// 95 multiply-adds; callback boundaries never reset filter history or phase.
/// The causal filter delays non-native-rate audio by 47 input samples (under
/// 1.1 ms at 44.1/48 kHz). Native 16 kHz input stays bit-exact and undelayed.
struct BandLimitedResampler {
    ratio: f64,
    coefficients: Vec<f32>,
    history: [f32; RESAMPLE_TAPS],
    head: usize,
    input_index: u64,
    output_index: u64,
}

impl BandLimitedResampler {
    fn new(input_rate: u32) -> Self {
        assert!(input_rate > 0, "resampler requires a nonzero input rate");
        let ratio = input_rate as f64 / 16_000.0;
        let mut coefficients = Vec::new();
        if input_rate != 16_000 {
            coefficients.reserve((RESAMPLE_PHASES + 1) * RESAMPLE_TAPS);
            let cutoff = 0.45 / ratio.max(1.0);
            let center = (RESAMPLE_TAPS - 1) as f64 / 2.0;
            for phase in 0..=RESAMPLE_PHASES {
                let offset = phase as f64 / RESAMPLE_PHASES as f64;
                let start = coefficients.len();
                let mut sum = 0.0;
                for tap in 0..RESAMPLE_TAPS {
                    let position = tap as f64 - center - offset;
                    let angle = 2.0 * std::f64::consts::PI * cutoff * position;
                    let sinc = if angle.abs() < 1e-12 {
                        1.0
                    } else {
                        angle.sin() / angle
                    };
                    let window = 0.54
                        - 0.46
                            * (2.0 * std::f64::consts::PI * tap as f64
                                / (RESAMPLE_TAPS - 1) as f64)
                                .cos();
                    let coefficient = (2.0 * cutoff * sinc * window) as f32;
                    coefficients.push(coefficient);
                    sum += coefficient;
                }
                for coefficient in &mut coefficients[start..] {
                    *coefficient /= sum;
                }
            }
        }
        Self {
            ratio,
            coefficients,
            history: [0.0; RESAMPLE_TAPS],
            head: 0,
            input_index: 0,
            output_index: 0,
        }
    }

    fn push(&mut self, sample: f32, emit: &mut impl FnMut(f32)) {
        let sample = if sample.is_finite() { sample } else { 0.0 };
        if self.coefficients.is_empty() {
            emit(sample);
            return;
        }
        self.history[self.head] = sample;
        self.head = (self.head + 1) % RESAMPLE_TAPS;
        let now = self.input_index as f64;
        loop {
            let position = self.output_index as f64 * self.ratio;
            if position > now {
                break;
            }
            let phase = ((now - position) * RESAMPLE_PHASES as f64).round() as usize;
            let start = phase.min(RESAMPLE_PHASES) * RESAMPLE_TAPS;
            let mut result = 0.0;
            for tap in 0..RESAMPLE_TAPS {
                let index = (self.head + RESAMPLE_TAPS - 1 - tap) % RESAMPLE_TAPS;
                result += self.history[index] * self.coefficients[start + tap];
            }
            emit(result);
            self.output_index += 1;
        }
        self.input_index += 1;
    }

    fn process(
        &mut self,
        samples: impl Iterator<Item = f32>,
        batch: &mut [f32; 1024],
        on_samples: &mut impl FnMut(&[f32]),
    ) {
        let mut count = 0;
        {
            let mut emit = |sample| {
                batch[count] = sample;
                count += 1;
                if count == batch.len() {
                    on_samples(batch);
                    count = 0;
                }
            };
            for sample in samples {
                self.push(sample, &mut emit);
            }
        }
        if count > 0 {
            on_samples(&batch[..count]);
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
    if sample_rate == 0 || channels == 0 {
        return Err(CaptureError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "audio device reported an empty sample rate or channel layout",
        )));
    }
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
            let mut resampler = BandLimitedResampler::new(sample_rate);
            let mut resampled = [0.0_f32; 1024];
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

                        resampler.process(
                            data.chunks(ch)
                                .map(|chunk| chunk.iter().sum::<f32>() / ch as f32),
                            &mut resampled,
                            &mut on_samples,
                        );
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
            let mut resampler = BandLimitedResampler::new(sample_rate);
            let mut resampled = [0.0_f32; 1024];
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

                        resampler.process(
                            data.chunks(ch).map(|chunk| {
                                chunk
                                    .iter()
                                    .map(|&sample| sample as f32 / 32768.0)
                                    .sum::<f32>()
                                    / ch as f32
                            }),
                            &mut resampled,
                            &mut on_samples,
                        );
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
mod resampling_tests {
    use super::*;

    fn resample(rate: u32, input: &[f32], chunk_size: usize) -> Vec<f32> {
        let mut resampler = BandLimitedResampler::new(rate);
        let mut batch = [0.0; 1024];
        let mut output = Vec::new();
        for chunk in input.chunks(chunk_size) {
            resampler.process(chunk.iter().copied(), &mut batch, &mut |samples| {
                assert!(samples.len() <= 1024);
                output.extend_from_slice(samples);
            });
        }
        output
    }

    fn tone(rate: u32, frequency: f64) -> Vec<f32> {
        (0..rate)
            .map(|sample| {
                (2.0 * std::f64::consts::PI * frequency * sample as f64 / rate as f64).sin() as f32
            })
            .collect()
    }

    fn rms(samples: &[f32]) -> f64 {
        (samples
            .iter()
            .map(|&sample| (sample as f64).powi(2))
            .sum::<f64>()
            / samples.len() as f64)
            .sqrt()
    }

    #[test]
    fn native_rate_is_bit_exact_and_output_batches_remain_bounded() {
        let input = tone(16_000, 1234.0);
        assert_eq!(resample(16_000, &input, input.len()), input);
    }

    #[test]
    fn fractional_resampling_preserves_count_and_callback_boundary_independence() {
        for rate in [44_100, 48_000] {
            let input = tone(rate, 1234.0);
            let whole = resample(rate, &input, input.len());
            assert_eq!(whole.len(), 16_000);
            assert_eq!(resample(rate, &input, 137), whole);
            assert_eq!(resample(rate, &input, 1), whole);
        }
    }

    #[test]
    fn downsampling_rejects_out_of_band_tones_instead_of_aliasing_them() {
        for rate in [44_100, 48_000] {
            let output = resample(rate, &tone(rate, 12_000.0), 511);
            // Naive sample selection aliases a full-scale 12 kHz tone into
            // speech at 4 kHz (~0.707 RMS). Require >50 dB attenuation.
            assert!(
                rms(&output[1024..]) < 0.002,
                "{rate}: {}",
                rms(&output[1024..])
            );
        }
    }

    #[test]
    fn speech_passband_level_and_dc_gain_are_preserved() {
        for rate in [44_100, 48_000] {
            for frequency in [100.0, 1000.0, 4000.0] {
                let output = resample(rate, &tone(rate, frequency), 233);
                assert!((rms(&output[1024..]) - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.01);
            }
            let output = resample(rate, &vec![1.0; rate as usize], 127);
            assert!(output[1024..]
                .iter()
                .all(|sample| (sample - 1.0).abs() < 1e-5));
        }
    }
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
