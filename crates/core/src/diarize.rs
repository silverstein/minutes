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
    /// Whether transcript attribution should use the wider stem-timing tolerance.
    pub from_stems: bool,
    /// Whether the result came from source-aware capture and still has a stable
    /// local-vs-remote distinction available to downstream attribution.
    pub source_aware: bool,
    /// Per-speaker averaged embeddings (for Level 3 confirmed learning).
    /// Empty when using the Python subprocess engine.
    pub speaker_embeddings: std::collections::HashMap<String, Vec<f32>>,
}

type EnergyWindow = (f64, f32);
type StemEnergyWindows = (Vec<EnergyWindow>, Vec<EnergyWindow>);

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
                    let text = rest[bracket_end + 1..].trim();
                    if let Some(name) = high_map.get(label) {
                        if !is_non_lexical_event_text(text) {
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
        }
        if !replaced {
            output.push_str(line);
            output.push('\n');
        }
    }
    output
}

fn is_non_lexical_event_text(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with('[') && trimmed.ends_with(']')
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

fn read_stem_energy_windows(
    stems: &StemPaths,
    window_secs: f64,
) -> Result<StemEnergyWindows, String> {
    let voice_energy = compute_energy_windows(&stems.voice, window_secs)
        .map_err(|error| format!("failed to read voice stem: {error}"))?;
    let system_energy = compute_energy_windows(&stems.system, window_secs)
        .map_err(|error| format!("failed to read system stem: {error}"))?;
    Ok((voice_energy, system_energy))
}

fn correlation_coefficient(xs: &[f32], ys: &[f32]) -> Option<f32> {
    if xs.len() != ys.len() || xs.len() < 2 {
        return None;
    }

    let n = xs.len() as f64;
    let mean_x = xs.iter().map(|&x| x as f64).sum::<f64>() / n;
    let mean_y = ys.iter().map(|&y| y as f64).sum::<f64>() / n;

    let mut num = 0.0;
    let mut den_x = 0.0;
    let mut den_y = 0.0;
    for (&x, &y) in xs.iter().zip(ys.iter()) {
        let dx = x as f64 - mean_x;
        let dy = y as f64 - mean_y;
        num += dx * dy;
        den_x += dx * dx;
        den_y += dy * dy;
    }

    let denom = (den_x * den_y).sqrt();
    if denom <= f64::EPSILON {
        None
    } else {
        Some((num / denom) as f32)
    }
}

fn merge_or_push_segment(segments: &mut Vec<SpeakerSegment>, speaker: &str, start: f64, end: f64) {
    if let Some(last) = segments.last_mut() {
        if last.speaker == speaker && (start - last.end).abs() < 0.01 {
            last.end = end;
            return;
        }
    }

    segments.push(SpeakerSegment {
        speaker: speaker.to_string(),
        start,
        end,
    });
}

fn collapse_to_single_speaker_segments(
    voice_energy: &[(f64, f32)],
    system_energy: &[(f64, f32)],
    window_secs: f64,
    silence_threshold: f32,
    speaker_label: &str,
) -> Vec<SpeakerSegment> {
    let mut segments = Vec::new();
    let window_count = voice_energy.len().min(system_energy.len());

    for i in 0..window_count {
        let (start, voice_rms) = voice_energy[i];
        let (_, system_rms) = system_energy[i];
        let end = start + window_secs;
        let voice_active = voice_rms > silence_threshold;
        let system_active = system_rms > silence_threshold;

        if voice_active || system_active {
            merge_or_push_segment(&mut segments, speaker_label, start, end);
        }
    }

    segments
}

fn maybe_relabel_single_call_speaker_to_voice(
    segments: &mut [SpeakerSegment],
    voice_values: &[f32],
    system_values: &[f32],
    silence_threshold: f32,
    stem_correlation_threshold: f32,
) {
    if segments.len() != 1 || segments[0].speaker != "SPEAKER_1" {
        return;
    }

    let active_voice_windows = voice_values
        .iter()
        .filter(|&&rms| rms > silence_threshold)
        .count();
    let active_voice_ratio = active_voice_windows as f32 / voice_values.len().max(1) as f32;
    let correlated = correlation_coefficient(voice_values, system_values)
        .is_some_and(|value| value >= stem_correlation_threshold);

    // If the microphone stem is active for most of the recording, this is
    // likely the local speaker bleeding into the system stem rather than a
    // true remote-only single speaker, but only when the two stems also move
    // together strongly. Mere mic-side noise should not relabel remote audio
    // as the local speaker.
    //
    // Shares stem_correlation_threshold with the primary collapse path.
    // Raising the threshold (e.g. to 1.0) disables both correlation-driven
    // collapses, which is what open-speaker-mic users need (issue #157).
    if active_voice_ratio >= 0.6 && correlated {
        segments[0].speaker = "SPEAKER_0".into();
    }
}

fn diarization_from_energy_windows(
    voice_energy: &[(f64, f32)],
    system_energy: &[(f64, f32)],
    window_secs: f64,
    stem_correlation_threshold: f32,
) -> Option<DiarizationResult> {
    // Energy threshold: below this RMS, the source is considered silent.
    // Typical speech RMS is 0.01-0.1; noise floor is <0.001.
    let silence_threshold = 0.005_f32;

    let voice_label = "SPEAKER_0";
    let call_label = "SPEAKER_1";
    let window_count = voice_energy.len().min(system_energy.len());

    let voice_values: Vec<f32> = voice_energy
        .iter()
        .take(window_count)
        .map(|(_, rms)| *rms)
        .collect();
    let system_values: Vec<f32> = system_energy
        .iter()
        .take(window_count)
        .map(|(_, rms)| *rms)
        .collect();
    let active_windows = voice_values
        .iter()
        .zip(system_values.iter())
        .filter(|(voice_rms, system_rms)| {
            **voice_rms > silence_threshold || **system_rms > silence_threshold
        })
        .count();
    let correlation = correlation_coefficient(&voice_values, &system_values);

    // When both stems move together for most windows, we're likely seeing the
    // same person bleeding into both sources (for example your own voice plus
    // system echo / self-monitor). Treat that as one human, not two speakers.
    //
    // This heuristic misfires for open-speaker mic setups where the mic
    // acoustically picks up multi-speaker system audio. Users hitting that
    // case can raise stem_correlation_threshold (config: diarization section)
    // to 1.0 or higher to disable the collapse.
    if active_windows >= 3 && correlation.is_some_and(|value| value >= stem_correlation_threshold) {
        let segments = collapse_to_single_speaker_segments(
            voice_energy,
            system_energy,
            window_secs,
            silence_threshold,
            voice_label,
        );
        if segments.is_empty() {
            return None;
        }

        tracing::info!(
            active_windows,
            correlation = correlation,
            threshold = stem_correlation_threshold,
            "stem energies strongly correlated — collapsing to one speaker"
        );

        return Some(DiarizationResult {
            segments,
            num_speakers: 1,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        });
    }

    let mut segments: Vec<SpeakerSegment> = Vec::new();

    for i in 0..window_count {
        let (start, voice_rms) = voice_energy[i];
        let (_, system_rms) = system_energy[i];
        let end = start + window_secs;

        let voice_active = voice_rms > silence_threshold;
        let system_active = system_rms > silence_threshold;

        let speaker = if voice_active && !system_active {
            voice_label
        } else if system_active && !voice_active {
            call_label
        } else if voice_active && system_active {
            if voice_rms >= system_rms {
                voice_label
            } else {
                call_label
            }
        } else {
            continue;
        };

        merge_or_push_segment(&mut segments, speaker, start, end);
    }

    let num_speakers = segments
        .iter()
        .map(|s| s.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    if num_speakers == 1 {
        maybe_relabel_single_call_speaker_to_voice(
            &mut segments,
            &voice_values,
            &system_values,
            silence_threshold,
            stem_correlation_threshold,
        );
    }

    if segments.is_empty() {
        None
    } else {
        let num_speakers = segments
            .iter()
            .map(|s| s.speaker.as_str())
            .collect::<std::collections::HashSet<_>>()
            .len();
        Some(DiarizationResult {
            segments,
            num_speakers,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        })
    }
}

/// Speaker attribution from per-source audio stems (no ML diarization).
/// Compares energy levels between voice and system stems per time window,
/// assigning "SPEAKER_0" (you) or "SPEAKER_1" (remote) to each window.
pub fn diarize_from_stems(stems: &StemPaths, config: &Config) -> Option<DiarizationResult> {
    let window_secs = 1.0; // 1-second energy windows

    let (voice_energy, system_energy) = match read_stem_energy_windows(stems, window_secs) {
        Ok(energies) => energies,
        Err(error) => {
            tracing::warn!(error = %error, "failed to read source-aware stems, falling back to ML diarization");
            return None;
        }
    };

    let stem_correlation_threshold = config.diarization.stem_correlation_threshold;
    let Some(result) = diarization_from_energy_windows(
        &voice_energy,
        &system_energy,
        window_secs,
        stem_correlation_threshold,
    ) else {
        tracing::warn!("stem-based diarization produced no segments (all silent), falling back");
        return None;
    };

    tracing::info!(
        speakers = result.num_speakers,
        segments = result.segments.len(),
        voice_stem = %stems.voice.display(),
        system_stem = %stems.system.display(),
        "stem-based diarization complete"
    );

    Some(result)
}

fn resolve_diarization_engine(config: &Config) -> Option<&str> {
    match config.diarization.engine.as_str() {
        "none" => None,
        "auto" => {
            if models_installed(config) {
                tracing::info!("diarization models found — auto-enabling pyannote-rs");
                Some("pyannote-rs")
            } else {
                tracing::debug!(
                    "diarization models not found — skipping (run `minutes setup --diarization` to enable)"
                );
                None
            }
        }
        other => Some(other),
    }
}

fn run_diarization_engine(
    audio_path: &Path,
    config: &Config,
    resolved_engine: &str,
) -> Option<DiarizationResult> {
    tracing::info!(
        engine = %resolved_engine,
        file = %audio_path.display(),
        "running diarization"
    );

    // Pre-process: resample to 16kHz mono via ffmpeg if available.
    // pyannote-rs/symphonia can struggle with 44.1kHz F32 WAVs from live capture.
    // This matches how transcribe.rs preprocesses audio for whisper.
    let (effective_path, temp_file) = preprocess_audio(audio_path);

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
            other => Err(format!("unknown diarization engine: {}", other).into()),
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
    if let Some(ref temp) = temp_file {
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

fn remap_diarization_labels(
    result: &DiarizationResult,
    starting_label: usize,
) -> DiarizationResult {
    let mut label_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut next_label = starting_label;

    let mut remap_label = |raw: &str| {
        label_map
            .entry(raw.to_string())
            .or_insert_with(|| {
                let label = format!("SPEAKER_{}", next_label);
                next_label += 1;
                label
            })
            .clone()
    };

    let segments = result
        .segments
        .iter()
        .map(|segment| SpeakerSegment {
            speaker: remap_label(&segment.speaker),
            start: segment.start,
            end: segment.end,
        })
        .collect();

    let mut embedding_keys: Vec<String> = result.speaker_embeddings.keys().cloned().collect();
    embedding_keys.sort();

    let mut speaker_embeddings = std::collections::HashMap::new();
    for raw_label in embedding_keys {
        let remapped_label = remap_label(&raw_label);
        if let Some(embedding) = result.speaker_embeddings.get(&raw_label) {
            speaker_embeddings.insert(remapped_label, embedding.clone());
        }
    }

    DiarizationResult {
        segments,
        num_speakers: label_map.len(),
        from_stems: result.from_stems,
        source_aware: result.source_aware,
        speaker_embeddings,
    }
}

fn merge_remote_diarization_into_stem_result(
    stem_result: &DiarizationResult,
    remote_result: &DiarizationResult,
) -> DiarizationResult {
    let mut base_segments = stem_result.segments.clone();
    base_segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut remote_segments = remote_result.segments.clone();
    remote_segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut merged = Vec::new();
    let mut remote_cursor = 0usize;

    for segment in base_segments {
        if segment.speaker != "SPEAKER_1" {
            merge_or_push_segment(&mut merged, &segment.speaker, segment.start, segment.end);
            continue;
        }

        while remote_cursor < remote_segments.len()
            && remote_segments[remote_cursor].end <= segment.start
        {
            remote_cursor += 1;
        }

        let mut idx = remote_cursor;
        let mut cursor = segment.start;
        while idx < remote_segments.len() && remote_segments[idx].start < segment.end {
            let remote = &remote_segments[idx];
            let start = segment.start.max(remote.start).max(cursor);
            let end = segment.end.min(remote.end);
            if start > cursor {
                merge_or_push_segment(&mut merged, "SPEAKER_1", cursor, start);
            }
            if end > start {
                merge_or_push_segment(&mut merged, &remote.speaker, start, end);
                cursor = end;
            }
            idx += 1;
        }

        if cursor < segment.end {
            merge_or_push_segment(&mut merged, "SPEAKER_1", cursor, segment.end);
        }
    }

    let present_labels: std::collections::HashSet<String> = merged
        .iter()
        .map(|segment| segment.speaker.clone())
        .collect();
    let speaker_embeddings = remote_result
        .speaker_embeddings
        .iter()
        .filter(|(label, _)| present_labels.contains(*label))
        .map(|(label, embedding)| (label.clone(), embedding.clone()))
        .collect();

    DiarizationResult {
        num_speakers: present_labels.len(),
        segments: merged,
        from_stems: false,
        source_aware: true,
        speaker_embeddings,
    }
}

fn meaningful_speaker_count_excluding(result: &DiarizationResult, ignored: &[&str]) -> usize {
    let mut speaker_durations: std::collections::HashMap<&str, f64> =
        std::collections::HashMap::new();
    for segment in &result.segments {
        if ignored.contains(&segment.speaker.as_str()) {
            continue;
        }

        let duration = (segment.end - segment.start).max(0.0);
        if duration > 0.0 {
            *speaker_durations
                .entry(segment.speaker.as_str())
                .or_insert(0.0) += duration;
        }
    }

    speaker_durations
        .values()
        .filter(|&&duration| duration >= 0.5)
        .count()
}

fn has_meaningful_remote_structure(result: &DiarizationResult) -> bool {
    meaningful_speaker_count_excluding(result, &["SPEAKER_0"]) >= 1
}

fn has_meaningful_system_stem_labels(result: &DiarizationResult) -> bool {
    meaningful_speaker_count_excluding(result, &["SPEAKER_0", "SPEAKER_1"]) >= 1
}

fn diarize_from_source_aware_stems(
    stems: &StemPaths,
    config: &Config,
    resolved_engine: Option<&str>,
) -> Option<DiarizationResult> {
    let window_secs = 1.0;
    let (voice_energy, system_energy) = match read_stem_energy_windows(stems, window_secs) {
        Ok(energies) => energies,
        Err(error) => {
            tracing::warn!(error = %error, "failed to read source-aware stems, falling back to ML diarization");
            return None;
        }
    };

    let stem_result = diarization_from_energy_windows(
        &voice_energy,
        &system_energy,
        window_secs,
        config.diarization.stem_correlation_threshold,
    )?;
    let local_only_collapse = stem_result.num_speakers == 1
        && !stem_result.segments.is_empty()
        && stem_result
            .segments
            .iter()
            .all(|segment| segment.speaker == "SPEAKER_0");
    let non_collapsed_stem_result =
        diarization_from_energy_windows(&voice_energy, &system_energy, window_secs, 2.0);

    let Some(resolved_engine) = resolved_engine else {
        return Some(stem_result);
    };

    let Some(remote_result) = run_diarization_engine(&stems.system, config, resolved_engine) else {
        tracing::warn!(
            system_stem = %stems.system.display(),
            "system-stem diarization failed, keeping stem-only attribution"
        );
        return Some(stem_result);
    };

    let remapped_remote = remap_diarization_labels(&remote_result, 2);
    if !has_meaningful_remote_structure(&remapped_remote) {
        tracing::info!(
            remote_speakers = remapped_remote.num_speakers,
            "system-stem diarization did not find stable remote structure, keeping stem-only attribution"
        );
        return Some(stem_result);
    }

    let merge_base = if local_only_collapse {
        non_collapsed_stem_result.as_ref().unwrap_or(&stem_result)
    } else {
        &stem_result
    };
    let merged = merge_remote_diarization_into_stem_result(merge_base, &remapped_remote);

    if !has_meaningful_system_stem_labels(&merged) {
        tracing::info!(
            stem_speakers = stem_result.num_speakers,
            merged_speakers = merged.num_speakers,
            "system-stem diarization did not contribute stable remote speaker labels, keeping stem-only attribution"
        );
        return Some(stem_result);
    }

    tracing::info!(
        stem_speakers = stem_result.num_speakers,
        merged_speakers = merged.num_speakers,
        "hybrid source-aware diarization complete"
    );

    Some(merged)
}

/// Run speaker diarization on an audio file.
/// Returns None if diarization is disabled or models are not available.
///
/// When per-source stems are available alongside the audio file,
/// prefers source-aware attribution and, when available, uses ML diarization
/// on the system stem to split remote participants without overriding local
/// voice-stem ownership.
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

    let resolved_engine = resolve_diarization_engine(config);

    // Check for per-source stems alongside the audio file.
    // If stems exist, prefer source-aware attribution and opportunistically
    // refine remote/system windows with ML diarization.
    if let Some(stems) = discover_stems(audio_path) {
        if let Some(result) = diarize_from_source_aware_stems(&stems, config, resolved_engine) {
            return Some(result);
        }
        // Stem attribution failed, fall through to ML diarization
        tracing::warn!("source-aware stem diarization failed, falling back to ML engine");
    }

    let resolved_engine = resolved_engine?;
    run_diarization_engine(audio_path, config, resolved_engine)
}

/// Apply diarization results to a transcript.
/// Replaces timestamp-only lines with speaker-labeled lines.
/// Segments are sorted by start time before matching.
pub fn apply_speakers(transcript: &str, result: &DiarizationResult) -> String {
    // Sort segments by start time for deterministic matching
    let mut sorted_segments = result.segments.clone();
    sorted_segments.sort_by(|a, b| {
        a.start
            .partial_cmp(&b.start)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    enum OutputLine {
        Attributed {
            speaker: String,
            ts_str: String,
            text: String,
        },
        Raw(String),
    }

    let mut lines: Vec<OutputLine> = Vec::new();
    let mut unknown_count = 0usize;
    let mut matched_count = 0usize;

    for line in transcript.lines() {
        // Parse timestamp from lines like "[0:00] text"
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let ts_str = &rest[..bracket_end];
                let text = rest[bracket_end + 1..].trim();

                if let Some(secs) = parse_timestamp(ts_str) {
                    let speaker =
                        find_speaker(secs, &sorted_segments, result.from_stems).to_string();
                    if speaker == "UNKNOWN" {
                        unknown_count += 1;
                    } else {
                        matched_count += 1;
                    }
                    lines.push(OutputLine::Attributed {
                        speaker,
                        ts_str: ts_str.to_string(),
                        text: text.to_string(),
                    });
                    continue;
                }
            }
        }
        lines.push(OutputLine::Raw(line.to_string()));
    }

    let dominant_speaker = dominant_speaker_label(&sorted_segments);

    // Hypothesis: Whisper often starts transcribing at t=0 while diarization
    // detects voice activity slightly later (VAD onset latency, mic warmup, or
    // leading silence). The first transcript segment therefore lands before the
    // first diarization segment, outside the 0.5s gap tolerance, and gets
    // labeled UNKNOWN. Since the opening words almost certainly belong to
    // whoever is about to speak, we inherit the speaker from the next
    // attributed segment rather than leaving it unresolved.
    let first_attr = lines
        .iter()
        .position(|l| matches!(l, OutputLine::Attributed { .. }));
    if let Some(first_idx) = first_attr {
        let is_unknown = matches!(&lines[first_idx], OutputLine::Attributed { speaker, .. } if speaker == "UNKNOWN");
        if is_unknown {
            let next_speaker = lines[first_idx + 1..].iter().find_map(|l| match l {
                OutputLine::Attributed { speaker, .. } if speaker != "UNKNOWN" => {
                    Some(speaker.clone())
                }
                _ => None,
            });
            if let Some(resolved) = next_speaker {
                if let OutputLine::Attributed { speaker, .. } = &mut lines[first_idx] {
                    *speaker = resolved;
                    unknown_count = unknown_count.saturating_sub(1);
                    matched_count += 1;
                }
            }
        }
    }

    // If every attributed line is still UNKNOWN but the diarization result has
    // one clearly dominant speaker, prefer that speaker over leaving the whole
    // clip unresolved. This is especially useful for short native-call clips
    // where the first transcript line starts before the first diarization
    // segment, but one speaker still dominates the clip overall.
    let all_unknown = !lines.is_empty()
        && lines.iter().all(|line| match line {
            OutputLine::Attributed { speaker, .. } => speaker == "UNKNOWN",
            OutputLine::Raw(_) => true,
        });
    if all_unknown {
        if let Some(dominant) = dominant_speaker {
            for line in &mut lines {
                if let OutputLine::Attributed { speaker, .. } = line {
                    if speaker == "UNKNOWN" {
                        *speaker = dominant.clone();
                        unknown_count = unknown_count.saturating_sub(1);
                        matched_count += 1;
                    }
                }
            }
        }
    }

    let mut output = String::new();
    for line in &lines {
        match line {
            OutputLine::Attributed {
                speaker,
                ts_str,
                text,
            } => {
                output.push_str(&format!("[{} {}] {}\n", speaker, ts_str, text));
            }
            OutputLine::Raw(raw) => {
                output.push_str(raw);
                output.push('\n');
            }
        }
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

fn dominant_speaker_label(segments: &[SpeakerSegment]) -> Option<String> {
    let mut durations: std::collections::HashMap<&str, f64> = std::collections::HashMap::new();
    for seg in segments {
        let dur = (seg.end - seg.start).max(0.0);
        *durations.entry(seg.speaker.as_str()).or_insert(0.0) += dur;
    }

    let total: f64 = durations.values().sum();
    if total <= f64::EPSILON {
        return None;
    }

    let (label, duration) = durations
        .into_iter()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

    // Require a strong majority before overriding UNKNOWN lines. This avoids
    // inventing certainty when the clip is genuinely mixed.
    if duration / total >= 0.6 {
        Some(label.to_string())
    } else {
        None
    }
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
fn find_speaker(time_secs: f64, segments: &[SpeakerSegment], from_stems: bool) -> &str {
    // Exact containment (binary search since segments are sorted)
    let idx = segments.partition_point(|seg| seg.end <= time_secs);
    if idx < segments.len() && time_secs >= segments[idx].start && time_secs < segments[idx].end {
        return &segments[idx].speaker;
    }

    // Gap fallback: check the surrounding segments within 0.5s tolerance.
    // Prefer the next segment (speaker about to talk) over the previous one.
    let next_tolerance = if from_stems { 2.0 } else { 0.5 };
    let prev_tolerance = 0.5;

    // Next segment: idx (the one whose end is > time_secs)
    if idx < segments.len() {
        let gap = segments[idx].start - time_secs;
        if gap >= 0.0 && gap <= next_tolerance {
            return &segments[idx].speaker;
        }
    }

    // Previous segment
    if idx > 0 {
        let prev = &segments[idx - 1];
        let gap = time_secs - prev.end;
        if gap >= 0.0 && gap <= prev_tolerance {
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
    use pyannote_rs::EmbeddingExtractor;

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

    let (mut f32_samples, mut i16_samples, sample_rate) = load_audio(audio_path)?;

    tracing::info!(
        f32_samples = f32_samples.len(),
        i16_samples = i16_samples.len(),
        sample_rate = sample_rate,
        "audio loaded for diarization"
    );

    // Step 1: Segment speech using the ONNX model directly with properly
    // normalized f32 input. We bypass pyannote_rs::get_segments because it
    // casts i16 to f32 without dividing by 32768, feeding the model values
    // in [-32768, 32767] when it expects [-1.0, 1.0].
    let mut speech_segments = segment_speech(&f32_samples, sample_rate, &seg_model)?;

    // If the model found no speech, the audio may be too quiet (e.g. MacBook
    // built-in mic with peaks as low as 0.0004). Normalize to a usable level
    // and retry — this avoids hardcoding a sensitivity threshold.
    if speech_segments.is_empty() {
        let peak = f32_samples.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        const TARGET_PEAK: f32 = 0.5;

        if peak > 0.0 && peak < TARGET_PEAK {
            let gain = TARGET_PEAK / peak;
            tracing::info!(
                peak = format!("{:.6}", peak),
                gain = format!("{:.1}x", gain),
                "no speech detected — retrying with normalized audio"
            );
            for s in &mut f32_samples {
                *s = (*s * gain).clamp(-1.0, 1.0);
            }
            i16_samples = f32_samples
                .iter()
                .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                .collect();

            speech_segments = segment_speech(&f32_samples, sample_rate, &seg_model)?;
        }
    }

    tracing::info!(
        segments = speech_segments.len(),
        "speech segmentation complete"
    );

    // Step 2: Extract speaker embeddings and cluster.
    //
    // We replace pyannote-rs's EmbeddingManager with our own clustering
    // that maintains running-average speaker templates. EmbeddingManager
    // only stores the first segment's embedding per speaker, which causes
    // over-segmentation (one person → multiple speakers) as the reference
    // embedding becomes unrepresentative over time.
    let mut extractor = EmbeddingExtractor::new(&emb_model)?;
    let threshold = config.diarization.threshold;

    // Merge adjacent speech segments that are separated by short gaps.
    // The segmentation model often splits continuous speech into many
    // tiny fragments; merging them produces longer, more stable segments
    // for embedding extraction.
    let speech_segments = merge_short_segments(speech_segments, sample_rate);

    tracing::info!(
        segments = speech_segments.len(),
        "speech segments after merge"
    );

    // speaker_templates[i] = (running average embedding, segment count)
    let mut speaker_templates: Vec<(Vec<f32>, usize)> = Vec::new();
    // Per-segment: which speaker index was assigned
    let mut seg_speaker_ids: Vec<usize> = Vec::new();

    // Minimum samples for reliable embedding extraction (~1.5s at 16kHz).
    // Shorter segments produce unstable embeddings that corrupt clustering.
    let min_embed_samples = (sample_rate as f64 * 1.5) as usize;

    for seg in &speech_segments {
        let seg_i16 = &i16_samples[seg.start_sample..seg.end_sample];

        // Skip too-short segments for clustering; they still appear in the
        // transcript but inherit the nearest speaker label.
        if seg_i16.len() < min_embed_samples {
            seg_speaker_ids.push(usize::MAX); // sentinel: inherit later
            continue;
        }

        let raw_embedding: Vec<f32> = extractor.compute(seg_i16)?.collect();

        // L2-normalize so every segment contributes equally to the
        // average direction, regardless of the model's output magnitude.
        let embedding = l2_normalize(&raw_embedding);

        // Find best matching speaker by cosine similarity
        let mut best_id = None;
        let mut best_sim = threshold;
        for (id, (template, _)) in speaker_templates.iter().enumerate() {
            let sim = crate::voice::cosine_similarity(&embedding, template);
            if sim > best_sim {
                best_sim = sim;
                best_id = Some(id);
            }
        }

        let speaker_id = match best_id {
            Some(id) => {
                let (ref mut template, ref mut count) = speaker_templates[id];
                let old_count = *count as f32;
                let new_count = old_count + 1.0;
                for (i, val) in embedding.iter().enumerate() {
                    template[i] = (template[i] * old_count + val) / new_count;
                }
                *count += 1;
                id
            }
            None => {
                let id = speaker_templates.len();
                speaker_templates.push((embedding, 1));
                id
            }
        };

        seg_speaker_ids.push(speaker_id);
    }

    // Merge pass: if two speaker templates are similar enough, merge them.
    // This catches cases where early segments created separate speakers
    // that converged as more data came in.
    //
    // The merge threshold is set to max(threshold - 0.05, 0.3) to avoid
    // merging genuinely different speakers. The 0.3 floor prevents overly
    // aggressive merging when the user sets a low diarization threshold.
    let merge_threshold = (threshold - 0.05).max(0.3);
    let num_templates = speaker_templates.len();
    let mut merge_map: Vec<usize> = (0..num_templates).collect();

    for i in 0..num_templates {
        for j in (i + 1)..num_templates {
            if merge_map[j] != j {
                continue; // already merged
            }
            let ri = merge_map[i]; // canonical id for i
            let sim =
                crate::voice::cosine_similarity(&speaker_templates[ri].0, &speaker_templates[j].0);
            if sim > merge_threshold {
                tracing::info!(
                    from = j,
                    into = ri,
                    similarity = format!("{:.4}", sim),
                    "merging speaker clusters"
                );
                merge_map[j] = ri;
            }
        }
    }

    // Resolve transitive merges (e.g. 3→2→1 becomes 3→1, 2→1).
    // Loop bound prevents infinite loops if merge_map is ever inconsistent.
    for i in 0..num_templates {
        let mut root = merge_map[i];
        let mut steps = 0;
        while merge_map[root] != root && steps < num_templates {
            root = merge_map[root];
            steps += 1;
        }
        merge_map[i] = root;
    }

    // Assign compact labels (SPEAKER_1, SPEAKER_2, ...) to canonical IDs
    let mut canonical_to_label: std::collections::HashMap<usize, String> =
        std::collections::HashMap::new();
    let mut next_label = 1usize;
    for &canonical in &merge_map {
        canonical_to_label.entry(canonical).or_insert_with(|| {
            let label = format!("SPEAKER_{}", next_label);
            next_label += 1;
            label
        });
    }

    // Build segments with merged labels.
    // Segments that were too short for embedding extraction (sentinel usize::MAX)
    // inherit the label of the nearest non-skipped segment.
    let mut segments = Vec::new();

    // First pass: resolve labels for non-skipped segments
    let resolved_labels: Vec<Option<String>> = seg_speaker_ids
        .iter()
        .map(|&raw_id| {
            if raw_id == usize::MAX {
                None
            } else {
                let canonical_id = merge_map[raw_id];
                Some(canonical_to_label[&canonical_id].clone())
            }
        })
        .collect();

    // Forward pass: fill skipped segments by inheriting from the nearest
    // *temporal* neighbor (not acoustic). A short segment between two different
    // speakers gets the label of whichever speaker was most recent, not
    // whichever it sounds like. This is an acceptable tradeoff: extracting
    // embeddings from <1.5s segments produces unreliable results, and temporal
    // proximity is a reasonable heuristic for meeting-style audio.
    let mut last_known_label: Option<String> = None;
    let mut final_labels: Vec<String> = Vec::with_capacity(resolved_labels.len());
    for label in &resolved_labels {
        if let Some(l) = label {
            last_known_label = Some(l.clone());
        }
        final_labels.push(last_known_label.clone().unwrap_or_else(|| "UNKNOWN".into()));
    }
    // Backward pass: fix leading skipped segments (before any known label)
    if let Some(first_known) = resolved_labels.iter().find_map(|l| l.as_ref()) {
        for label in &mut final_labels {
            if label == "UNKNOWN" {
                *label = first_known.clone();
            } else {
                break;
            }
        }
    }

    for (idx, seg) in speech_segments.iter().enumerate() {
        segments.push(SpeakerSegment {
            speaker: final_labels[idx].clone(),
            start: seg.start,
            end: seg.end,
        });
    }

    // Rebuild final speaker embeddings by weighted-averaging merged templates.
    // Each template is weighted by its segment count so a template built from
    // 50 segments contributes proportionally more than one from 2 segments.
    let mut speaker_embeddings: std::collections::HashMap<String, Vec<f32>> =
        std::collections::HashMap::new();
    let mut speaker_total_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for (raw_id, (template, count)) in speaker_templates.iter().enumerate() {
        let canonical_id = merge_map[raw_id];
        let label = canonical_to_label[&canonical_id].clone();
        let entry = speaker_embeddings
            .entry(label.clone())
            .or_insert_with(|| vec![0.0f32; template.len()]);
        for (i, val) in template.iter().enumerate() {
            entry[i] += val * (*count as f32);
        }
        *speaker_total_counts.entry(label).or_insert(0) += count;
    }
    for (label, embedding) in speaker_embeddings.iter_mut() {
        let total = *speaker_total_counts.get(label).unwrap_or(&1) as f32;
        for val in embedding.iter_mut() {
            *val /= total;
        }
    }

    let num_speakers = speaker_embeddings.len();

    tracing::info!(
        raw_clusters = num_templates,
        merged_speakers = num_speakers,
        threshold = threshold,
        merge_threshold = format!("{:.3}", merge_threshold),
        "speaker clustering complete"
    );

    Ok(DiarizationResult {
        segments,
        num_speakers,
        from_stems: false,
        source_aware: false,
        speaker_embeddings,
    })
}

/// A detected speech region with sample-level boundaries for embedding extraction.
#[cfg(feature = "diarize")]
#[derive(Clone)]
struct SpeechSegment {
    start: f64,
    end: f64,
    start_sample: usize,
    end_sample: usize,
}

/// L2-normalize a vector to unit length. Returns the zero vector if the input
/// has zero norm (avoids NaN propagation).
#[cfg(feature = "diarize")]
fn l2_normalize(v: &[f32]) -> Vec<f32> {
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm == 0.0 {
        return v.to_vec();
    }
    v.iter().map(|x| x / norm).collect()
}

/// Merge speech segments that are separated by gaps shorter than `max_gap`
/// and ensure all resulting segments are at least `min_dur` long by absorbing
/// tiny neighbours. This reduces over-fragmentation from the frame-level
/// segmentation model, producing longer segments with more stable embeddings.
#[cfg(feature = "diarize")]
fn merge_short_segments(segments: Vec<SpeechSegment>, sample_rate: u32) -> Vec<SpeechSegment> {
    if segments.is_empty() {
        return segments;
    }

    let max_gap_samples = (sample_rate as f64 * 0.3) as usize; // 300ms gap tolerance
    let min_dur_samples = (sample_rate as f64 * 0.5) as usize; // 0.5s minimum

    // Cap gap tolerance for short segments so they don't absorb across long pauses.
    let max_short_gap_samples = (sample_rate as f64 * 1.0) as usize; // 1s ceiling

    let mut merged: Vec<SpeechSegment> = Vec::new();
    let mut current = segments[0].clone();

    for seg in segments.iter().skip(1) {
        let gap = seg.start_sample.saturating_sub(current.end_sample);
        let current_dur = current.end_sample.saturating_sub(current.start_sample);

        let should_merge = gap <= max_gap_samples
            || (current_dur < min_dur_samples && gap <= max_short_gap_samples);

        if should_merge {
            current.end = seg.end;
            current.end_sample = seg.end_sample;
        } else {
            merged.push(current);
            current = seg.clone();
        }
    }
    merged.push(current);

    tracing::debug!(
        before = segments.len(),
        after = merged.len(),
        "merged adjacent speech segments"
    );

    merged
}

/// Run the segmentation ONNX model directly with properly normalised f32 audio.
///
/// pyannote-rs's `get_segments` has a bug: it casts raw i16 samples to f32
/// (`x as f32`) without dividing by 32768, so the model receives values in
/// [-32768, 32767] instead of the [-1.0, 1.0] it was trained on. This causes
/// the model to classify all frames as non-speech for typical microphone input.
///
/// This function mirrors the same sliding-window logic but feeds the model
/// correctly normalised f32 waveform data.
#[cfg(feature = "diarize")]
fn segment_speech(
    samples: &[f32],
    sample_rate: u32,
    model_path: &Path,
) -> Result<Vec<SpeechSegment>, Box<dyn std::error::Error>> {
    use ndarray::{Array1, ArrayViewD, Axis, IxDyn};
    use ort::session::builder::GraphOptimizationLevel;
    use ort::session::Session;

    let mut session = Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .commit_from_file(model_path)?;

    // These constants come from the pyannote segmentation-3.0 model architecture:
    // - frame_size (270 samples @ 16kHz = 16.875ms) is the hop between output frames,
    //   derived from the model's sincnet + temporal pooling stride.
    // - frame_start (721 samples @ 16kHz = 45ms) is the receptive-field offset, i.e.
    //   how many input samples precede the center of the first output frame.
    // - window_size (10s @ sample_rate) matches the model's fixed-length input window.
    // See pyannote-rs source and pyannote-audio's SlidingWindowFeature for derivation.
    let frame_size: usize = 270;
    let frame_start: usize = 721;
    let window_size = (sample_rate as usize) * 10;

    // Pad to fill the last window
    let mut padded = samples.to_vec();
    let remainder = padded.len() % window_size;
    if remainder != 0 {
        padded.extend(vec![0.0f32; window_size - remainder]);
    }

    let mut result = Vec::new();
    let mut is_speeching = false;
    let mut offset = frame_start;
    let mut start_offset = 0usize;

    for window_start in (0..padded.len()).step_by(window_size) {
        let window_end = (window_start + window_size).min(padded.len());
        let window = &padded[window_start..window_end];

        let array = Array1::from_iter(window.iter().copied());
        let array = array.view().insert_axis(Axis(0)).insert_axis(Axis(1));

        let inputs = ort::inputs![ort::value::TensorRef::from_array_view(array.into_dyn())
            .map_err(|e| format!("tensor prep: {e:?}"))?];

        let ort_outs = session.run(inputs)?;
        let ort_out = ort_outs
            .get("output")
            .ok_or("segmentation model missing 'output' tensor")?;
        let ort_out = ort_out
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("tensor extract: {e:?}"))?;

        let (shape, data) = ort_out;
        let shape_slice: Vec<usize> = (0..shape.len()).map(|i| shape[i] as usize).collect();
        let view = ArrayViewD::<f32>::from_shape(IxDyn(&shape_slice), data)
            .map_err(|e| format!("ndarray shape: {e}"))?;

        for row in view.outer_iter() {
            for sub_row in row.axis_iter(Axis(0)) {
                let max_index = sub_row
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(i, _)| i)
                    .unwrap_or(0);

                if max_index != 0 {
                    if !is_speeching {
                        start_offset = offset;
                        is_speeching = true;
                    }
                } else if is_speeching {
                    let start_secs = start_offset as f64 / sample_rate as f64;
                    let end_secs = offset as f64 / sample_rate as f64;
                    let si = start_offset.min(samples.len().saturating_sub(1));
                    let ei = offset.min(samples.len());
                    result.push(SpeechSegment {
                        start: start_secs,
                        end: end_secs,
                        start_sample: si,
                        end_sample: ei,
                    });
                    is_speeching = false;
                }
                offset += frame_size;
            }
        }
    }

    // Flush trailing speech (unlike pyannote-rs, we don't drop it)
    if is_speeching {
        let start_secs = start_offset as f64 / sample_rate as f64;
        let end_secs = offset as f64 / sample_rate as f64;
        let si = start_offset.min(samples.len().saturating_sub(1));
        let ei = samples.len();
        result.push(SpeechSegment {
            start: start_secs,
            end: end_secs,
            start_sample: si,
            end_sample: ei,
        });
    }

    Ok(result)
}

/// Load audio file as both f32 (for segmentation) and i16 (for embedding extraction).
///
/// Returns `(f32_samples, i16_samples, sample_rate)` where f32 is normalised
/// to [-1.0, 1.0] and i16 mirrors the same waveform in PCM scale.
#[cfg(feature = "diarize")]
#[allow(clippy::type_complexity)]
fn load_audio(audio_path: &Path) -> Result<(Vec<f32>, Vec<i16>, u32), Box<dyn std::error::Error>> {
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

        if channels > 1 {
            for chunk in samples.chunks(channels) {
                let mono: f32 = chunk.iter().sum::<f32>() / channels as f32;
                all_samples.push(mono);
            }
        } else {
            all_samples.extend_from_slice(samples);
        }
    }

    let i16_samples: Vec<i16> = all_samples
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect();

    Ok((all_samples, i16_samples, sample_rate))
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
        from_stems: false,
        source_aware: false,
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

        assert_eq!(find_speaker(2.5, &segments, false), "SPEAKER_0");
        assert_eq!(find_speaker(7.0, &segments, false), "SPEAKER_1");
        assert_eq!(find_speaker(15.0, &segments, false), "UNKNOWN");
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
        assert_eq!(find_speaker(0.0, &segments, false), "SPEAKER_0");
        // Timestamp 4.0 falls in gap: 0.02s from A end, 0.12s from B start
        // Prefer next speaker (B) — they're about to talk
        assert_eq!(find_speaker(4.0, &segments, false), "SPEAKER_1");
        // Timestamp 8.6 is 0.1s past segment B — within 0.5s tolerance
        assert_eq!(find_speaker(8.6, &segments, false), "SPEAKER_1");
        // Timestamp 10.0 is 1.5s past segment B — beyond 0.5s tolerance
        assert_eq!(find_speaker(10.0, &segments, false), "UNKNOWN");
        // Timestamp 15.0 is far from any segment
        assert_eq!(find_speaker(15.0, &segments, false), "UNKNOWN");
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
        assert_eq!(find_speaker(7.0, &segments, false), "UNKNOWN");
    }

    #[test]
    fn find_speaker_from_stems_allows_larger_forward_tolerance() {
        let segments = vec![
            SpeakerSegment {
                speaker: "SPEAKER_0".into(),
                start: 0.0,
                end: 5.0,
            },
            SpeakerSegment {
                speaker: "SPEAKER_1".into(),
                start: 8.8,
                end: 10.0,
            },
        ];

        assert_eq!(find_speaker(7.0, &segments, false), "UNKNOWN");
        assert_eq!(find_speaker(7.0, &segments, true), "SPEAKER_1");
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
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::new(),
        };

        let labeled = apply_speakers(transcript, &result);
        assert!(labeled.contains("[SPEAKER_0 0:00]"));
        assert!(labeled.contains("[SPEAKER_1 0:05]"));
    }

    #[test]
    fn apply_speakers_first_unknown_inherits_next_speaker() {
        // Simulate Whisper starting at t=0 but diarization detecting speech
        // only from t=1.5 — the first line would be UNKNOWN without the fix
        let transcript = "[0:00] Hello\n[0:03] How are you\n[0:07] Good thanks\n";
        let result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 1.5,
                    end: 5.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 5.0,
                    end: 10.0,
                },
            ],
            num_speakers: 2,
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::new(),
        };

        let labeled = apply_speakers(transcript, &result);
        // First segment inherits from the next attributed segment (SPEAKER_0)
        assert!(
            labeled.contains("[SPEAKER_0 0:00]"),
            "first UNKNOWN should inherit next speaker, got: {labeled}"
        );
        assert!(labeled.contains("[SPEAKER_0 0:03]"));
        assert!(labeled.contains("[SPEAKER_1 0:07]"));
    }

    #[test]
    fn apply_speakers_all_unknown_prefers_dominant_speaker() {
        let transcript = "[0:00] Short intro line\n";
        let result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 1.0,
                    end: 9.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 9.0,
                    end: 10.0,
                },
            ],
            num_speakers: 2,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        };

        let labeled = apply_speakers(transcript, &result);
        assert!(labeled.contains("[SPEAKER_1 0:00]"));
    }

    #[test]
    fn dominant_speaker_requires_clear_majority() {
        let segments = vec![
            SpeakerSegment {
                speaker: "SPEAKER_0".into(),
                start: 0.0,
                end: 5.0,
            },
            SpeakerSegment {
                speaker: "SPEAKER_1".into(),
                start: 5.0,
                end: 9.0,
            },
        ];
        assert_eq!(dominant_speaker_label(&segments), None);
    }

    #[test]
    fn stem_energy_correlation_collapses_to_single_speaker() {
        let voice_energy = vec![(0.0, 0.12), (1.0, 0.20), (2.0, 0.18), (3.0, 0.11)];
        let system_energy = vec![(0.0, 0.08), (1.0, 0.14), (2.0, 0.13), (3.0, 0.07)];

        let result = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 0.85)
            .expect("correlated stems should still produce diarization");

        assert_eq!(result.num_speakers, 1);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].speaker, "SPEAKER_0");
        assert_eq!(result.segments[0].start, 0.0);
        assert_eq!(result.segments[0].end, 4.0);
    }

    #[test]
    fn stem_correlation_threshold_of_one_preserves_remote_label_on_open_speaker_bleed() {
        // Reproduces issue #157: open-speaker mic (Studio Display, laptop,
        // etc.) acoustically picks up multi-speaker system audio. The system
        // stem is louder than the mic (remote voices on speakers), and the
        // mic follows that waveform at lower amplitude — high correlation,
        // but system is the real source.
        //
        // At the default threshold (0.85) both correlation gates fire and
        // everything collapses to SPEAKER_0. Raising the threshold to 1.0
        // must suppress both the primary collapse (line ~418) and the
        // single-speaker relabel (line ~371), leaving the system-dominant
        // per-window attribution intact as SPEAKER_1.
        let voice_energy = vec![(0.0, 0.08), (1.0, 0.14), (2.0, 0.12), (3.0, 0.06)];
        let system_energy = vec![(0.0, 0.20), (1.0, 0.28), (2.0, 0.24), (3.0, 0.12)];

        // Default threshold → collapses to single SPEAKER_0 (the bug).
        let collapsed = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 0.85)
            .expect("default threshold should produce a diarization result");
        assert_eq!(collapsed.segments.len(), 1);
        assert_eq!(collapsed.segments[0].speaker, "SPEAKER_0");

        // Raised threshold → correlation gates skipped, per-window attribution
        // wins, system-dominant windows stay labeled as the remote speaker.
        let preserved = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 1.0)
            .expect("threshold=1.0 must not suppress diarization, only the collapse");
        assert_eq!(preserved.segments[0].speaker, "SPEAKER_1");
    }

    #[test]
    fn stem_energy_distinguishes_two_sources_when_patterns_diverge() {
        let voice_energy = vec![(0.0, 0.16), (1.0, 0.14), (2.0, 0.0), (3.0, 0.0)];
        let system_energy = vec![(0.0, 0.0), (1.0, 0.0), (2.0, 0.18), (3.0, 0.15)];

        let result = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 0.85)
            .expect("distinct stem patterns should produce diarization");

        assert_eq!(result.num_speakers, 2);
        assert_eq!(result.segments.len(), 2);
        assert_eq!(result.segments[0].speaker, "SPEAKER_0");
        assert_eq!(result.segments[1].speaker, "SPEAKER_1");
    }

    #[test]
    fn single_system_dominant_speaker_relabels_to_voice_when_mic_is_consistently_active() {
        let voice_energy = vec![(0.0, 0.020), (1.0, 0.024), (2.0, 0.018), (3.0, 0.022)];
        let system_energy = vec![(0.0, 0.050), (1.0, 0.060), (2.0, 0.045), (3.0, 0.055)];

        let result = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 0.85)
            .expect("single dominant system speaker should still produce diarization");

        assert_eq!(result.num_speakers, 1);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].speaker, "SPEAKER_0");
    }

    #[test]
    fn single_system_dominant_speaker_stays_remote_when_mic_noise_is_uncorrelated() {
        let voice_energy = vec![(0.0, 0.020), (1.0, 0.006), (2.0, 0.019), (3.0, 0.007)];
        let system_energy = vec![(0.0, 0.050), (1.0, 0.048), (2.0, 0.047), (3.0, 0.051)];

        let result = diarization_from_energy_windows(&voice_energy, &system_energy, 1.0, 0.85)
            .expect("single dominant system speaker should still produce diarization");

        assert_eq!(result.num_speakers, 1);
        assert_eq!(result.segments.len(), 1);
        assert_eq!(result.segments[0].speaker, "SPEAKER_1");
    }

    #[test]
    fn remap_diarization_labels_rebases_remote_namespace() {
        let result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "remote-alex".into(),
                    start: 0.0,
                    end: 1.0,
                },
                SpeakerSegment {
                    speaker: "remote-sam".into(),
                    start: 1.0,
                    end: 2.0,
                },
                SpeakerSegment {
                    speaker: "remote-alex".into(),
                    start: 2.0,
                    end: 3.0,
                },
            ],
            num_speakers: 2,
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::from([
                ("remote-alex".to_string(), vec![0.1, 0.2]),
                ("remote-sam".to_string(), vec![0.3, 0.4]),
            ]),
        };

        let remapped = remap_diarization_labels(&result, 1);
        assert_eq!(remapped.num_speakers, 2);
        assert_eq!(remapped.segments[0].speaker, "SPEAKER_1");
        assert_eq!(remapped.segments[1].speaker, "SPEAKER_2");
        assert_eq!(remapped.segments[2].speaker, "SPEAKER_1");
        assert!(remapped.speaker_embeddings.contains_key("SPEAKER_1"));
        assert!(remapped.speaker_embeddings.contains_key("SPEAKER_2"));
    }

    #[test]
    fn merge_remote_diarization_into_stem_result_keeps_local_and_splits_remote_windows() {
        let stem_result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 2.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 2.0,
                    end: 6.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 6.0,
                    end: 7.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 7.0,
                    end: 10.0,
                },
            ],
            num_speakers: 2,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        };
        let remote_result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_2".into(),
                    start: 2.1,
                    end: 3.6,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_3".into(),
                    start: 3.6,
                    end: 5.8,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_3".into(),
                    start: 7.2,
                    end: 8.4,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_2".into(),
                    start: 8.4,
                    end: 9.9,
                },
            ],
            num_speakers: 2,
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::from([
                ("SPEAKER_2".to_string(), vec![0.1]),
                ("SPEAKER_3".to_string(), vec![0.2]),
            ]),
        };

        let merged = merge_remote_diarization_into_stem_result(&stem_result, &remote_result);
        assert_eq!(merged.num_speakers, 4);
        assert!(!merged.from_stems);
        assert!(merged.source_aware);
        assert_eq!(
            merged
                .segments
                .iter()
                .map(|segment| (segment.speaker.as_str(), segment.start, segment.end))
                .collect::<Vec<_>>(),
            vec![
                ("SPEAKER_0", 0.0, 2.0),
                ("SPEAKER_1", 2.0, 2.1),
                ("SPEAKER_2", 2.1, 3.6),
                ("SPEAKER_3", 3.6, 5.8),
                ("SPEAKER_1", 5.8, 6.0),
                ("SPEAKER_0", 6.0, 7.0),
                ("SPEAKER_1", 7.0, 7.2),
                ("SPEAKER_3", 7.2, 8.4),
                ("SPEAKER_2", 8.4, 9.9),
                ("SPEAKER_1", 9.9, 10.0),
            ]
        );
        assert!(merged.speaker_embeddings.contains_key("SPEAKER_2"));
        assert!(merged.speaker_embeddings.contains_key("SPEAKER_3"));
    }

    #[test]
    fn has_meaningful_remote_structure_rejects_noise_but_accepts_one_remote_speaker() {
        let weak_remote = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 2.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 2.0,
                    end: 2.4,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_2".into(),
                    start: 2.4,
                    end: 2.8,
                },
            ],
            num_speakers: 3,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        };
        let single_remote = DiarizationResult {
            segments: vec![SpeakerSegment {
                speaker: "SPEAKER_2".into(),
                start: 1.0,
                end: 2.2,
            }],
            num_speakers: 1,
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::new(),
        };
        let strong_remote = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 1.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 1.0,
                    end: 1.7,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_2".into(),
                    start: 1.7,
                    end: 2.4,
                },
            ],
            num_speakers: 3,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        };

        assert!(!has_meaningful_remote_structure(&weak_remote));
        assert!(has_meaningful_remote_structure(&single_remote));
        assert!(has_meaningful_remote_structure(&strong_remote));
    }

    #[test]
    fn merged_system_stem_label_is_useful_even_without_more_speakers() {
        let stem_result = DiarizationResult {
            segments: vec![
                SpeakerSegment {
                    speaker: "SPEAKER_0".into(),
                    start: 0.0,
                    end: 2.0,
                },
                SpeakerSegment {
                    speaker: "SPEAKER_1".into(),
                    start: 2.0,
                    end: 5.0,
                },
            ],
            num_speakers: 2,
            from_stems: true,
            source_aware: true,
            speaker_embeddings: std::collections::HashMap::new(),
        };
        let remote_result = DiarizationResult {
            segments: vec![SpeakerSegment {
                speaker: "SPEAKER_2".into(),
                start: 2.0,
                end: 5.0,
            }],
            num_speakers: 1,
            from_stems: false,
            source_aware: false,
            speaker_embeddings: std::collections::HashMap::from([(
                "SPEAKER_2".to_string(),
                vec![0.2],
            )]),
        };

        let merged = merge_remote_diarization_into_stem_result(&stem_result, &remote_result);

        assert_eq!(merged.num_speakers, 2);
        assert!(has_meaningful_system_stem_labels(&merged));
        assert_eq!(
            merged
                .segments
                .iter()
                .map(|segment| (segment.speaker.as_str(), segment.start, segment.end))
                .collect::<Vec<_>>(),
            vec![("SPEAKER_0", 0.0, 2.0), ("SPEAKER_2", 2.0, 5.0)]
        );
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
    fn apply_confirmed_names_keeps_non_speech_events_anonymous() {
        let transcript =
            "[SPEAKER_1 0:00] [beep]\n[SPEAKER_1 0:01] Hello there\n[SPEAKER_1 0:02] [typing]\n";
        let result = apply_confirmed_names(
            transcript,
            &[SpeakerAttribution {
                speaker_label: "SPEAKER_1".into(),
                name: "Mat".into(),
                confidence: Confidence::High,
                source: AttributionSource::Manual,
            }],
        );

        assert!(result.contains("[SPEAKER_1 0:00] [beep]"));
        assert!(result.contains("[Mat 0:01] Hello there"));
        assert!(result.contains("[SPEAKER_1 0:02] [typing]"));
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
        assert_eq!(config.diarization.threshold, 0.4);
    }

    // ── l2_normalize tests ──────────────────────────────────────

    #[cfg(feature = "diarize")]
    #[test]
    fn l2_normalize_unit_vector() {
        let v = vec![3.0f32, 4.0];
        let n = l2_normalize(&v);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "expected unit length, got {}",
            norm
        );
        assert!((n[0] - 0.6).abs() < 1e-6);
        assert!((n[1] - 0.8).abs() < 1e-6);
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn l2_normalize_zero_vector() {
        let v = vec![0.0f32; 5];
        let n = l2_normalize(&v);
        assert_eq!(n, v, "zero vector should be returned as-is");
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn l2_normalize_single_element() {
        let v = vec![7.0f32];
        let n = l2_normalize(&v);
        assert!((n[0] - 1.0).abs() < 1e-6);
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn l2_normalize_negative_values() {
        let v = vec![-3.0f32, 4.0];
        let n = l2_normalize(&v);
        let norm: f32 = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
        assert!(n[0] < 0.0, "sign should be preserved");
    }

    // ── merge_short_segments tests ──────────────────────────────

    #[cfg(feature = "diarize")]
    fn make_seg(start_s: f64, end_s: f64, sr: u32) -> SpeechSegment {
        SpeechSegment {
            start: start_s,
            end: end_s,
            start_sample: (start_s * sr as f64) as usize,
            end_sample: (end_s * sr as f64) as usize,
        }
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_empty_input() {
        let result = merge_short_segments(vec![], 16000);
        assert!(result.is_empty());
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_single_segment() {
        let segs = vec![make_seg(0.0, 2.0, 16000)];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(result.len(), 1);
        assert!((result[0].start - 0.0).abs() < 1e-6);
        assert!((result[0].end - 2.0).abs() < 1e-6);
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_merges_small_gaps() {
        // Two segments 200ms apart → should merge (300ms tolerance)
        let segs = vec![make_seg(0.0, 1.0, 16000), make_seg(1.2, 2.0, 16000)];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(result.len(), 1);
        assert!((result[0].end - 2.0).abs() < 1e-6);
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_preserves_large_gaps() {
        // Two segments 2s apart → should NOT merge
        let segs = vec![make_seg(0.0, 1.0, 16000), make_seg(3.0, 4.0, 16000)];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(result.len(), 2);
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_short_segment_respects_gap_ceiling() {
        // A short segment (0.3s) followed by another 1.5s away.
        // Even though the first is <0.5s (min_dur), the gap exceeds the 1s
        // ceiling so they should NOT merge.
        let segs = vec![make_seg(0.0, 0.3, 16000), make_seg(1.8, 3.0, 16000)];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(
            result.len(),
            2,
            "short segment should not absorb across >1s gap"
        );
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_short_segment_merges_within_ceiling() {
        // A short segment (0.3s) followed by another 0.8s away.
        // First is <0.5s and gap is <1s ceiling → should merge.
        let segs = vec![make_seg(0.0, 0.3, 16000), make_seg(1.1, 2.0, 16000)];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(
            result.len(),
            1,
            "short segment should absorb within 1s ceiling"
        );
    }

    #[cfg(feature = "diarize")]
    #[test]
    fn merge_short_segments_all_below_min_duration() {
        // All segments are very short. They should chain-merge until they
        // hit the gap ceiling.
        let segs = vec![
            make_seg(0.0, 0.1, 16000),
            make_seg(0.2, 0.3, 16000),
            make_seg(0.4, 0.5, 16000),
            // 3s gap — exceeds ceiling
            make_seg(3.5, 3.6, 16000),
        ];
        let result = merge_short_segments(segs, 16000);
        assert_eq!(
            result.len(),
            2,
            "chain of short segments should merge, but not across 3s gap"
        );
        assert!((result[0].end - 0.5).abs() < 1e-6);
        assert!((result[1].start - 3.5).abs() < 1e-6);
    }
}
