use crate::config::Config;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ──────────────────────────────────────────────────────────────
// Speaker diarization.
//
// Engines:
//   "pyannote-rs" → Native Rust via pyannote-rs crate (recommended)
//   "pyannote"    → Python pyannote.audio subprocess (legacy)
//   "none"        → Skip diarization (default)
//
// The pyannote-rs engine uses ONNX models (~34 MB total):
//   - segmentation-3.0.onnx (speech segmentation)
//   - voxceleb_CAM++_LM.onnx (speaker embeddings, large-margin fine-tuned)
//
// Download with: minutes setup --diarization
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeakerSegment {
    pub speaker: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone)]
pub struct DiarizationResult {
    pub segments: Vec<SpeakerSegment>,
    pub num_speakers: usize,
    /// Per-speaker averaged embeddings (for Level 3 confirmed learning).
    /// Empty when using the Python subprocess engine.
    pub speaker_embeddings: std::collections::HashMap<String, Vec<f32>>,
}

// ── Speaker attribution ──────────────────────────────────────

/// How confident we are that a speaker label maps to a real person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    High,
    Medium,
    Low,
}

/// How the attribution was determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum AttributionSource {
    Deterministic,
    Llm,
    Enrollment,
    Manual,
}

/// A mapping from an anonymous speaker label to a real person.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SpeakerAttribution {
    pub speaker_label: String,
    pub name: String,
    pub confidence: Confidence,
    pub source: AttributionSource,
}

/// Rewrite speaker labels in a transcript for high-confidence attributions only.
pub fn apply_confirmed_names(transcript: &str, attributions: &[SpeakerAttribution]) -> String {
    let high_map: std::collections::HashMap<&str, &str> = attributions
        .iter()
        .filter(|a| a.confidence == Confidence::High)
        .map(|a| (a.speaker_label.as_str(), a.name.as_str()))
        .collect();

    if high_map.is_empty() {
        return transcript.to_string();
    }

    let mut output = String::new();
    for line in transcript.lines() {
        let mut replaced = false;
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let inside = &rest[..bracket_end];
                if let Some(space_pos) = inside.find(' ') {
                    let label = &inside[..space_pos];
                    if let Some(name) = high_map.get(label) {
                        let after = &rest[bracket_end..];
                        output.push_str(&format!(
                            "[{} {}{}\n",
                            name,
                            &inside[space_pos + 1..],
                            after
                        ));
                        replaced = true;
                    }
                }
            }
        }
        if !replaced {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

/// Model filenames expected by pyannote-rs.
pub const SEGMENTATION_MODEL: &str = "segmentation-3.0.onnx";

pub const SEGMENTATION_MODEL_URL: &str =
    "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/segmentation-3.0.onnx";

/// Descriptor for a speaker embedding ONNX model.
pub struct EmbeddingModelInfo {
    pub filename: &'static str,
    pub url: &'static str,
    pub version: &'static str,
}

/// Resolve the configured embedding model name to its ONNX file, download URL,
/// and version tag stored alongside voice profiles.
pub fn embedding_model_info(name: &str) -> Option<&'static EmbeddingModelInfo> {
    static CAM_PP: EmbeddingModelInfo = EmbeddingModelInfo {
        filename: "wespeaker_en_voxceleb_CAM++.onnx",
        url: "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/wespeaker_en_voxceleb_CAM++.onnx",
        version: "wespeaker_en_voxceleb_CAM++_v0.3",
    };
    static CAM_PP_LM: EmbeddingModelInfo = EmbeddingModelInfo {
        filename: "voxceleb_CAM++_LM.onnx",
        url: "https://huggingface.co/Wespeaker/wespeaker-voxceleb-campplus-LM/resolve/main/voxceleb_CAM%2B%2B_LM.onnx",
        version: "wespeaker_voxceleb_CAM++_LM_v0.3",
    };

    match name {
        "cam++" => Some(&CAM_PP),
        "cam++-lm" => Some(&CAM_PP_LM),
        _ => None,
    }
}

