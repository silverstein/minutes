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

    // Step 2: Silence handling.
    // If Silero VAD model is available, whisper handles silence internally via
    // integrated VAD (set in default_whisper_params). Otherwise, fall back to
    // energy-based silence stripping to prevent hallucination loops (issue #21).
    #[cfg(feature = "whisper")]
    let use_integrated_vad = resolve_vad_model_path(config).is_some();
    #[cfg(not(feature = "whisper"))]
    let use_integrated_vad = false;

    let samples = if use_integrated_vad {
        tracing::debug!("Silero VAD available — skipping energy-based silence stripping");
        samples
    } else {
        strip_silence(&samples)
    };

    if samples.is_empty() {
        return Err(TranscribeError::EmptyAudio);
    }

    // Step 3: Transcribe
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
    _audio_path: &Path,
    config: &Config,
) -> Result<String, TranscribeError> {
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

    // Resolve VAD model path and convert to string for FullParams lifetime
    let vad_path = resolve_vad_model_path(config);
    let vad_path_str = vad_path.as_ref().and_then(|p| p.to_str());
    let mut params = default_whisper_params(vad_path_str);
    params.set_n_threads(num_cpus());
    params.set_language(config.transcription.language.as_deref());
    params.set_token_timestamps(true);

    state
        .full(params, samples)
        .map_err(|e| TranscribeError::TranscriptionFailed(format!("{}", e)))?;

    let num_segments = state.full_n_segments();

    // Collect segments, filtering by no_speech probability
    let mut lines: Vec<String> = Vec::new();
    let mut skipped_no_speech = 0u32;
    for i in 0..num_segments {
        let segment = match state.get_segment(i) {
            Some(seg) => seg,
            None => continue,
        };

        // Layer 3: Skip segments with high no_speech probability (likely hallucination)
        let no_speech_prob = segment.no_speech_probability();
        if no_speech_prob > 0.8 {
            skipped_no_speech += 1;
            tracing::debug!(
                segment = i,
                no_speech_prob = format!("{:.2}", no_speech_prob),
                "skipping segment — high no_speech probability"
            );
            continue;
        }

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
        lines.push(format!("[{}:{:02}] {}", mins, secs, text));
    }

    if skipped_no_speech > 0 {
        tracing::info!(
            skipped = skipped_no_speech,
            "filtered segments with high no_speech probability"
        );
    }

    // Layer 2: Remove repetition loops — detect consecutive near-identical segments
    let lines = dedup_segments(lines);

    let transcript = lines.join("\n");
    let transcript = if transcript.is_empty() {
        transcript
    } else {
        format!("{}\n", transcript)
    };

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        segments = num_segments,
        words = word_count,
        "transcription complete"
    );

    Ok(transcript)
}

/// Load audio from any supported format as 16kHz mono f32 samples.
///
/// For non-WAV formats (m4a, mp3, ogg, etc.), prefers ffmpeg when available
/// because symphonia's AAC decoder produces samples that cause whisper to
/// hallucinate on non-English audio (issue #21). Falls back to symphonia
/// when ffmpeg is not installed.
fn load_audio_samples(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        "wav" => load_wav(path),
        "m4a" | "mp3" | "ogg" | "webm" | "mp4" | "aac" => {
            // Prefer ffmpeg — its resampler and AAC decoder produce samples that
            // whisper transcribes correctly across all languages. Symphonia's AAC
            // decoder produces subtly different samples that trigger hallucination
            // loops on non-English audio (confirmed in issue #21).
            match decode_with_ffmpeg(path) {
                Ok(samples) => Ok(samples),
                Err(e) => {
                    let is_not_found = e.to_string().contains("not available")
                        || e.to_string().contains("not found");
                    if is_not_found {
                        tracing::warn!(
                            "ffmpeg not found — falling back to symphonia for {} decoding. \
                             Non-English audio may produce poor results. \
                             Install ffmpeg: brew install ffmpeg (macOS) / apt install ffmpeg (Linux)",
                            ext
                        );
                    } else {
                        tracing::warn!(
                            error = %e,
                            "ffmpeg decode failed — falling back to symphonia"
                        );
                    }
                    decode_with_symphonia(path)
                }
            }
        }
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

    // Read all samples as f32, normalizing by actual bit depth
    let bits = spec.bits_per_sample;
    let max_val = (1_i64 << (bits - 1)) as f32; // e.g. 16-bit → 32768.0
    let raw_samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => reader
            .into_samples::<i32>()
            .filter_map(|s| s.ok())
            .map(|s| s as f32 / max_val)
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
    let resampled = if sample_rate != 16000 {
        resample(&mono, sample_rate, 16000)
    } else {
        mono
    };

    // Auto-normalize: if peak is below target, boost so whisper gets usable levels.
    // Quiet mics (e.g. MacBook Pro) can produce peaks of 0.004 which whisper can't detect.
    Ok(normalize_audio(resampled))
}

