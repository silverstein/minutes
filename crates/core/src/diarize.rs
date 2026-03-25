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
//   - wespeaker_en_voxceleb_CAM++.onnx (speaker embeddings)
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
pub fn apply_confirmed_names(
    transcript: &str,
    attributions: &[SpeakerAttribution],
) -> String {
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
                        output.push_str(&format!("[{} {}{}\n", name, &inside[space_pos + 1..], after));
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
pub const EMBEDDING_MODEL: &str = "wespeaker_en_voxceleb_CAM++.onnx";

/// Download URLs for diarization models (hosted on pyannote-rs GitHub releases).
pub const SEGMENTATION_MODEL_URL: &str =
    "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/segmentation-3.0.onnx";
pub const EMBEDDING_MODEL_URL: &str =
    "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/wespeaker_en_voxceleb_CAM++.onnx";

/// Check if diarization models are installed.
pub fn models_installed(config: &Config) -> bool {
    let dir = &config.diarization.model_path;
    dir.join(SEGMENTATION_MODEL).exists() && dir.join(EMBEDDING_MODEL).exists()
}

/// Run speaker diarization on an audio file.
/// Returns None if diarization is disabled or models are not available.
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

    let result = match resolved_engine {
        #[cfg(feature = "diarize")]
        "pyannote-rs" => diarize_with_pyannote_rs(audio_path, config),
        #[cfg(not(feature = "diarize"))]
        "pyannote-rs" => {
            tracing::error!("pyannote-rs engine requires the 'diarize' feature. Rebuild with: cargo build --features diarize");
            return None;
        }
        "pyannote" => diarize_with_pyannote(audio_path),
        other => {
            tracing::warn!(engine = %other, "unknown diarization engine, skipping");
            return None;
        }
    };

    match result {
        Ok(result) => {
            tracing::info!(
                speakers = result.num_speakers,
                segments = result.segments.len(),
                "diarization complete"
            );
            Some(result)
        }
        Err(e) => {
            tracing::error!(error = %e, "diarization failed, continuing without speaker labels");
            None
        }
    }
}

/// Apply diarization results to a transcript.
/// Replaces timestamp-only lines with speaker-labeled lines.
pub fn apply_speakers(transcript: &str, result: &DiarizationResult) -> String {
    let mut output = String::new();

    for line in transcript.lines() {
        // Parse timestamp from lines like "[0:00] text"
        if let Some(rest) = line.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let ts_str = &rest[..bracket_end];
                let text = rest[bracket_end + 1..].trim();

                if let Some(secs) = parse_timestamp(ts_str) {
                    let speaker = find_speaker(secs, &result.segments);
                    output.push_str(&format!("[{} {}] {}\n", speaker, ts_str, text));
                    continue;
                }
            }
        }
        output.push_str(line);
        output.push('\n');
    }

    output
}

/// Find which speaker is talking at a given timestamp.
fn find_speaker(time_secs: f64, segments: &[SpeakerSegment]) -> &str {
    for seg in segments {
        if time_secs >= seg.start && time_secs < seg.end {
            return &seg.speaker;
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
    let emb_model = model_dir.join(EMBEDDING_MODEL);

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
    for segment_result in segments_iter {
        let segment = segment_result?;
        let embedding: Vec<f32> = extractor.compute(&segment.samples)?.collect();

        let speaker_id = manager
            .search_speaker(embedding, threshold)
            .map(|id| id.to_string())
            .unwrap_or_else(|| "0".to_string());

        segments.push(SpeakerSegment {
            speaker: format!("SPEAKER_{}", speaker_id),
            start: segment.start,
            end: segment.end,
        });
    }

    let num_speakers = segments
        .iter()
        .map(|s| s.speaker.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len();

    Ok(DiarizationResult {
        segments,
        num_speakers,
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
        let result = apply_confirmed_names(transcript, &[SpeakerAttribution {
            speaker_label: "SPEAKER_1".into(),
            name: "Mat".into(),
            confidence: Confidence::Medium,
            source: AttributionSource::Deterministic,
        }]);
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
        let config = Config::default();
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