/// All recognized embedding model names (for help / error messages).
pub const EMBEDDING_MODEL_NAMES: &[&str] = &["cam++", "cam++-lm"];

/// Resolve from config, falling back to the default (cam++).
pub fn embedding_model_for_config(config: &Config) -> &'static EmbeddingModelInfo {
    embedding_model_info(&config.diarization.embedding_model)
        .unwrap_or_else(|| embedding_model_info("cam++").unwrap())
}

/// Check if diarization models are installed.
pub fn models_installed(config: &Config) -> bool {
    let dir = &config.diarization.model_path;
    let emb = embedding_model_for_config(config);
    dir.join(SEGMENTATION_MODEL).exists() && dir.join(emb.filename).exists()
}

/// Pre-process audio to 16kHz mono WAV via ffmpeg (if available).
/// Returns (effective_path, temp_path_to_cleanup).
/// pyannote-rs works best with 16kHz mono s16 WAV. Live recordings from cpal
/// are often 44.1kHz F32, which the symphonia fallback can struggle with.
fn preprocess_audio(audio_path: &Path) -> (std::path::PathBuf, Option<std::path::PathBuf>) {
    let temp_path = std::env::temp_dir().join("minutes-diarize-preprocessed.wav");

    match std::process::Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            audio_path.to_str().unwrap_or(""),
            "-ar",
            "16000",
            "-ac",
            "1",
            "-sample_fmt",
            "s16",
            temp_path.to_str().unwrap_or(""),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            tracing::info!("audio preprocessed to 16kHz mono via ffmpeg");
            (temp_path.clone(), Some(temp_path))
        }
        _ => {
            tracing::debug!("ffmpeg not available for preprocessing, using original audio");
            (audio_path.to_path_buf(), None)
        }
    }
}

/// Paths to per-source audio stems from a multi-source call capture.
#[derive(Debug, Clone)]
pub struct StemPaths {
    pub voice: std::path::PathBuf,
    pub system: std::path::PathBuf,
}

/// Discover stem files alongside an audio file.
/// The native call helper writes `{basename}.voice.wav` and `{basename}.system.wav`
/// next to the main recording. Returns Some only if both files exist and are non-empty.
pub fn discover_stems(audio_path: &Path) -> Option<StemPaths> {
    let stem = audio_path.file_stem()?.to_str()?;
    let dir = audio_path.parent()?;
    let voice = dir.join(format!("{}.voice.wav", stem));
    let system = dir.join(format!("{}.system.wav", stem));

    let voice_ok = std::fs::metadata(&voice)
        .map(|m| m.len() > 44) // WAV header is 44 bytes; must have actual data
        .unwrap_or(false);
    let system_ok = std::fs::metadata(&system)
        .map(|m| m.len() > 44)
        .unwrap_or(false);

    if voice_ok && system_ok {
        tracing::info!(
            voice = %voice.display(),
            system = %system.display(),
            "discovered per-source audio stems"
        );
        Some(StemPaths { voice, system })
    } else {
        None
    }
}

