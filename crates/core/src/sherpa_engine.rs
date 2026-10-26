//! sherpa-onnx transcription engine (feature `engine-sherpa`, opt-in, off by default).
//!
//! In-process via the `sherpa-rs` crate (no Python). Linkage differs by target
//! (see the per-target `sherpa-rs` deps in Cargo.toml): on macOS sherpa-onnx is
//! linked STATICALLY so opt-in app/CLI binaries are self-contained (#369); on
//! Linux/Windows it stays DYNAMIC (upstream's static path needs a global
//! RUSTFLAGS hack there), so binaries must run from the cargo target layout.
//! Either way it coexists with the `ort`-based pyannote/vad path: sherpa's
//! onnxruntime symbols stay separate from the app's `ort` dependency.
//! parakeet-tdt-0.6b-v3 is multilingual (FR/ES/etc.) with correct orthography.
//!
//! Scaffold scope: model directory is resolved from `MINUTES_SHERPA_MODEL_DIR`.
//! A config field + `minutes setup` model download land in phase 2.

use crate::config::Config;
use std::path::PathBuf;
// Path/resolution helpers below are always compiled (pure std/Config) so the
// CLI `setup` command can install + locate models without enabling the engine.
// Only the sherpa-rs transcription path requires the `engine-sherpa` feature.
#[cfg(feature = "engine-sherpa")]
use crate::sherpa_plugin::PluginRecognizer;
#[cfg(all(
    feature = "engine-sherpa",
    feature = "vad-ort",
    not(feature = "whisper")
))]
use crate::vad::VadEngine;

/// The default sherpa parakeet-v3 model variant directory name (under the
/// models base). `minutes setup` installs the int8 export here.
pub const DEFAULT_SHERPA_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";
pub const DEFAULT_SHERPA_STREAMING_MODEL: &str = "streaming-zipformer-en-20M-2023-02-17";

/// Base directory under which sherpa engine models are installed:
/// `<model_path>/sherpa/`. Mirrors the parakeet `installs_root` convention.
pub fn installs_root(config: &Config) -> PathBuf {
    config.transcription.model_path.join("sherpa")
}

/// Resolve the directory holding the parakeet-v3 ONNX files
/// (`encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt`).
///
/// Resolution order: explicit config `sherpa_model_dir` -> the
/// `MINUTES_SHERPA_MODEL_DIR` env override -> the default install location
/// (`<model_path>/sherpa/parakeet-tdt-0.6b-v3-int8`).
pub fn model_dir(config: &Config) -> PathBuf {
    let configured = config.transcription.sherpa_model_dir.trim();
    if !configured.is_empty() {
        return PathBuf::from(configured);
    }
    if let Ok(dir) = std::env::var("MINUTES_SHERPA_MODEL_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    installs_root(config).join(DEFAULT_SHERPA_MODEL)
}

/// Resolve the dedicated true-streaming first-pass model. It is intentionally
/// separate from the excellent but offline Parakeet final model: pointing the
/// online C API at Parakeet v3 aborts inside sherpa because that export has no
/// streaming window metadata.
pub fn streaming_model_dir(config: &Config) -> PathBuf {
    if let Ok(dir) = std::env::var("MINUTES_SHERPA_STREAMING_MODEL_DIR") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir);
        }
    }
    installs_root(config).join(DEFAULT_SHERPA_STREAMING_MODEL)
}

/// Required model files with a conservative minimum byte size. The size floor
/// rejects zero-byte / truncated downloads that a plain existence check would
/// accept (and would then fail at model load).
pub const MODEL_FILES: [(&str, u64); 4] = [
    ("encoder.int8.onnx", 500_000_000),
    ("decoder.int8.onnx", 5_000_000),
    ("joiner.int8.onnx", 3_000_000),
    ("tokens.txt", 10_000),
];

/// The int8 Parakeet v3 export the desktop installer downloads, pinned to one
/// Hugging Face revision so every file can be checked against a known digest.
const PINNED_MODEL_BASE: &str = "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/2bda32ec70b097a55adaa07d9a7173915b43cc78";