/// Decode audio with ffmpeg (preferred for non-WAV formats).
///
/// Shells out to `ffmpeg` to convert any audio to 16kHz mono f32le PCM.
/// This matches exactly what whisper-cli does and produces samples that
/// whisper transcribes correctly across all languages.
///
/// Returns an error if ffmpeg is not installed or the conversion fails,
/// allowing the caller to fall back to symphonia.
fn decode_with_ffmpeg(path: &Path) -> Result<Vec<f32>, TranscribeError> {
    use std::process::Command;

    let tmp_dir = std::env::temp_dir();
    let tmp_wav = tmp_dir.join(format!("minutes-ffmpeg-{}.wav", std::process::id()));

    let output = Command::new("ffmpeg")
        .args([
            "-i",
            path.to_str().unwrap_or(""),
            "-ar",
            "16000", // 16kHz sample rate
            "-ac",
            "1", // mono
            "-f",
            "wav", // WAV output
            "-y",  // overwrite
        ])
        .arg(&tmp_wav)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            TranscribeError::TranscriptionFailed(format!("ffmpeg not available: {}", e))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Clean up temp file on failure
        let _ = std::fs::remove_file(&tmp_wav);
        return Err(TranscribeError::TranscriptionFailed(format!(
            "ffmpeg conversion failed: {}",
            stderr.lines().last().unwrap_or("unknown error")
        )));
    }

    tracing::info!(
        source = %path.display(),
        "decoded audio with ffmpeg (16kHz mono WAV)"
    );

    // Load the ffmpeg-produced WAV (already 16kHz mono)
    let result = load_wav(&tmp_wav);

    // Clean up temp file
    let _ = std::fs::remove_file(&tmp_wav);

    result
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
    let resampled = if sample_rate != 16000 {
        resample(&all_samples, sample_rate, 16000)
    } else {
        all_samples
    };

    Ok(normalize_audio(resampled))
}

/// Windowed-sinc resampler for high-quality rate conversion.
///
/// Linear interpolation introduces aliasing when downsampling (e.g. 44100→16000)
/// because it doesn't low-pass filter first. This matters for whisper: aliased
/// artifacts confuse the decoder and contribute to hallucination loops on
/// non-English audio (issue #21).
///
/// This uses a sinc kernel with a Hann window (width=32 taps). The cutoff
/// frequency is set to the Nyquist of the lower rate, preventing aliasing.
/// Quality is comparable to ffmpeg's default SWR resampler.
fn resample(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate {
        return samples.to_vec();
    }

    let ratio = from_rate as f64 / to_rate as f64;
    let output_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(output_len);

    // Cutoff at Nyquist of the lower rate to prevent aliasing
    let cutoff = if to_rate < from_rate {
        to_rate as f64 / from_rate as f64
    } else {
        1.0
    };

    const HALF_WIDTH: i32 = 16; // 32-tap kernel

    for i in 0..output_len {
        let src_pos = i as f64 * ratio;
        let src_center = src_pos as i32;

        let mut sum = 0.0f64;
        let mut weight_sum = 0.0f64;

        for j in (src_center - HALF_WIDTH + 1)..=(src_center + HALF_WIDTH) {
            if j < 0 || j >= samples.len() as i32 {
                continue;
            }

            let delta = src_pos - j as f64;

            // Sinc function with cutoff
            let sinc = if delta.abs() < 1e-10 {
                cutoff
            } else {
                let x = std::f64::consts::PI * delta * cutoff;
                (x.sin() / (std::f64::consts::PI * delta)) * cutoff
            };

            // Hann window
            let window_pos = (delta / HALF_WIDTH as f64 + 1.0) * 0.5;
            let window = if (0.0..=1.0).contains(&window_pos) {
                0.5 * (1.0 - (2.0 * std::f64::consts::PI * window_pos).cos())
            } else {
                0.0
            };

            let w = sinc * window;
            sum += samples[j as usize] as f64 * w;
            weight_sum += w;
        }

        let sample = if weight_sum.abs() > 1e-10 {
            sum / weight_sum
        } else {
            0.0
        };

        output.push(sample as f32);
    }

    output
}