/// Compute RMS energy per time window from a WAV file.
/// Returns a vec of (start_secs, rms) tuples, one per window.
fn compute_energy_windows(wav_path: &Path, window_secs: f64) -> Result<Vec<(f64, f32)>, String> {
    let reader = hound::WavReader::open(wav_path)
        .map_err(|e| format!("failed to open stem {}: {}", wav_path.display(), e))?;
    let spec = reader.spec();
    let sample_rate = spec.sample_rate as f64;
    let window_samples = (sample_rate * window_secs) as usize;

    if window_samples == 0 {
        return Err("window too small".into());
    }

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader
            .into_samples::<f32>()
            .filter_map(|s| s.ok())
            .collect(),
        hound::SampleFormat::Int => {
            let bits = spec.bits_per_sample;
            let max_val = (1i64 << (bits - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(|s| s.ok())
                .map(|s| s as f32 / max_val)
                .collect()
        }
    };

    // Mix to mono if multi-channel
    let channels = spec.channels as usize;
    let mono: Vec<f32> = if channels > 1 {
        samples
            .chunks(channels)
            .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
            .collect()
    } else {
        samples
    };

    let mut windows = Vec::new();
    for (i, chunk) in mono.chunks(window_samples).enumerate() {
        let sum_sq: f64 = chunk.iter().map(|&s| (s as f64) * (s as f64)).sum();
        let rms = (sum_sq / chunk.len() as f64).sqrt() as f32;
        let start = i as f64 * window_secs;
        windows.push((start, rms));
    }

    Ok(windows)
}

/// Speaker attribution from per-source audio stems (no ML diarization).
/// Compares energy levels between voice and system stems per time window,
/// assigning "SPEAKER_0" (you) or "SPEAKER_1" (remote) to each window.
pub fn diarize_from_stems(stems: &StemPaths, _config: &Config) -> Option<DiarizationResult> {
    let window_secs = 1.0; // 1-second energy windows

    let voice_energy = match compute_energy_windows(&stems.voice, window_secs) {
        Ok(e) => e,
        Err(error) => {
            tracing::warn!(error = %error, "failed to read voice stem, falling back to ML diarization");
            return None;
        }
    };
    let system_energy = match compute_energy_windows(&stems.system, window_secs) {
        Ok(e) => e,
        Err(error) => {
            tracing::warn!(error = %error, "failed to read system stem, falling back to ML diarization");
            return None;
        }
    };

    // Energy threshold: below this RMS, the source is considered silent.
    // Typical speech RMS is 0.01-0.1; noise floor is <0.001.
    let silence_threshold = 0.005_f32;

    let mut segments: Vec<SpeakerSegment> = Vec::new();
    let window_count = voice_energy.len().min(system_energy.len());

    // Use SPEAKER_0/SPEAKER_1 labels to match apply_speakers expectations.
    // The attribution pipeline maps these to real names via speaker_map.
    let voice_label = "SPEAKER_0";
    let call_label = "SPEAKER_1";

    for i in 0..window_count {
        let (start, voice_rms) = voice_energy[i];
        let (_, system_rms) = system_energy[i];
        let end = start + window_secs;

        let voice_active = voice_rms > silence_threshold;
        let system_active = system_rms > silence_threshold;

        let speaker = if voice_active && !system_active {
            voice_label.to_string()
        } else if system_active && !voice_active {
            call_label.to_string()
        } else if voice_active && system_active {
            if voice_rms >= system_rms {
                voice_label.to_string()
            } else {
                call_label.to_string()
            }
        } else {
            continue; // Both silent, skip
        };

        // Merge with previous segment if same speaker
        if let Some(last) = segments.last_mut() {
            if last.speaker == speaker && (start - last.end).abs() < 0.01 {
                last.end = end;
                continue;
            }
        }

        segments.push(SpeakerSegment {
            speaker,
            start,
            end,
        });
    }

    let num_speakers = segments
        .iter()
        .map(|s| s.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // If no segments were produced (both stems are silent), fall back to ML
    if segments.is_empty() {
        tracing::warn!("stem-based diarization produced no segments (all silent), falling back");
        return None;
    }

    tracing::info!(
        speakers = num_speakers,
        segments = segments.len(),
        voice_stem = %stems.voice.display(),
        system_stem = %stems.system.display(),
        "stem-based diarization complete"
    );

    Some(DiarizationResult {
        segments,
        num_speakers,
        speaker_embeddings: std::collections::HashMap::new(),
    })
}

/// Run speaker diarization on an audio file.
/// Returns None if diarization is disabled or models are not available.
///
/// When per-source stems are available alongside the audio file,
/// uses energy-based attribution instead of ML diarization.
///
/// Engine options:
/// - `"auto"` (default): use pyannote-rs if models are downloaded, otherwise skip
/// - `"pyannote-rs"`: native Rust diarization (requires `minutes setup --diarization`)
/// - `"pyannote"`: legacy Python subprocess (requires `pip install pyannote.audio`)
/// - `"none"`: explicitly disabled
pub fn diarize(audio_path: &Path, config: &Config) -> Option<DiarizationResult> {
    let engine = &config.diarization.engine;

    if engine == "none" {
        return None;
    }

    // Check for per-source stems alongside the audio file.
    // If stems exist, use energy-based attribution (zero ML cost).
    if let Some(stems) = discover_stems(audio_path) {
        if let Some(result) = diarize_from_stems(&stems, config) {
            return Some(result);
        }
        // Stem attribution failed, fall through to ML diarization
        tracing::warn!("stem-based diarization failed, falling back to ML engine");
    }

    // "auto" mode: use pyannote-rs if models are downloaded, otherwise skip silently
    let resolved_engine = if engine == "auto" {
        if models_installed(config) {
            tracing::info!("diarization models found — auto-enabling pyannote-rs");
            "pyannote-rs"
        } else {
            tracing::debug!("diarization models not found — skipping (run `minutes setup --diarization` to enable)");
            return None;
        }
    } else {
        engine.as_str()
    };

    tracing::info!(engine = %resolved_engine, file = %audio_path.display(), "running diarization");

    // Pre-process: resample to 16kHz mono via ffmpeg if available.
    // pyannote-rs/symphonia can struggle with 44.1kHz F32 WAVs from live capture.
    // This matches how transcribe.rs preprocesses audio for whisper.
    let (effective_path, _temp_file) = preprocess_audio(audio_path);

    // Run diarization in a separate thread so we can detect panics and
    // keep the main pipeline from getting stuck on ONNX inference issues.
    let effective_path_owned = effective_path.clone();
    #[allow(unused_variables)] // config_clone is used only when the diarize feature is enabled
    let config_clone = config.clone();
    let engine_owned = resolved_engine.to_string();
    let handle = std::thread::spawn(move || -> Result<DiarizationResult, String> {
        let result = match engine_owned.as_str() {
            #[cfg(feature = "diarize")]
            "pyannote-rs" => diarize_with_pyannote_rs(&effective_path_owned, &config_clone),
            #[cfg(not(feature = "diarize"))]
            "pyannote-rs" => {
                Err("pyannote-rs engine requires the 'diarize' feature. Rebuild with: cargo build --features diarize".into())
            }
            "pyannote" => diarize_with_pyannote(&effective_path_owned),
            other => {
                Err(format!("unknown diarization engine: {}", other).into())
            }
        };
        result.map_err(|e| e.to_string())
    });

    let result = match handle.join() {
        Ok(r) => Some(r),
        Err(_) => {
            tracing::error!("diarization thread panicked");
            None
        }
    };

    // Clean up preprocessed temp file
    if let Some(ref temp) = _temp_file {
        std::fs::remove_file(temp).ok();
    }

    match result {
        Some(Ok(result)) => {
            tracing::info!(
                speakers = result.num_speakers,
                segments = result.segments.len(),
                "diarization complete"
            );
            Some(result)
        }
        Some(Err(e)) => {
            tracing::error!(error = %e, "diarization failed, continuing without speaker labels");
            None
        }
        None => None,
    }
}

/// Apply diarization results to a transcript.
/// Replaces timestamp-only lines with speaker-labeled lines.
/// Segments are sorted by start time before matching.
pub fn apply_speakers(transcript: &str, result: &DiarizationResult) -> String {
    let mut output = String::new();

    // Sort segments by start time for deterministic matching
    let mut sorted_segments = result.segments.clone();
    sorted_segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut unknown_count = 0usize;
    let mut matched_count = 0usize;

    for line in transcript.lines() {
        // Parse timestamp from lines like "[0:00] text"
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let ts_str = &rest[..bracket_end];
                let text = rest[bracket_end + 1..].trim();

                if let Some(secs) = parse_timestamp(ts_str) {
                    let speaker = find_speaker(secs, &sorted_segments);
                    if speaker == "UNKNOWN" {
                        unknown_count += 1;
                    } else {
                        matched_count += 1;
                    }
                    output.push_str(&format!("[{} {}] {}\n", speaker, ts_str, text));
                    continue;
                }
            }
        }
        output.push_str(line);
        output.push('\n');
    }

    if unknown_count > 0 {
        tracing::warn!(
            unknown = unknown_count,
            matched = matched_count,
            "speaker attribution results — high unknown count may indicate timestamp/segment mismatch"
        );
    }

    output
}