/// `(file, exact bytes, sha256)` at [`PINNED_MODEL_BASE`].
pub const PINNED_MODEL_FILES: [(&str, u64, &str); 4] = [
    (
        "encoder.int8.onnx",
        652_184_281,
        "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
    ),
    (
        "decoder.int8.onnx",
        11_845_275,
        "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
    ),
    (
        "joiner.int8.onnx",
        6_355_277,
        "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
    ),
    (
        "tokens.txt",
        93_939,
        "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    ),
];

/// Total download size of the pinned model, for the install prompt.
pub fn pinned_model_bytes() -> u64 {
    PINNED_MODEL_FILES.iter().map(|(_, bytes, _)| bytes).sum()
}

/// The directory the pinned installer writes to: the default location only.
/// A configured or env-overridden `sherpa_model_dir` belongs to the user (or
/// to a manifest-managed install such as Orukeet) and is never written here.
pub fn default_model_dir(config: &Config) -> PathBuf {
    installs_root(config).join(DEFAULT_SHERPA_MODEL)
}

/// Download and verify the pinned Parakeet v3 model into
/// [`default_model_dir`]. Files that already match their digest are kept
/// without a network request; a failed or mismatched download never replaces
/// a file. `progress` receives `(bytes done, total bytes)`.
pub fn install_pinned_model(
    config: &Config,
    progress: &mut dyn FnMut(u64, u64),
) -> Result<PathBuf, String> {
    let dir = default_model_dir(config);
    if model_dir(config) != dir {
        return Err(format!(
            "a custom Parakeet model directory is configured ({}); install there with `minutes setup --sherpa`",
            model_dir(config).display()
        ));
    }
    install_pinned_with(&dir, &PINNED_MODEL_FILES, progress, |file| {
        let response = ureq::get(&format!("{PINNED_MODEL_BASE}/{file}"))
            .config()
            .timeout_connect(Some(std::time::Duration::from_secs(15)))
            .timeout_recv_response(Some(std::time::Duration::from_secs(30)))
            .timeout_global(Some(std::time::Duration::from_secs(60 * 60)))
            .build()
            .call()
            .map_err(|e| format!("Parakeet model download failed: {e}"))?;
        Ok(Box::new(response.into_body().into_reader()) as Box<dyn std::io::Read>)
    })
}

fn install_pinned_with(
    dir: &std::path::Path,
    files: &[(&str, u64, &str)],
    progress: &mut dyn FnMut(u64, u64),
    mut fetch: impl FnMut(&str) -> Result<Box<dyn std::io::Read>, String>,
) -> Result<PathBuf, String> {
    use fs2::FileExt;
    use std::io::{Read, Write};

    // The CLI treats `manifest.json` or `.install.lock` inside a model
    // directory as a manifest-managed install and refuses to write there, so
    // the lock lives beside the directory, not in it.
    for marker in ["manifest.json", ".install.lock"] {
        if dir.join(marker).exists() {
            return Err(format!(
                "{} is a manifest-managed model directory; it was left unchanged",
                dir.display()
            ));
        }
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let parent = dir.parent().unwrap_or(dir);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(parent.join(".parakeet-v3-install.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock_exclusive().map_err(|e| e.to_string())?;

    let total: u64 = files.iter().map(|(_, bytes, _)| bytes).sum();
    let mut done = 0u64;
    progress(done, total);
    for (name, bytes, sha256) in files {
        let dest = dir.join(name);
        if file_verified(&dest, sha256, *bytes)? {
            done += bytes;
            progress(done, total);
            continue;
        }
        let mut staging = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
        let mut body = fetch(name)?.take(bytes + 1);
        let mut buffer = vec![0u8; 1 << 20];
        let mut written = 0u64;
        loop {
            let read = body.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            staging
                .write_all(&buffer[..read])
                .map_err(|e| e.to_string())?;
            written += read as u64;
            progress(done + written.min(*bytes), total);
        }
        staging.flush().map_err(|e| e.to_string())?;
        if !file_verified(staging.path(), sha256, *bytes)? {
            return Err(format!("Parakeet model file {name} failed its checksum"));
        }
        staging.persist(&dest).map_err(|e| e.to_string())?;
        done += bytes;
        progress(done, total);
    }
    Ok(dir.to_path_buf())
}

fn file_verified(path: &std::path::Path, sha256: &str, bytes: u64) -> Result<bool, String> {
    use sha2::{Digest, Sha256};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|e| e.to_string())?.len() != bytes {
        return Ok(false);
    }
    let mut digest = Sha256::new();
    std::io::copy(&mut file, &mut digest).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", digest.finalize()) == sha256)
}

/// ISO 639-1 codes Parakeet v3 (parakeet-tdt-0.6b-v3) transcribes. Whisper
/// covers many more, including Chinese, Japanese, Hindi, Urdu and Arabic.
const PARAKEET_V3_LANGUAGES: &[&str] = &[
    "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv", "mt",
    "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
];

/// Whether Parakeet v3 handles the language this user speaks. An explicit
/// transcription language decides; on auto-detect, a Chinese interface keeps
/// Whisper, since Parakeet v3 has no Chinese.
pub fn parakeet_v3_fits_language(language: Option<&str>, ui_language: &str) -> bool {
    match language
        .map(|code| code.trim().to_ascii_lowercase())
        .filter(|code| !code.is_empty() && code != "auto")
    {
        Some(code) => {
            let base = code.split(['-', '_']).next().unwrap_or("");
            PARAKEET_V3_LANGUAGES.contains(&base)
        }
        None => crate::i18n::resolve_locale(ui_language) != crate::i18n::Locale::ZhCn,
    }
}

pub const STREAMING_MODEL_FILES: [(&str, u64); 4] = [
    ("encoder-epoch-99-avg-1.int8.onnx", 40_000_000),
    ("decoder-epoch-99-avg-1.onnx", 2_000_000),
    ("joiner-epoch-99-avg-1.int8.onnx", 200_000),
    ("tokens.txt", 4_000),
];

/// True when every required model file exists in `dir` and meets its size floor.
pub fn model_files_present(dir: &std::path::Path) -> bool {
    MODEL_FILES.iter().all(|(name, min)| {
        std::fs::metadata(dir.join(name))
            .map(|m| m.is_file() && m.len() >= *min)
            .unwrap_or(false)
    })
}

pub fn streaming_model_files_present(dir: &std::path::Path) -> bool {
    STREAMING_MODEL_FILES.iter().all(|(name, min)| {
        std::fs::metadata(dir.join(name))
            .map(|metadata| metadata.is_file() && metadata.len() >= *min)
            .unwrap_or(false)
    })
}

#[cfg(feature = "engine-sherpa")]
const SAMPLE_RATE: usize = 16_000;
#[cfg(feature = "engine-sherpa")]
const FIXED_WINDOW_SAMPLES: usize = SAMPLE_RATE * 15;
#[cfg(feature = "engine-sherpa")]
const SHERPA_MAX_REGION_SAMPLES: usize = SAMPLE_RATE * 30;
#[cfg(feature = "engine-sherpa")]
const SHERPA_MIN_SPLIT_SEGMENT_SAMPLES: usize = SAMPLE_RATE;
#[cfg(feature = "engine-sherpa")]
const SHERPA_PADDING_SAMPLES: usize = SAMPLE_RATE / 5; // 200 ms
#[cfg(feature = "engine-sherpa")]
const SHERPA_MERGE_GAP_SAMPLES: usize = SAMPLE_RATE * 3 / 10; // 300 ms
#[cfg(feature = "engine-sherpa")]
const ENERGY_WINDOW_SAMPLES: usize = SAMPLE_RATE / 10; // 100 ms
#[cfg(all(
    feature = "engine-sherpa",
    feature = "vad-ort",
    not(feature = "whisper")
))]
const ORT_SILERO_WINDOW_SAMPLES: usize = 512;

