use crate::error::CaptureError;
use cpal::traits::{DeviceTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Metadata about the audio device configuration used to build a resampled stream.
#[allow(dead_code)]
pub struct StreamConfig {
    /// Native sample rate of the device (e.g., 44100, 48000).
    pub native_sample_rate: u32,
    /// Number of channels on the device.
    pub channels: u16,
    /// Decimation ratio (native_rate / 16000.0).
    pub ratio: f64,
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
    let device_name = device.name().unwrap_or_else(|_| "unknown".into());

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

    let stream_config = StreamConfig {
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
            let stop_clone = Arc::clone(stop_flag);
            let err_flag_clone = Arc::clone(err_flag);

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

                        // Decimate to 16kHz
                        let mut resampled = Vec::new();
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
                        tracing::error!("audio stream error: {}", err);
                        err_flag_clone.store(true, Ordering::Relaxed);
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
            let stop_clone = Arc::clone(stop_flag);
            let err_flag_clone = Arc::clone(err_flag);

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

                        // Decimate to 16kHz
                        let mut resampled = Vec::new();
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
                        tracing::error!("audio stream error: {}", err);
                        err_flag_clone.store(true, Ordering::Relaxed);
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