/// Find which speaker is talking at a given timestamp.
/// Segments MUST be sorted by start time.
///
/// 1. Exact containment: timestamp falls within [start, end)
/// 2. Gap fallback (0.5s tolerance): if the timestamp falls in a small gap
///    between segments, prefer the *next* speaker (who is about to talk)
///    over the previous one (who just stopped). This matches how whisper
///    floors timestamps to segment boundaries.
/// 3. Beyond tolerance: return "UNKNOWN" — don't fabricate attribution
///    for timestamps in silence.
fn find_speaker(time_secs: f64, segments: &[SpeakerSegment]) -> &str {
    // Exact containment (binary search since segments are sorted)
    let idx = segments.partition_point(|seg| seg.end <= time_secs);
    if idx < segments.len() && time_secs >= segments[idx].start && time_secs < segments[idx].end {
        return &segments[idx].speaker;
    }

    // Gap fallback: check the surrounding segments within 0.5s tolerance.
    // Prefer the next segment (speaker about to talk) over the previous one.
    let tolerance = 0.5;

    // Next segment: idx (the one whose end is > time_secs)
    if idx < segments.len() {
        let gap = segments[idx].start - time_secs;
        if gap >= 0.0 && gap <= tolerance {
            return &segments[idx].speaker;
        }
    }

    // Previous segment
    if idx > 0 {
        let prev = &segments[idx - 1];
        let gap = time_secs - prev.end;
        if gap >= 0.0 && gap <= tolerance {
            return &prev.speaker;
        }
    }

    "UNKNOWN"
}