#[cfg(feature = "engine-sherpa")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SherpaTranscriptionRange {
    decode_start: usize,
    decode_end: usize,
    speech_start: usize,
}

#[cfg(feature = "engine-sherpa")]
impl SherpaTranscriptionRange {
    fn new(decode_start: usize, decode_end: usize, speech_start: usize) -> Self {
        Self {
            decode_start,
            decode_end,
            speech_start,
        }
    }
}

#[cfg(feature = "engine-sherpa")]
pub(crate) fn build_recognizer(config: &Config) -> Result<PluginRecognizer, String> {
    let dir = model_dir(config);
    if !model_files_present(&dir) {
        return Err(format!(
            "sherpa model not found in {}. Run `minutes setup --sherpa` to download it \
             (or set transcription.sherpa_model_dir / MINUTES_SHERPA_MODEL_DIR).",
            dir.display()
        ));
    }
    // The model layout and the parakeet-TDT loader quirks now live in the
    // plugin, which is the only thing that links sherpa (#685).
    tracing::info!(
        model_dir = %dir.display(),
        "loading sherpa-onnx transducer recognizer through the isolated plugin"
    );
    let started = std::time::Instant::now();
    let result = PluginRecognizer::new(&dir, config);
    tracing::info!(
        elapsed_ms = started.elapsed().as_millis() as u64,
        success = result.is_ok(),
        "sherpa recognizer initialization finished"
    );
    result
}