/// Normalize audio to a target peak level for consistent whisper input.
/// Only boosts quiet audio — already-loud recordings are left untouched.
fn normalize_audio(mut samples: Vec<f32>) -> Vec<f32> {
    if samples.is_empty() {
        return samples;
    }

    let peak = samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);

    // Target peak: 0.5 (leaves headroom, loud enough for whisper)
    // Only normalize if peak is below 0.1 (quiet mic) and above noise floor
    const TARGET_PEAK: f32 = 0.5;
    const QUIET_THRESHOLD: f32 = 0.1;
    const NOISE_FLOOR: f32 = 0.0001;

    if peak < QUIET_THRESHOLD && peak > NOISE_FLOOR {
        let gain = TARGET_PEAK / peak;
        // Cap gain at 100x to avoid amplifying pure noise
        let gain = gain.min(100.0);
        tracing::info!(
            peak = format!("{:.4}", peak),
            gain = format!("{:.1}x", gain),
            "auto-normalizing quiet audio"
        );
        for s in &mut samples {
            *s = (*s * gain).clamp(-1.0, 1.0);
        }
    }

    samples
}

/// Detect and remove repetition loops from whisper output (issue #21).
///
/// Whisper's decoder can get stuck repeating the same text across consecutive segments,
/// especially on non-English audio. This function detects runs of 3+ consecutive segments
/// with >80% text overlap and collapses them to the first occurrence.
#[allow(dead_code)] // Only used with whisper feature
fn dedup_segments(lines: Vec<String>) -> Vec<String> {
    if lines.len() < 3 {
        return lines;
    }

    // Extract just the text portion (after the timestamp) for comparison
    fn text_part(line: &str) -> &str {
        // Lines look like "[0:00] some text"
        line.find("] ").map(|i| &line[i + 2..]).unwrap_or(line)
    }

    // Simple text similarity: ratio of matching chars to total chars (normalized)
    fn similarity(a: &str, b: &str) -> f64 {
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }
        let a_lower = a.to_lowercase();
        let b_lower = b.to_lowercase();
        if a_lower == b_lower {
            return 1.0;
        }
        // Use longest common substring ratio as a fast similarity measure
        let (short, long) = if a_lower.len() <= b_lower.len() {
            (&a_lower, &b_lower)
        } else {
            (&b_lower, &a_lower)
        };
        if long.contains(short.as_str()) {
            return short.len() as f64 / long.len() as f64;
        }
        // Count matching words as fallback
        let a_words: Vec<&str> = a_lower.split_whitespace().collect();
        let b_words: Vec<&str> = b_lower.split_whitespace().collect();
        let matching = a_words.iter().filter(|w| b_words.contains(w)).count();
        let total = a_words.len().max(b_words.len());
        if total == 0 {
            return 0.0;
        }
        matching as f64 / total as f64
    }

    let mut result = Vec::with_capacity(lines.len());
    let mut i = 0;

    while i < lines.len() {
        // Look ahead for a run of similar segments
        let base_text = text_part(&lines[i]);
        let mut run_end = i + 1;

        while run_end < lines.len() {
            let candidate = text_part(&lines[run_end]);
            if similarity(base_text, candidate) >= 0.8 {
                run_end += 1;
            } else {
                break;
            }
        }

        let run_len = run_end - i;

        if run_len >= 3 {
            // Repetition detected — keep only the first segment
            tracing::warn!(
                first_segment = i,
                repeated_count = run_len,
                text = base_text,
                "detected repetition loop in whisper output — collapsing {} segments",
                run_len
            );
            result.push(lines[i].clone());
            result.push(format!(
                "[...] [repeated audio removed — {} identical segments collapsed]",
                run_len - 1
            ));
            i = run_end;
        } else {
            result.push(lines[i].clone());
            i += 1;
        }
    }

    result
}

