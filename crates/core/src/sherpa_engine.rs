//! sherpa-onnx transcription engine (feature `engine-sherpa`, opt-in, off by default).
//!
//! In-process via the `sherpa-rs` crate (no Python). Validated 2026-06-24 to
//! coexist with the existing `ort`-based pyannote/vad path: `sherpa-rs-sys`
//! statically embeds onnxruntime with hidden symbols (no `links = onnxruntime`
//! manifest), `ort` ships its own dynamic onnxruntime, and macOS two-level
//! namespacing keeps them separate (both happen to be onnxruntime 1.17.1).
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
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};

/// The default sherpa parakeet-v3 model variant directory name (under the
/// models base). `minutes setup` installs the int8 export here.
pub const DEFAULT_SHERPA_MODEL: &str = "parakeet-tdt-0.6b-v3-int8";

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

/// Required model files with a conservative minimum byte size. The size floor
/// rejects zero-byte / truncated downloads that a plain existence check would
/// accept (and would then fail at model load).
pub const MODEL_FILES: [(&str, u64); 4] = [
    ("encoder.int8.onnx", 500_000_000),
    ("decoder.int8.onnx", 5_000_000),
    ("joiner.int8.onnx", 3_000_000),
    ("tokens.txt", 10_000),
];

/// True when every required model file exists in `dir` and meets its size floor.
pub fn model_files_present(dir: &std::path::Path) -> bool {
    MODEL_FILES.iter().all(|(name, min)| {
        std::fs::metadata(dir.join(name))
            .map(|m| m.is_file() && m.len() >= *min)
            .unwrap_or(false)
    })
}

/// Transcribe 16 kHz mono f32 samples with parakeet-tdt-0.6b-v3 via sherpa-onnx.
///
/// Returns the trimmed transcript text, or an error string. The caller
/// (`transcribe_sherpa_dispatch`) wraps errors into `TranscribeError`.
#[cfg(feature = "engine-sherpa")]
pub fn transcribe_samples(samples: &[f32], config: &Config) -> Result<String, String> {
    let dir = model_dir(config);
    if !model_files_present(&dir) {
        return Err(format!(
            "sherpa model not found in {}. Run `minutes setup --sherpa` to download it \
             (or set transcription.sherpa_model_dir / MINUTES_SHERPA_MODEL_DIR).",
            dir.display()
        ));
    }
    let path = |file: &str| dir.join(file).to_string_lossy().into_owned();

    let cfg = TransducerConfig {
        encoder: path("encoder.int8.onnx"),
        decoder: path("decoder.int8.onnx"),
        joiner: path("joiner.int8.onnx"),
        tokens: path("tokens.txt"),
        num_threads: 4,
        decoding_method: "greedy_search".into(),
        // Empty model_type -> sherpa auto-detects the NeMo parakeet-TDT loader.
        // The default "transducer" forces the generic loader, which fails with
        // "vocab_size does not exist in the metadata".
        model_type: String::new(),
        debug: false,
        ..Default::default()
    };

    let mut recognizer =
        TransducerRecognizer::new(cfg).map_err(|e| format!("failed to load sherpa model: {e}"))?;
    Ok(recognizer.transcribe(16_000, samples).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