#[cfg(feature = "engine-sherpa")]
fn fixed_window_ranges(samples_len: usize) -> Vec<SherpaTranscriptionRange> {
    (0..samples_len)
        .step_by(FIXED_WINDOW_SAMPLES)
        .map(|start| {
            SherpaTranscriptionRange::new(
                start,
                (start + FIXED_WINDOW_SAMPLES).min(samples_len),
                start,
            )
        })
        .filter(|range| range.decode_end > range.decode_start)
        .collect()
}

#[cfg(feature = "engine-sherpa")]
fn pad_merge_and_split_regions(
    samples: &[f32],
    speech_regions: &[(usize, usize)],
) -> Vec<SherpaTranscriptionRange> {
    if speech_regions.is_empty() {
        return Vec::new();
    }

    let mut padded = Vec::with_capacity(speech_regions.len());
    for &(start, end) in speech_regions {
        if end <= start || start >= samples.len() {
            continue;
        }
        padded.push(SherpaTranscriptionRange::new(
            start.saturating_sub(SHERPA_PADDING_SAMPLES),
            end.saturating_add(SHERPA_PADDING_SAMPLES)
                .min(samples.len()),
            start,
        ));
    }
    if padded.is_empty() {
        return Vec::new();
    }

    padded.sort_unstable_by_key(|range| range.decode_start);
    let mut merged: Vec<SherpaTranscriptionRange> = Vec::with_capacity(padded.len());
    for range in padded {
        if let Some(last) = merged.last_mut() {
            if range.decode_start.saturating_sub(last.decode_end) < SHERPA_MERGE_GAP_SAMPLES {
                last.decode_end = last.decode_end.max(range.decode_end);
                last.speech_start = last.speech_start.min(range.speech_start);
                continue;
            }
        }
        merged.push(range);
    }

    let mut bounded = Vec::with_capacity(merged.len());
    for range in merged {
        split_long_region(samples, range, &mut bounded);
    }
    bounded
}

#[cfg(feature = "engine-sherpa")]
fn split_long_region(
    samples: &[f32],
    range: SherpaTranscriptionRange,
    out: &mut Vec<SherpaTranscriptionRange>,
) {
    let mut start = range.decode_start;
    let end = range.decode_end;
    let mut speech_start = range.speech_start;
    while end.saturating_sub(start) > SHERPA_MAX_REGION_SAMPLES {
        let hard_end = (start + SHERPA_MAX_REGION_SAMPLES).min(end);
        let split = find_low_energy_split(samples, start, hard_end).unwrap_or(hard_end);
        let split = split.clamp(
            start + SHERPA_MIN_SPLIT_SEGMENT_SAMPLES,
            hard_end.max(start + SHERPA_MIN_SPLIT_SEGMENT_SAMPLES),
        );
        out.push(SherpaTranscriptionRange::new(start, split, speech_start));
        start = split;
        speech_start = split;
    }
    if end > start {
        if end - start < SHERPA_MIN_SPLIT_SEGMENT_SAMPLES {
            if let Some(last) = out.last_mut() {
                last.decode_end = end;
                return;
            }
        }
        out.push(SherpaTranscriptionRange::new(start, end, speech_start));
    }
}

#[cfg(feature = "engine-sherpa")]
fn find_low_energy_split(samples: &[f32], start: usize, hard_end: usize) -> Option<usize> {
    let search_start = start + SHERPA_MIN_SPLIT_SEGMENT_SAMPLES;
    let search_end = hard_end.saturating_sub(SHERPA_MIN_SPLIT_SEGMENT_SAMPLES);
    if search_end <= search_start {
        return None;
    }

    let region_rms = rms(&samples[start..hard_end]);
    let mut best: Option<(usize, f32)> = None;
    let mut window_start = search_start;
    while window_start + ENERGY_WINDOW_SAMPLES <= search_end {
        let window = &samples[window_start..window_start + ENERGY_WINDOW_SAMPLES];
        let rms = rms(window);
        if best.map(|(_, best_rms)| rms < best_rms).unwrap_or(true) {
            best = Some((window_start + ENERGY_WINDOW_SAMPLES / 2, rms));
        }
        window_start += ENERGY_WINDOW_SAMPLES;
    }
    best.and_then(|(split, best_rms)| {
        if best_rms < region_rms * 0.25 {
            Some(split)
        } else {
            None
        }
    })
}