/// Parse a timestamp like "0:00" or "1:30" into seconds.
fn parse_timestamp(ts: &str) -> Option<f64> {
    let parts: Vec<&str> = ts.split(':').collect();
    match parts.len() {
        2 => {
            let mins: f64 = parts[0].parse().ok()?;
            let secs: f64 = parts[1].parse().ok()?;
            Some(mins * 60.0 + secs)
        }
        3 => {
            let hours: f64 = parts[0].parse().ok()?;
            let mins: f64 = parts[1].parse().ok()?;
            let secs: f64 = parts[2].parse().ok()?;
            Some(hours * 3600.0 + mins * 60.0 + secs)
        }
        _ => None,
    }
}

// ── Native diarization via pyannote-rs ──────────────────────

#[cfg(feature = "diarize")]
fn diarize_with_pyannote_rs(
    audio_path: &Path,
    config: &Config,
) -> Result<DiarizationResult, Box<dyn std::error::Error>> {
    use pyannote_rs::{EmbeddingExtractor, EmbeddingManager};

    let model_dir = &config.diarization.model_path;
    let seg_model = model_dir.join(SEGMENTATION_MODEL);
    let emb_info = embedding_model_for_config(config);
    let emb_model = model_dir.join(emb_info.filename);

    if !seg_model.exists() {
        return Err(format!(
            "Segmentation model not found at {}. Run `minutes setup --diarization` to download.",
            seg_model.display()
        )
        .into());
    }
    if !emb_model.exists() {
        return Err(format!(
            "Embedding model not found at {}. Run `minutes setup --diarization` to download.",
            emb_model.display()
        )
        .into());
    }

    // Load audio — pyannote-rs needs mono 16-bit PCM.
    // Use symphonia to decode any format, then convert to i16 samples.
    let (samples, sample_rate) = load_audio_as_i16(audio_path)?;

    tracing::info!(
        samples = samples.len(),
        sample_rate = sample_rate,
        "audio loaded for diarization"
    );

    // Step 1: Segment speech regions
    let segments_iter = pyannote_rs::get_segments(&samples, sample_rate, &seg_model)?;

    // Step 2: Extract speaker embeddings and cluster
    let mut extractor = EmbeddingExtractor::new(&emb_model)?;
    let mut manager = EmbeddingManager::new(usize::MAX);
    let threshold = config.diarization.threshold;

    let mut segments = Vec::new();
    let mut embedding_accum: std::collections::HashMap<String, (Vec<f32>, usize)> =
        std::collections::HashMap::new();

    for segment_result in segments_iter {
        let segment = segment_result?;
        let embedding: Vec<f32> = extractor.compute(&segment.samples)?.collect();

        let speaker_id = manager
            .search_speaker(embedding.clone(), threshold)
            .map(|id| id.to_string())
            .unwrap_or_else(|| "0".to_string());

        let label = format!("SPEAKER_{}", speaker_id);

        // Accumulate embeddings per speaker for averaging
        let entry = embedding_accum
            .entry(label.clone())
            .or_insert_with(|| (vec![0.0f32; embedding.len()], 0));
        for (i, val) in embedding.iter().enumerate() {
            entry.0[i] += val;
        }
        entry.1 += 1;

        segments.push(SpeakerSegment {
            speaker: label,
            start: segment.start,
            end: segment.end,
        });
    }

    let num_speakers = segments
        .iter()
        .map(|s| s.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    // Average embeddings per speaker
    let speaker_embeddings: std::collections::HashMap<String, Vec<f32>> = embedding_accum
        .into_iter()
        .map(|(label, (sum, count))| {
            let avg = sum.into_iter().map(|v| v / count as f32).collect();
            (label, avg)
        })
        .collect();

    Ok(DiarizationResult {
        segments,
        num_speakers,
        speaker_embeddings,
    })
}

/// Load audio file as mono 16-bit PCM samples using symphonia.
/// Handles WAV, M4A, MP3, OGG, and other formats symphonia supports.
#[cfg(feature = "diarize")]
fn load_audio_as_i16(audio_path: &Path) -> Result<(Vec<i16>, u32), Box<dyn std::error::Error>> {
    use symphonia::core::audio::SampleBuffer;
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(audio_path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = audio_path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;

    let track = format.default_track().ok_or("no audio track found")?;
    let track_id = track.id;
    let sample_rate = track.codec_params.sample_rate.ok_or("no sample rate")?;
    let channels = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);

    let mut decoder =
        symphonia::default::get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut all_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = decoder.decode(&packet)?;
        let spec = *decoded.spec();
        let num_frames = decoded.capacity();

        let mut sample_buf = SampleBuffer::<f32>::new(num_frames as u64, spec);
        sample_buf.copy_interleaved_ref(decoded);

        let samples = sample_buf.samples();

        // Mix to mono if multi-channel
        if channels > 1 {
            for chunk in samples.chunks(channels) {
                let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                all_samples.push(mono);
            }
        } else {
            all_samples.extend_from_slice(samples);
        }
    }

    // Convert f32 [-1.0, 1.0] to i16
    let i16_samples: Vec<i16> = all_samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * 32767.0) as i16
        })
        .collect();

    Ok((i16_samples, sample_rate))
}

