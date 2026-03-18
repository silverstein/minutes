use crate::config::Config;
use crate::error::TranscribeError;
use std::path::Path;
#[cfg(feature = "whisper")]
use std::path::PathBuf;

// ──────────────────────────────────────────────────────────────
// Transcription pipeline:
//
//   Input audio (.wav, .m4a, .mp3, .ogg)
//        │
//        ├─ .wav ──────────────────────────────────▶ whisper-rs
//        │
//        └─ .m4a/.mp3/.ogg ─▶ symphonia decode ─▶ whisper-rs
//                              (to 16kHz mono PCM)
//
// whisper-rs wraps whisper.cpp, uses Apple Accelerate on M-series.
// Model must be downloaded first via `minutes setup`.
// ──────────────────────────────────────────────────────────────

/// Transcribe an audio file to text.
///
/// With the `whisper` feature (default): uses whisper.cpp via whisper-rs.
/// Without: returns a placeholder transcript (for testing without a model).
///
/// Handles format conversion (m4a/mp3/ogg → PCM) automatically via symphonia.
pub fn transcribe(audio_path: &Path, config: &Config) -> Result<String, TranscribeError> {
    // Step 1: Load audio as 16kHz mono f32 PCM samples
    let samples = load_audio_samples(audio_path)?;

    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Step 2: Transcribe
    #[cfg(feature = "whisper")]
    {
        transcribe_with_whisper(&samples, audio_path, config)
    }

    #[cfg(not(feature = "whisper"))]
    {
        let _ = config; // suppress unused warning
        let duration_secs = samples.len() as f64 / 16000.0;
        Ok(format!(
            "[Transcription placeholder — whisper feature not enabled]\n\
             Audio file: {}\n\
             Duration: {:.1}s ({} samples at 16kHz)\n\
             \n\
             Build with `cargo build --features whisper` and download a model\n\
             via `minutes setup` to enable real transcription.",
            audio_path.display(),
            duration_secs,
            samples.len(),
        ))
    }
}

/// Real transcription using whisper.cpp via whisper-rs.
#[cfg(feature = "whisper")]
fn transcribe_with_whisper(
    samples: &[f32],
    audio_path: &Path,
    config: &Config,
) -> Result<String, TranscribeError> {
    // Suppress unused warnings when whisper feature is disabled
    #[cfg(not(feature = "whisper"))]
    let _ = config;
    // Load whisper model
    let model_path = resolve_model_path(config)?;
    tracing::info!(model = %model_path.display(), "loading whisper model");

    let ctx = whisper_rs::WhisperContext::new_with_params(
        model_path
            .to_str()
            .ok_or_else(|| TranscribeError::ModelLoadError("invalid model path encoding".into()))?,
        whisper_rs::WhisperContextParameters::default(),
    )
    .map_err(|e| TranscribeError::ModelLoadError(format!("{}", e)))?;

    tracing::info!(
        samples = samples.len(),
        duration_secs = samples.len() as f64 / 16000.0,
        "starting whisper transcription"
    );

    let mut state = ctx
        .create_state()
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("create state: {}", e)))?;

    let mut params =
        whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });

    params.set_n_threads(num_cpus());
    params.set_language(Some("en"));
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_token_timestamps(true);

    state
        .full(params, samples)
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("{}", e)))?;

    let num_segments = state.full_n_segments();

    let mut transcript = String::new();
    for i in 0..num_segments {
        let segment = match state.get_segment(i) {
            Some(seg) => seg,
            None => continue,
        };

        let start_ts = segment.start_timestamp();
        let text = segment
            .to_str_lossy()
            .map_err(|e| TranscribeError::TranscriptionFailed(format!("get text: {}", e)))?;

        let text = text.trim();
        if text.is_empty() {
            continue;
        }

        let mins = start_ts / 6000;
        let secs = (start_ts % 6000) / 100;
        transcript.push_str(&format!("[{}:{:02}] {}\n", mins, secs, text));
    }

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        segments = num_segments,
        words = word_count,
        "transcription complete"
    );

    Ok(transcript)
}

/// Load audio from any supported format as 16kHz mono f32 samples.
fn load_audio_samples(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "wav" => load_wav(path),
        "m4a" | "mp3" | "ogg" | "webm" | "mp4" | "aac" => decode_with_symphonia(path),
        other => Err(TranscribeError::UnsupportedFormat(other.to_string())),
    }
}

/// Load WAV file as f32 samples, converting to 16kHz mono if needed.
fn load_wav(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let reader = hound::WavReader::open(path).map_err(|e| {
        if e.to_string().contains("Not a WAVE file") || e.to_string().contains("unexpected EOF") {
            TranscribeError::UnsupportedFormat("corrupt or invalid WAV file".into())
        } else {
            TranscribeError::Io(std::io::Error::other(e.to_string()))
        }
    })?;

    let spec = reader.spec();
    let sample_rate = spec.sample_rate;
    let channels = spec.channels as usize;

    // Read all samples as f32
    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / i32::MAX as f32)
            .collect(),
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
    };

    if raw_samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Convert to mono
    let mono = if channels > 1 {
        raw_samples
            .chunks(channels)
            .map(|frame| frame.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        raw_samples
    };

    // Resample to 16kHz if needed
    if sample_rate != 16000 {
        Ok(resample(&mono, sample_rate, 16000))
    } else {
        Ok(mono)
    }
}