#[cfg(feature = "engine-sherpa")]
fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples
        .iter()
        .map(|sample| (*sample as f64) * (*sample as f64))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt() as f32
}

#[cfg(all(feature = "engine-sherpa", feature = "whisper"))]
fn vad_speech_regions(samples: &[f32], config: &Config) -> Result<Vec<(usize, usize)>, String> {
    let vad_path = crate::transcribe::resolve_vad_model_path(config)
        .ok_or_else(|| "Silero VAD model not found".to_string())?;
    let vad_path = vad_path
        .to_str()
        .ok_or_else(|| format!("Silero VAD path is not UTF-8: {}", vad_path.display()))?;

    let mut ctx_params = whisper_rs::WhisperVadContextParams::default();
    ctx_params.set_n_threads(
        std::thread::available_parallelism()
            .map(|count| count.get() as i32)
            .unwrap_or(4)
            .min(4),
    );

    let mut params = whisper_rs::WhisperVadParams::default();
    params.set_threshold(0.2);
    params.set_min_speech_duration(150);
    params.set_min_silence_duration(500);
    params.set_speech_pad(80);

    let mut ctx = whisper_rs::WhisperVadContext::new(vad_path, ctx_params)
        .map_err(|e| format!("failed to initialize Silero VAD: {e}"))?;
    let detected = ctx
        .segments_from_samples(params, samples)
        .map_err(|e| format!("failed to run Silero VAD: {e}"))?;

    let segment_count = detected.num_segments();
    let mut regions = Vec::with_capacity(segment_count.max(0) as usize);
    for index in 0..segment_count {
        let segment = detected
            .get_segment(index)
            .ok_or_else(|| format!("Silero VAD segment {index} disappeared"))?;
        let start = ((segment.start * 10.0).round().max(0.0) as usize * SAMPLE_RATE / 1000)
            .min(samples.len());
        let end = ((segment.end * 10.0).round().max(0.0) as usize * SAMPLE_RATE / 1000)
            .min(samples.len());
        if end > start {
            regions.push((start, end));
        }
    }

    Ok(regions)
}

#[cfg(all(
    feature = "engine-sherpa",
    feature = "vad-ort",
    not(feature = "whisper")
))]
fn resolve_silero_onnx_path(config: &Config) -> Option<PathBuf> {
    let vad_model = &config.transcription.vad_model;
    if vad_model.is_empty() {
        return None;
    }

    let model_dir = &config.transcription.model_path;
    let candidates = [
        model_dir.join(format!("{vad_model}.onnx")),
        model_dir.join("silero-vad-v6.2.0.onnx"),
        model_dir.join("silero_vad.onnx"),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    let direct = PathBuf::from(vad_model);
    if direct.exists() {
        return Some(direct);
    }

    None
}

#[cfg(all(
    feature = "engine-sherpa",
    feature = "vad-ort",
    not(feature = "whisper")
))]
fn vad_speech_regions(samples: &[f32], config: &Config) -> Result<Vec<(usize, usize)>, String> {
    let vad_path = resolve_silero_onnx_path(config)
        .ok_or_else(|| "Silero ONNX VAD model not found".to_string())?;
    let mut vad = crate::silero_vad::OrtSileroVad::new_catching_unwind(&vad_path)
        .map_err(|e| format!("failed to initialize ort-Silero VAD: {e}"))?;

    let mut regions = Vec::new();
    let mut active_start: Option<usize> = None;
    let mut position = 0usize;
    while position < samples.len() {
        let end = (position + ORT_SILERO_WINDOW_SAMPLES).min(samples.len());
        let chunk = &samples[position..end];
        let rms = rms(chunk);
        let result = vad.process(chunk, rms);
        if !vad.is_healthy() {
            return Err("ort-Silero VAD became unhealthy during sherpa segmentation".to_string());
        }

        if result.speaking {
            active_start.get_or_insert(position);
        } else if let Some(start) = active_start.take() {
            regions.push((start, end));
        }

        position = end;
    }

    if let Some(start) = active_start {
        regions.push((start, samples.len()));
    }

    Ok(regions)
}