// ── Legacy Python subprocess diarization ────────────────────

/// Run pyannote diarization via Python subprocess.
fn diarize_with_pyannote(
    audio_path: &Path,
) -> Result<DiarizationResult, Box<dyn std::error::Error>> {
    let python = find_python()?;

    // Security: pass audio path as sys.argv[1], never interpolate into source code.
    let script = r#"
import json, sys
try:
    from pyannote.audio import Pipeline
    pipeline = Pipeline.from_pretrained("pyannote/speaker-diarization-3.1",
                                         use_auth_token=False)
    diarization = pipeline(sys.argv[1])
    segments = []
    for turn, _, speaker in diarization.itertracks(yield_label=True):
        segments.append({"speaker": speaker, "start": turn.start, "end": turn.end})
    print(json.dumps(segments))
except ImportError:
    print("ERROR: pyannote.audio not installed. Run: pip install pyannote.audio", file=sys.stderr)
    sys.exit(1)
except Exception as e:
    print(f"ERROR: {e}", file=sys.stderr)
    sys.exit(1)
"#;

    let output = std::process::Command::new(&python)
        .args(["-c", script, audio_path.to_str().unwrap_or("")])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("pyannote failed: {}", stderr).into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let segments: Vec<SpeakerSegment> = serde_json::from_str(&stdout)?;

    let num_speakers = segments
        .iter()
        .map(|s| s.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    Ok(DiarizationResult {
        segments,
        num_speakers,
        speaker_embeddings: std::collections::HashMap::new(), // Python path can't extract embeddings
    })
}

/// Find the Python interpreter.
fn find_python() -> Result<String, Box<dyn std::error::Error>> {
    for candidate in &["python3", "python"] {
        let result = std::process::Command::new(candidate)
            .args(["--version"])
            .output();
        if let Ok(output) = result {
            if output.status.success() {
                return Ok(candidate.to_string());
            }
        }
    }
    Err("Python not found. Install Python 3 for speaker diarization.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_timestamp_minutes_seconds() {
        assert_eq!(parse_timestamp("0:00"), Some(0.0));
        assert_eq!(parse_timestamp("1:30"), Some(90.0));
        assert_eq!(parse_timestamp("10:05"), Some(605.0));
    }

    #[test]
    fn parse_timestamp_hours() {
        assert_eq!(parse_timestamp("1:00:00"), Some(3600.0));
    }

    #[test]
    fn parse_timestamp_invalid() {
        assert_eq!(parse_timestamp("abc"), None);
        assert_eq!(parse_timestamp(""), None);
    }

    #[test]
    fn find_speaker_returns_correct_label() {
        let segments = vec![
            SpeakerSegment {
                speaker: "SPEAKER_0".into(),
                start: 0.0,
                end: 5.0,
            },
            SpeakerSegment {
                speaker: "SPEAKER_1".into(),
                start: 5.0,
                end: 10.0,
            },
        ];

        assert_eq!(find_speaker(2.5, &segments), "SPEAKER_0");
        assert_eq!(find_speaker(7.0, &segments), "SPEAKER_1");
        assert_eq!(find_speaker(15.0, &segments), "UNKNOWN");
    }

    #[test]
    fn find_speaker_gap_fallback_prefers_next_speaker() {
        // Segments with gaps — sorted by start time (as apply_speakers provides)
        let segments = vec![
            SpeakerSegment {
                speaker: "SPEAKER_0".into(),
                start: 0.045,
                end: 3.98,
            },
            SpeakerSegment {
                speaker: "SPEAKER_1".into(),
                start: 4.12,
                end: 8.5,
            },
        ];

        // Timestamp 0.0 falls 0.045s before first segment — within 0.5s tolerance
        assert_eq!(find_speaker(0.0, &segments), "SPEAKER_0");
        // Timestamp 4.0 falls in gap: 0.02s from A end, 0.12s from B start
        // Prefer next speaker (B) — they're about to talk
        assert_eq!(find_speaker(4.0, &segments), "SPEAKER_1");
        // Timestamp 8.6 is 0.1s past segment B — within 0.5s tolerance
        assert_eq!(find_speaker(8.6, &segments), "SPEAKER_1");
        // Timestamp 10.0 is 1.5s past segment B — beyond 0.5s tolerance
        assert_eq!(find_speaker(10.0, &segments), "UNKNOWN");
        // Timestamp 15.0 is far from any segment
        assert_eq!(find_speaker(15.0, &segments), "UNKNOWN");
    }

    #[test]
    fn find_speaker_silence_stays_unknown() {
        // Long silence gap between speakers — should NOT fabricate attribution
        let segments = vec![
            SpeakerSegment {
                speaker: "SPEAKER_0".into(),
                start: 0.0,
                end: 5.0,
            },
            SpeakerSegment {
                speaker: "SPEAKER_1".into(),
                start: 10.0,
                end: 15.0,
            },
        ];

        // Timestamp 7.0 is 2s from both segments — beyond tolerance
        assert_eq!(find_speaker(7.0, &segments), "UNKNOWN");
    }

    #[test]
    fn apply_speakers_labels_transcript() {
        let transcript = "[0:00] Hello everyone\n[0:05] Thanks for joining\n";
        let result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 3.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 3.0,
                    end: 10.0,
                },
            ],
            num_speakers: 2,
            speaker_embeddings: std::collections::HashMap::new(),
        };

        let labeled = apply_speakers(transcript, &result);
        assert!(labeled.contains("[SPEAKER_0 0:00]"));
        assert!(labeled.contains("[SPEAKER_1 0:05]"));
    }

    #[test]
    fn diarize_returns_none_when_disabled() {
        let config = Config::default(); // engine = "none"
        let result = diarize(Path::new("/fake.wav"), &config);
        assert!(result.is_none());
    }

    #[test]
    fn apply_confirmed_names_rewrites_high_confidence() {
        let transcript = "[SPEAKER_1 0:00] Hello\n[SPEAKER_2 0:05] Hi there\n";
        let attributions = vec![
            SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Mat".into(),
                confidence: Confidence::High,
                source: AttributionSource::Manual,
            },
            SpeakerAttribution {
                speaker_label: "SPEAKER_2".into(),
                name: "Alex".into(),
                confidence: Confidence::Medium,
                source: AttributionSource::Deterministic,
            },
        ];
        let result = apply_confirmed_names(transcript, &attributions);
        assert!(result.contains("[Mat 0:00]"));
        assert!(result.contains("[SPEAKER_2 0:05]"));
    }

    #[test]
    fn apply_confirmed_names_no_high_is_noop() {
        let transcript = "[SPEAKER_1 0:00] Hello\n";
        let result = apply_confirmed_names(
            transcript,
            &[SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Mat".into(),
                confidence: Confidence::Medium,
                source: AttributionSource::Deterministic,
            }],
        );
        assert_eq!(result, transcript);
    }

    #[test]
    fn speaker_attribution_roundtrips_yaml() {
        let attr = SpeakerAttribution {
            speaker_label: "SPEAKER_2".into(),
            name: "Sarah".into(),
            confidence: Confidence::High,
            source: AttributionSource::Manual,
        };
        let yaml = serde_yaml::to_string(&attr).unwrap();
        let parsed: SpeakerAttribution = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.confidence, Confidence::High);
        assert_eq!(parsed.source, AttributionSource::Manual);
    }

    #[test]
    fn diarize_returns_none_for_unknown_engine() {
        let mut config = Config::default();
        config.diarization.engine = "nonexistent".into();
        let result = diarize(Path::new("/fake.wav"), &config);
        assert!(result.is_none());
    }

    #[test]
    fn models_installed_returns_false_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.diarization.model_path = dir.path().join("missing-models");
        assert!(!models_installed(&config));
    }

    #[test]
    fn config_recognizes_pyannote_rs_engine() {
        let mut config = Config::default();
        config.diarization.engine = "pyannote-rs".into();
        assert_eq!(config.diarization.engine, "pyannote-rs");
        assert_eq!(config.diarization.threshold, 0.5);
    }
}