/// Strip silence from audio using energy detection, replacing long gaps with short padding.
///
/// Whisper hallucinates repeating text when fed long silence segments,
/// especially on non-English audio. This function:
/// 1. Computes RMS energy per 100ms chunk with adaptive noise floor
/// 2. Keeps all speech chunks plus context padding
/// 3. Replaces silence gaps >500ms with 300ms of zero padding (enough for
///    whisper to detect a segment boundary without triggering hallucination)
fn strip_silence(samples: &[f32]) -> Vec<f32> {
    const SAMPLE_RATE: usize = 16000;
    const CHUNK_SIZE: usize = SAMPLE_RATE / 10; // 100ms chunks
    const MAX_SILENCE_CHUNKS: usize = 5; // 500ms — silence beyond this gets trimmed
    const PAD_CHUNKS: usize = 3; // 300ms of silence inserted at gap boundaries
    const CONTEXT_CHUNKS: usize = 2; // 200ms of context kept around speech
    const ENERGY_MULTIPLIER: f32 = 4.0; // speech must be 4x above noise floor

    if samples.len() < CHUNK_SIZE * 3 {
        return samples.to_vec();
    }

    let num_chunks = samples.len() / CHUNK_SIZE;

    // Phase 1: compute RMS per chunk
    let rms_values: Vec<f32> = (0..num_chunks)
        .map(|i| {
            let start = i * CHUNK_SIZE;
            let end = (start + CHUNK_SIZE).min(samples.len());
            let chunk = &samples[start..end];
            (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt()
        })
        .collect();

    // Phase 2: estimate noise floor from the quietest 20% of chunks
    let mut sorted_rms = rms_values.clone();
    sorted_rms.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let quiet_count = (num_chunks / 5).max(1);
    let noise_floor =
        (sorted_rms[..quiet_count].iter().sum::<f32>() / quiet_count as f32).clamp(0.0001, 0.02);
    let threshold = noise_floor * ENERGY_MULTIPLIER;

    // Phase 3: classify chunks as speech (with hangover to avoid flapping)
    let mut is_speech = vec![false; num_chunks];
    let mut hangover = 0u32;
    const HANGOVER_CHUNKS: u32 = 5; // 500ms hangover
    for (i, rms) in rms_values.iter().enumerate() {
        if *rms > threshold {
            is_speech[i] = true;
            hangover = HANGOVER_CHUNKS;
        } else if hangover > 0 {
            is_speech[i] = true;
            hangover -= 1;
        }
    }

    // Phase 4: expand speech regions by CONTEXT_CHUNKS in each direction
    let mut keep = is_speech.clone();
    for (i, &speech) in is_speech.iter().enumerate() {
        if speech {
            let from = i.saturating_sub(CONTEXT_CHUNKS);
            let to = (i + CONTEXT_CHUNKS + 1).min(num_chunks);
            for k in &mut keep[from..to] {
                *k = true;
            }
        }
    }

    // Phase 5: assemble output — keep speech, replace long silence with short pad
    let mut output = Vec::with_capacity(samples.len());
    let mut consecutive_silence = 0usize;
    let silence_pad: Vec<f32> = vec![0.0; PAD_CHUNKS * CHUNK_SIZE];

    for (i, &kept) in keep.iter().enumerate() {
        let start = i * CHUNK_SIZE;
        let end = (start + CHUNK_SIZE).min(samples.len());

        if kept {
            if consecutive_silence > MAX_SILENCE_CHUNKS {
                output.extend_from_slice(&silence_pad);
            }
            consecutive_silence = 0;
            output.extend_from_slice(&samples[start..end]);
        } else {
            consecutive_silence += 1;
            if consecutive_silence <= MAX_SILENCE_CHUNKS {
                output.extend_from_slice(&samples[start..end]);
            }
        }
    }

    // Include any trailing partial chunk
    let remainder_start = num_chunks * CHUNK_SIZE;
    if remainder_start < samples.len() {
        output.extend_from_slice(&samples[remainder_start..]);
    }

    let original_secs = samples.len() as f64 / SAMPLE_RATE as f64;
    let stripped_secs = output.len() as f64 / SAMPLE_RATE as f64;
    if stripped_secs < original_secs * 0.95 {
        tracing::info!(
            original_secs = format!("{:.1}", original_secs),
            stripped_secs = format!("{:.1}", stripped_secs),
            removed_pct = format!("{:.0}", (1.0 - stripped_secs / original_secs) * 100.0),
            "VAD stripped silence from audio"
        );
    }

    output
}

/// Resolve the whisper model file path for dictation (uses dictation.model config).
#[cfg(feature = "whisper")]
pub fn resolve_model_path_for_dictation(config: &Config) -> Result<PathBuf, TranscribeError> {
    let model_name = &config.dictation.model;
    let model_dir = &config.transcription.model_path;

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

    let direct = PathBuf::from(model_name);
    if direct.exists() {
        return Ok(direct);
    }

    Err(TranscribeError::ModelNotFound(format!(
        "Expected model file \"ggml-{}.bin\" in {}",
        model_name,
        model_dir.display(),
    )))
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
        "Expected model file \"ggml-{}.bin\" in {}",
        model_name,
        model_dir.display(),
    )))
}