#[cfg(feature = "engine-sherpa")]
fn vad_segmented_ranges(
    samples: &[f32],
    vad_result: Result<Vec<(usize, usize)>, String>,
) -> Result<Vec<SherpaTranscriptionRange>, String> {
    vad_result.map(|regions| {
        let ranges = pad_merge_and_split_regions(samples, &regions);
        if ranges.is_empty() {
            tracing::info!(
                samples = samples.len(),
                vad_regions = regions.len(),
                "Silero VAD produced no sherpa speech ranges; returning silence"
            );
            return Vec::new();
        }

        tracing::info!(
            samples = samples.len(),
            vad_regions = regions.len(),
            transcription_ranges = ranges.len(),
            "sherpa transcription using Silero VAD segmentation"
        );
        ranges
    })
}

#[cfg(feature = "engine-sherpa")]
fn transcription_ranges(samples: &[f32], config: &Config) -> Vec<SherpaTranscriptionRange> {
    #[cfg(any(feature = "whisper", feature = "vad-ort"))]
    {
        match vad_segmented_ranges(samples, vad_speech_regions(samples, config)) {
            Ok(ranges) => {
                return ranges;
            }
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "Silero VAD unavailable for sherpa transcription; falling back to fixed windows"
                );
            }
        }
    }

    fixed_window_ranges(samples.len())
}

/// Transcribe 16 kHz mono f32, returning `(start_ms, text)` for each non-empty
/// speech segment. When Silero VAD is available, sherpa receives speech-boundary
/// regions with padding, tiny-gap merge, and long-region splitting. If VAD is
/// unavailable or fails, this falls back to the legacy 15 s fixed windows. If
/// VAD runs successfully and finds no speech, this returns no segments.
///
/// This is the raw engine, with no whisper fallback: a missing plugin, an
/// unloadable one, or a decode failure returns `Err`. Callers that must not
/// lose a recording should go through `transcribe::transcribe_dispatch`, which
/// answers with whisper whenever sherpa cannot.
#[cfg(feature = "engine-sherpa")]
pub fn transcribe_segments(samples: &[f32], config: &Config) -> Result<Vec<(u64, String)>, String> {
    let mut recognizer = build_recognizer(config)?;
    transcribe_segments_with_recognizer(samples, config, &mut recognizer)
}

/// Reuse model weights across chunks while keeping each decode stream independent.
#[cfg(feature = "engine-sherpa")]
pub(crate) fn transcribe_segments_with_recognizer(
    samples: &[f32],
    config: &Config,
    recognizer: &mut PluginRecognizer,
) -> Result<Vec<(u64, String)>, String> {
    let started = std::time::Instant::now();
    let ranges = transcription_ranges(samples, config);
    let segmentation_ms = started.elapsed().as_millis() as u64;
    let decode_started = std::time::Instant::now();
    let mut segments = Vec::new();
    for range in ranges {
        let window = &samples[range.decode_start..range.decode_end];
        let start_ms = range.speech_start as u64 * 1000 / SAMPLE_RATE as u64;
        let text = recognizer.transcribe(16_000, window)?.trim().to_string();
        if !text.is_empty() {
            segments.push((start_ms, text));
        }
    }
    tracing::info!(
        samples = samples.len(),
        segments = segments.len(),
        segmentation_ms,
        decode_ms = decode_started.elapsed().as_millis() as u64,
        "sherpa-onnx transcription complete"
    );
    Ok(segments)
}