/// Decode audio with symphonia (handles m4a, mp3, ogg, etc.)
/// Outputs 16kHz mono f32 samples.
fn decode_with_symphonia(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let format_opts = FormatOptions::default();
    let metadata_opts = MetadataOptions::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)
        .map_err(|e| TranscribeError::UnsupportedFormat(format!("probe failed: {}", e)))?;

    let mut format = probed.format;

    // Find the first audio track
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or_else(|| TranscribeError::UnsupportedFormat("no audio track found".into()))?;

    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.unwrap_or(44100);
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let decoder_opts = DecoderOptions::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &decoder_opts)
        .map_err(|e| TranscribeError::UnsupportedFormat(format!("decoder: {}", e)))?;

    let mut all_samples: Vec<f32> = Vec::new();

    // Decode all packets
    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break; // End of stream
            }
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(_) => continue, // Skip bad packets
        };

        let spec = *decoded.spec();
        let duration = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(duration as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        let samples = sample_buf.samples();

        // Convert to mono if needed
        if channels > 1 {
            for chunk in samples.chunks(channels) {
                let mono_sample = chunk.iter().sum::<f32>() / channels as f32;
                all_samples.push(mono_sample);
            }
        } else {
            all_samples.extend_from_slice(samples);
        }
    }

    if all_samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Resample to 16kHz if needed
    if sample_rate != 16000 {
        Ok(resample(&all_samples, sample_rate, 16000))
    } else {
        Ok(all_samples)
    }
}

/// Simple linear interpolation resampler.
/// Good enough for speech transcription (not music production).
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_idx = src_pos as usize;
        let frac = src_pos - src_idx as f64;

        let sample = if src_idx + 1 < samples.len() {
            samples[src_idx] as f64 * (1.0 - frac) + samples[src_idx + 1] as f64 * frac
        } else {
            samples[src_idx] as f64
        };

        output.push(sample as f32);
    }

    output
}

/// Resolve the whisper model file path.
#[cfg(feature = "whisper")]
fn resolve_model_path(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_name = &config.transcription.model;
    let model_dir = &config.transcription.model_path;

    // Try common naming patterns
    let candidates = [
        model_dir.join(format!("ggml-{}.bin", model_name)),
        model_dir.join(format!("whisper-{}.bin", model_name)),
        model_dir.join(format!("{}.bin", model_name)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Ok(candidate.clone());
        }
    }

    // If model_name is an absolute path, try it directly
    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return Ok(direct);
    }

    Err(TranscribeError::ModelNotFound(format!(
        "looked in {} for ggml-{}.bin — run: minutes setup --model {}",
        model_dir.display(),
        model_name,
        model_name,
    )))
}

/// Get number of CPU threads to use for whisper.
#[cfg(feature = "whisper")]
fn num_cpus() -> i32 {
    std::thread::available_parallelism()
        .map(|p| p.get() as i32)
        .unwrap_or(4)
        .min(8) // Cap at 8 — diminishing returns beyond that for whisper
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resample_preserves_length_proportionally() {
        let samples: Vec<f32> = (0..44100).map(|i| (i as f32 / 44100.0).sin()).collect();
        let resampled = resample(&samples, 44100, 16000);
        // Should be approximately 16000 samples
        let expected = 16000;
        assert!(
            (resampled.len() as i64 - expected as i64).unsigned_abs() < 10,
            "expected ~{} samples, got {}",
            expected,
            resampled.len()
        );
    }

    #[test]
    fn resample_noop_at_same_rate() {
        let samples = vec![1.0f32, 2.0, 3.0, 4.0];
        let resampled = resample(&samples, 16000, 16000);
        assert_eq!(samples, resampled);
    }

    #[test]
    #[cfg(feature = "whisper")]
    fn resolve_model_path_returns_error_for_missing() {
        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model: "nonexistent".into(),
                model_path: PathBuf::from("/tmp/no-such-dir"),
                min_words: 10,
            },
            ..Config::default()
        };
        let result = resolve_model_path(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("minutes setup"));
    }

    #[test]
    fn load_wav_rejects_empty_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("empty.wav");
        std::fs::write(&path, "").unwrap();
        let result = load_wav(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_wav_reads_valid_wav() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.wav");

        // Create a short WAV with hound
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for i in 0..16000 {
            let sample =
                (10000.0 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin()) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();

        let samples = load_wav(&path).unwrap();
        assert!(!samples.is_empty());
        // 1 second at 16kHz = 16000 samples
        assert_eq!(samples.len(), 16000);
    }

    #[test]
    fn load_audio_rejects_unknown_extension() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.xyz");
        std::fs::write(&path, "not audio").unwrap();
        let result = load_audio_samples(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("xyz"));
    }
}