/// Resolve the Silero VAD model path. Returns None if VAD is disabled or model not found.
#[cfg(feature = "whisper")]
fn resolve_vad_model_path(config: &Config) -> Option<PathBuf> {
    let vad_model = &config.transcription.vad_model;
    if vad_model.is_empty() {
        return None;
    }

    let model_dir = &config.transcription.model_path;
    let candidates = [
        model_dir.join(format!("ggml-{}.bin", vad_model)),
        model_dir.join(format!("{}.bin", vad_model)),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    // Try as absolute path
    let direct = PathBuf::from(vad_model);
    if direct.exists() {
        return Some(direct);
    }

    tracing::debug!(
        vad_model = vad_model,
        "VAD model not found — falling back to energy-based silence stripping"
    );
    None
}

/// Build whisper FullParams with sane defaults matching whisper.cpp CLI.
///
/// The whisper.cpp CLI uses `best_of=5`, entropy/logprob thresholds, and
/// temperature fallback to prevent decoder loops on non-English or noisy
/// audio. Without these, `Greedy { best_of: 1 }` can repeat gibberish
/// indefinitely (see GitHub issue #21).
///
/// When a Silero VAD model is available, enables integrated VAD so whisper
/// only transcribes speech segments (matching whisper-cli behavior exactly).
///
/// Use this for batch transcription. For latency-sensitive streaming,
/// use [`streaming_whisper_params`] instead.
#[cfg(feature = "whisper")]
pub fn default_whisper_params<'a, 'b>(
    vad_model_path: Option<&str>,
) -> whisper_rs::FullParams<'a, 'b> {
    let mut params =
        whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 5 });

    // Match whisper.cpp CLI defaults for stable decoding
    params.set_temperature(0.0);
    params.set_temperature_inc(0.2); // retry at higher temp on high-entropy segments
    params.set_entropy_thold(2.4); // flag segments with entropy above this
    params.set_logprob_thold(-1.0); // flag segments with avg logprob below this
    params.set_no_speech_thold(0.6); // probability threshold for silence detection
    params.set_suppress_blank(true); // suppress blank/repeated token hallucinations

    // Enable Silero VAD if model is available — this is the key difference vs whisper-cli.
    // Without VAD, silence segments trigger decoder hallucination loops, especially on
    // non-English audio (issue #21).
    if let Some(path) = vad_model_path {
        params.set_vad_model_path(Some(path));
        params.enable_vad(true);
        params.set_vad_params(whisper_rs::WhisperVadParams::default());
        tracing::info!("Silero VAD enabled for transcription");
    }

    // Suppress noisy output
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params
}