/// Text-only transcript (concatenated windows). Back-compat for callers that do
/// not need timestamps; `transcribe_segments` is preferred for the meeting path.
#[cfg(feature = "engine-sherpa")]
pub fn transcribe_samples(samples: &[f32], config: &Config) -> Result<String, String> {
    Ok(transcribe_segments(samples, config)?
        .into_iter()
        .map(|(_, text)| text)
        .collect::<Vec<_>>()
        .join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sha256_hex(bytes: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn pinned_install_downloads_verifies_and_skips_verified_files() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("parakeet");
        let a = sha256_hex(b"alpha");
        let b = sha256_hex(b"bravo!");
        let files = [("a.onnx", 5, a.as_str()), ("b.txt", 6, b.as_str())];
        let mut fetched = Vec::new();
        let mut last = (0, 0);
        install_pinned_with(
            &dir,
            &files,
            &mut |done, total| last = (done, total),
            |name| {
                fetched.push(name.to_string());
                let body: &'static [u8] = if name == "a.onnx" {
                    b"alpha"
                } else {
                    b"bravo!"
                };
                Ok(Box::new(body) as Box<dyn std::io::Read>)
            },
        )
        .unwrap();
        assert_eq!(fetched, ["a.onnx", "b.txt"]);
        assert_eq!(last, (11, 11));
        assert_eq!(std::fs::read(dir.join("a.onnx")).unwrap(), b"alpha");
        assert!(!dir.join(".install.lock").exists());

        // A second run finds both files verified and fetches nothing.
        install_pinned_with(&dir, &files, &mut |_, _| {}, |name| {
            panic!("unexpected fetch of {name}")
        })
        .unwrap();
    }

    #[test]
    fn pinned_install_rejects_a_mismatched_download_and_keeps_nothing() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("parakeet");
        let want = sha256_hex(b"alpha");
        let files = [("a.onnx", 5, want.as_str())];
        let error = install_pinned_with(&dir, &files, &mut |_, _| {}, |_| {
            Ok(Box::new(&b"omega"[..]) as Box<dyn std::io::Read>)
        })
        .unwrap_err();
        assert!(error.contains("checksum"), "{error}");
        assert!(!dir.join("a.onnx").exists());
    }

    #[test]
    fn pinned_install_leaves_a_manifest_managed_directory_alone() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("manifest.json"), "{}").unwrap();
        let error = install_pinned_with(root.path(), &[], &mut |_, _| {}, |_| {
            panic!("no fetch expected")
        })
        .unwrap_err();
        assert!(error.contains("manifest-managed"), "{error}");
    }

    /// Downloads the real 640 MB model from the pinned revision into a
    /// temporary directory. Run with `--ignored` after changing the pin.
    #[test]
    #[ignore = "downloads 640 MB from Hugging Face"]
    fn pinned_install_downloads_the_real_model() {
        let root = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.transcription.sherpa_model_dir = String::new();
        config.transcription.model_path = root.path().to_path_buf();
        let mut last = (0, 0);
        let dir = install_pinned_model(&config, &mut |done, total| last = (done, total)).unwrap();
        assert_eq!(last, (pinned_model_bytes(), pinned_model_bytes()));
        assert!(model_files_present(&dir));
    }

    #[test]
    fn pinned_install_refuses_a_custom_model_directory() {
        let mut config = Config::default();
        config.transcription.sherpa_model_dir = "/custom/sherpa".into();
        let error = install_pinned_model(&config, &mut |_, _| {}).unwrap_err();
        assert!(error.contains("custom Parakeet model directory"), "{error}");
    }

    #[test]
    fn model_dir_prefers_explicit_config_field() {
        let mut config = Config::default();
        config.transcription.sherpa_model_dir = "/custom/sherpa".into();
        assert_eq!(model_dir(&config), PathBuf::from("/custom/sherpa"));
    }

    #[test]
    fn model_dir_defaults_under_model_path() {
        let mut config = Config::default();
        config.transcription.sherpa_model_dir = String::new();
        config.transcription.model_path = PathBuf::from("/models");
        // Env override (if set in the test environment) takes precedence over the
        // default; only assert the default-path shape when it is unset.
        if std::env::var("MINUTES_SHERPA_MODEL_DIR").is_err() {
            assert_eq!(
                model_dir(&config),
                PathBuf::from("/models/sherpa").join(DEFAULT_SHERPA_MODEL)
            );
        }
    }

    #[test]
    fn model_files_present_requires_all_and_size_floor() {
        let tmp = std::env::temp_dir().join(format!("sherpa-files-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // Missing files.
        assert!(!model_files_present(&tmp));
        // 1-byte files are below every floor -> still "not present" (truncation guard).
        for (name, _min) in MODEL_FILES {
            std::fs::write(tmp.join(name), b"x").unwrap();
        }
        assert!(!model_files_present(&tmp));
        // Sparse files at the floor size satisfy presence without a real disk write.
        for (name, min) in MODEL_FILES {
            let f = std::fs::File::create(tmp.join(name)).unwrap();
            f.set_len(min).unwrap();
        }
        assert!(model_files_present(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[cfg(feature = "engine-sherpa")]
    fn samples(seconds: usize, amplitude: f32) -> Vec<f32> {
        vec![amplitude; seconds * SAMPLE_RATE]
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_ranges_align_to_speech_boundaries_and_merge_tiny_gaps() {
        let mut audio = samples(1, 0.0);
        audio.extend(samples(2, 0.05));
        audio.extend(vec![0.0; SHERPA_MERGE_GAP_SAMPLES / 2]);
        audio.extend(samples(2, 0.05));
        audio.extend(samples(4, 0.0));
        audio.extend(samples(2, 0.05));

        let first_start = SAMPLE_RATE;
        let first_end = first_start + SAMPLE_RATE * 2;
        let second_start = first_end + SHERPA_MERGE_GAP_SAMPLES / 2;
        let second_end = second_start + SAMPLE_RATE * 2;
        let third_start = second_end + SAMPLE_RATE * 4;
        let third_end = third_start + SAMPLE_RATE * 2;

        let ranges = pad_merge_and_split_regions(
            &audio,
            &[
                (first_start, first_end),
                (second_start, second_end),
                (third_start, third_end),
            ],
        );

        assert_eq!(
            ranges,
            vec![
                SherpaTranscriptionRange::new(
                    first_start - SHERPA_PADDING_SAMPLES,
                    second_end + SHERPA_PADDING_SAMPLES,
                    first_start,
                ),
                SherpaTranscriptionRange::new(
                    third_start - SHERPA_PADDING_SAMPLES,
                    (third_end + SHERPA_PADDING_SAMPLES).min(audio.len()),
                    third_start,
                )
            ],
            "speech-boundary ranges should merge tiny gaps instead of using 15s cuts"
        );
        assert_ne!(ranges[0].decode_end, FIXED_WINDOW_SAMPLES);
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_empty_success_returns_no_ranges() {
        let audio = samples(30, 0.0);

        let ranges = vad_segmented_ranges(&audio, Ok(Vec::new())).unwrap();

        assert!(
            ranges.is_empty(),
            "successful VAD with no speech must not fall back to fixed windows"
        );
        assert_ne!(ranges, fixed_window_ranges(audio.len()));
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_error_falls_back_to_fixed_windows() {
        let audio = samples(30, 0.0);

        let ranges = vad_segmented_ranges(&audio, Err("init failed".to_string()))
            .unwrap_or_else(|_| fixed_window_ranges(audio.len()));

        assert_eq!(ranges, fixed_window_ranges(audio.len()));
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_range_reports_unpadded_speech_start() {
        let audio = samples(15, 0.05);
        let region_start = SAMPLE_RATE * 10;
        let region_end = SAMPLE_RATE * 12;

        let ranges = pad_merge_and_split_regions(&audio, &[(region_start, region_end)]);

        assert_eq!(ranges.len(), 1);
        assert_eq!(
            ranges[0].decode_start,
            region_start - SHERPA_PADDING_SAMPLES
        );
        assert_eq!(ranges[0].speech_start, region_start);
        assert_eq!(
            ranges[0].speech_start as u64 * 1000 / SAMPLE_RATE as u64,
            10_000
        );
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_ranges_split_long_regions_at_low_energy_gap() {
        let mut audio = samples(10, 0.05);
        audio.extend(samples(1, 0.0));
        audio.extend(samples(25, 0.05));
        let speech_end = audio.len();

        let ranges = pad_merge_and_split_regions(&audio, &[(0, speech_end)]);

        assert_eq!(
            ranges,
            vec![
                SherpaTranscriptionRange::new(0, SAMPLE_RATE * 10 + ENERGY_WINDOW_SAMPLES / 2, 0),
                SherpaTranscriptionRange::new(
                    SAMPLE_RATE * 10 + ENERGY_WINDOW_SAMPLES / 2,
                    speech_end,
                    SAMPLE_RATE * 10 + ENERGY_WINDOW_SAMPLES / 2,
                )
            ],
            "long speech regions should split at the quietest internal gap"
        );
        assert!(ranges
            .iter()
            .all(|range| range.decode_end - range.decode_start <= SHERPA_MAX_REGION_SAMPLES));
    }

    #[test]
    #[cfg(feature = "engine-sherpa")]
    fn sherpa_vad_ranges_absorb_tiny_final_tail() {
        let audio = vec![0.05; SHERPA_MAX_REGION_SAMPLES + SHERPA_MIN_SPLIT_SEGMENT_SAMPLES / 2];

        let ranges = pad_merge_and_split_regions(&audio, &[(0, audio.len())]);

        assert_eq!(
            ranges,
            vec![SherpaTranscriptionRange::new(0, audio.len(), 0)]
        );
    }
}
