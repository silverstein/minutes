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
use sherpa_rs::transducer::{TransducerConfig, TransducerRecognizer};
use std::path::PathBuf;

/// Resolve the directory holding the parakeet-v3 ONNX files
/// (`encoder.int8.onnx`, `decoder.int8.onnx`, `joiner.int8.onnx`, `tokens.txt`).
fn model_dir(_config: &Config) -> Result<PathBuf, String> {
    match std::env::var("MINUTES_SHERPA_MODEL_DIR") {
        Ok(dir) if !dir.is_empty() => Ok(PathBuf::from(dir)),
        _ => Err(
            "sherpa engine: set MINUTES_SHERPA_MODEL_DIR to the parakeet-v3 \
                  model directory (encoder/decoder/joiner .onnx + tokens.txt). \
                  Config-based setup lands in phase 2."
                .into(),
        ),
    }
}

/// Transcribe 16 kHz mono f32 samples with parakeet-tdt-0.6b-v3 via sherpa-onnx.
///
/// Returns the trimmed transcript text, or an error string. The caller
/// (`transcribe_sherpa_dispatch`) wraps errors into `TranscribeError`.
pub fn transcribe_samples(samples: &[f32], config: &Config) -> Result<String, String> {
    let dir = model_dir(config)?;
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