/// Lighter whisper params for streaming/dictation where latency matters.
///
/// Keeps `best_of=1` and disables temperature fallback to stay within
/// the ~200ms (base) / ~500ms (small) budget for partial transcription.
/// Still sets entropy/logprob/no-speech thresholds and suppress_blank
/// to catch the worst hallucinations without the 5x cost of best_of=5.
#[cfg(feature = "whisper")]
pub fn streaming_whisper_params<'a, 'b>() -> whisper_rs::FullParams<'a, 'b> {
    let mut params =
        whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });

    params.set_temperature(0.0);
    params.set_temperature_inc(0.0); // no retry — latency budget too tight
    params.set_entropy_thold(2.4);
    params.set_logprob_thold(-1.0);
    params.set_no_speech_thold(0.6);
    params.set_suppress_blank(true);

    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);

    params
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
    fn normalize_boosts_quiet_audio() {
        // Peak 0.01 → gain = 0.5/0.01 = 50x → new peak = 0.5
        let samples = vec![0.005f32, -0.008, 0.01, -0.003, 0.007];
        let normalized = normalize_audio(samples);
        let peak = normalized.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(peak > 0.4, "expected peak > 0.4, got {}", peak);
        assert!(peak <= 0.5, "expected peak <= 0.5, got {}", peak);
    }

    #[test]
    fn normalize_leaves_loud_audio_untouched() {
        let samples = vec![0.3f32, -0.5, 0.2, -0.1];
        let normalized = normalize_audio(samples.clone());
        assert_eq!(samples, normalized);
    }

    #[test]
    fn normalize_ignores_noise_floor() {
        let samples = vec![0.00001f32, -0.00002, 0.00001];
        let normalized = normalize_audio(samples.clone());
        // Below noise floor — should not be boosted
        assert_eq!(samples, normalized);
    }

    #[test]
    #[cfg(feature = "whisper")]
    fn resolve_model_path_returns_error_for_missing() {
        let config = Config {
            transcription: crate::config::TranscriptionConfig {
                model: "nonexistent".into(),
                model_path: PathBuf::from("/tmp/no-such-dir"),
                min_words: 10,
                language: Some("en".into()),
                vad_model: String::new(),
            },
            ..Config::default()
        };
        let result = resolve_model_path(&config);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("minutes setup --model tiny"),
            "error should tell user how to fix it: {}",
            err
        );
        assert!(
            err.contains("ggml-nonexistent.bin"),
            "error should include expected model filename: {}",
            err
        );
        assert!(
            err.contains("/tmp/no-such-dir"),
            "error should include the model directory: {}",
            err
        );
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

    #[test]
    fn strip_silence_preserves_speech() {
        // 1s of "speech" (high energy sine wave)
        let speech: Vec<f32> = (0..16000)
            .map(|i| 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin())
            .collect();
        let result = strip_silence(&speech);
        // All speech — nothing should be stripped
        assert_eq!(result.len(), speech.len());
    }

    #[test]
    fn strip_silence_trims_long_silence() {
        let mut samples = Vec::new();
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }
        // 5s silence
        samples.extend(vec![0.0f32; 16000 * 5]);
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }

        let result = strip_silence(&samples);
        // Should be significantly shorter than 7s (5s of silence trimmed)
        let original_secs = samples.len() as f64 / 16000.0;
        let result_secs = result.len() as f64 / 16000.0;
        assert!(
            result_secs < original_secs * 0.7,
            "expected significant trimming: {:.1}s → {:.1}s",
            original_secs,
            result_secs
        );
        // But should still have both speech segments + padding
        assert!(
            result_secs > 2.0,
            "should preserve both speech segments: {:.1}s",
            result_secs
        );
    }

    #[test]
    fn strip_silence_keeps_short_pauses() {
        let mut samples = Vec::new();
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }
        // 400ms silence (short natural pause — should be kept)
        samples.extend(vec![0.0f32; 6400]);
        // 1s speech
        for i in 0..16000 {
            samples.push(0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16000.0).sin());
        }

        let result = strip_silence(&samples);
        // Short pause should be preserved — output ≈ input length
        let ratio = result.len() as f64 / samples.len() as f64;
        assert!(
            ratio > 0.9,
            "short pauses should be preserved: ratio {:.2}",
            ratio
        );
    }

    #[test]
    fn strip_silence_handles_all_silence() {
        let samples = vec![0.0f32; 16000 * 10]; // 10s of silence
        let result = strip_silence(&samples);
        // Should still produce something (short pad at minimum)
        assert!(result.len() < samples.len() / 2, "should trim most silence");
    }

    #[test]
    fn sinc_resample_no_aliasing() {
        // Generate a 440Hz tone at 44100Hz, resample to 16000Hz.
        // 440Hz is well below Nyquist (8000Hz), so it should survive.
        let n = 44100;
        let samples: Vec<f32> = (0..n)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        let resampled = resample(&samples, 44100, 16000);

        // Check the resampled signal has reasonable amplitude (not attenuated to nothing)
        let peak = resampled.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            peak > 0.8,
            "440Hz tone should survive resampling with peak > 0.8, got {}",
            peak
        );
    }

    #[test]
    fn dedup_no_repetition() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] How are you".into(),
            "[0:06] Fine thanks".into(),
        ];
        let result = dedup_segments(lines.clone());
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_collapses_exact_repetition() {
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Hello world".into(),
            "[0:09] Hello world".into(),
            "[0:12] Something different".into(),
        ];
        let result = dedup_segments(lines);
        assert_eq!(result.len(), 3); // first + marker + different
        assert!(result[0].contains("Hello world"));
        assert!(result[1].contains("repeated audio removed"));
        assert!(result[2].contains("Something different"));
    }

    #[test]
    fn dedup_collapses_near_identical() {
        // Whisper often produces slight variations of the same repeated text
        let lines = vec![
            "[0:00] Ok bene le macedi diesel".into(),
            "[0:03] Ok, bene le macedi diesel".into(),
            "[0:06] Ok bene, le macedi diesel".into(),
            "[0:09] Good morning".into(),
        ];
        let result = dedup_segments(lines);
        assert_eq!(result.len(), 3); // first + marker + different
        assert!(result[1].contains("repeated audio removed"));
    }

    #[test]
    fn dedup_leaves_two_similar_alone() {
        // Only 2 similar — below threshold of 3
        let lines = vec![
            "[0:00] Hello world".into(),
            "[0:03] Hello world".into(),
            "[0:06] Something else".into(),
        ];
        let result = dedup_segments(lines.clone());
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_handles_empty() {
        let result = dedup_segments(vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn dedup_handles_single_line() {
        let lines = vec!["[0:00] Hello".into()];
        let result = dedup_segments(lines.clone());
        assert_eq!(result, lines);
    }

    #[test]
    fn dedup_multiple_runs() {
        let lines = vec![
            "[0:00] First phrase".into(),
            "[0:03] First phrase".into(),
            "[0:06] First phrase".into(),
            "[0:09] Second phrase".into(),
            "[0:12] Second phrase".into(),
            "[0:15] Second phrase".into(),
            "[0:18] Second phrase".into(),
            "[0:21] Normal text".into(),
        ];
        let result = dedup_segments(lines);
        // Two collapsed runs + normal text
        assert_eq!(result.len(), 5); // first + marker + second + marker + normal
        assert!(result[1].contains("2 identical"));
        assert!(result[3].contains("3 identical"));
    }
}
