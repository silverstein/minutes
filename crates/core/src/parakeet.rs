use crate::config::{Config, VALID_PARAKEET_MODELS};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub const PARAKEET_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParakeetInstallFile {
    pub filename: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParakeetInstallMetadata {
    pub schema_version: u32,
    pub model_id: String,
    pub source_repo: String,
    pub source_artifact: String,
    pub model_file: ParakeetInstallFile,
    pub tokenizer_file: ParakeetInstallFile,
    pub installed_at: String,
}

pub fn installs_root(config: &Config) -> PathBuf {
    config.transcription.model_path.join("parakeet")
}

pub fn install_dir(config: &Config, model: &str) -> PathBuf {
    installs_root(config).join(model)
}

pub fn metadata_path(config: &Config, model: &str) -> PathBuf {
    install_dir(config, model).join("metadata.json")
}

pub fn default_tokenizer_filename(model: &str) -> String {
    format!("{}.tokenizer.vocab", model)
}

pub fn default_model_filename(model: &str) -> String {
    format!("{}.safetensors", model)
}

pub fn source_repo_for_model(model: &str) -> &'static str {
    match model {
        "tdt-ctc-110m" => "nvidia/parakeet-tdt_ctc-110m",
        "tdt-600m" => "nvidia/parakeet-tdt-0.6b-v3",
        _ => "unknown",
    }
}

pub fn source_artifact_for_model(model: &str) -> &'static str {
    match model {
        "tdt-ctc-110m" => "parakeet-tdt_ctc-110m.nemo",
        "tdt-600m" => "parakeet-tdt-0.6b-v3.nemo",
        _ => "unknown.nemo",
    }
}

pub fn read_install_metadata(config: &Config, model: &str) -> Option<ParakeetInstallMetadata> {
    let path = metadata_path(config, model);
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn write_install_metadata(
    config: &Config,
    model: &str,
    model_path: &Path,
    tokenizer_path: &Path,
) -> io::Result<PathBuf> {
    let model_size = fs::metadata(model_path)?.len();
    let tokenizer_size = fs::metadata(tokenizer_path)?.len();
    let metadata = ParakeetInstallMetadata {
        schema_version: PARAKEET_SCHEMA_VERSION,
        model_id: model.to_string(),
        source_repo: source_repo_for_model(model).to_string(),
        source_artifact: source_artifact_for_model(model).to_string(),
        model_file: ParakeetInstallFile {
            filename: model_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string(),
            size_bytes: model_size,
        },
        tokenizer_file: ParakeetInstallFile {
            filename: tokenizer_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string(),
            size_bytes: tokenizer_size,
        },
        installed_at: Utc::now().to_rfc3339(),
    };
    let dir = install_dir(config, model);
    fs::create_dir_all(&dir)?;
    let path = metadata_path(config, model);
    fs::write(&path, serde_json::to_string_pretty(&metadata)?)?;
    Ok(path)
}

pub fn resolve_model_file(config: &Config, model: &str) -> Option<PathBuf> {
    let direct = PathBuf::from(model);
    if direct.exists() {
        return Some(direct);
    }

    let dir = install_dir(config, model);
    let model_filename = default_model_filename(model);
    let install_candidate = dir.join(&model_filename);
    if install_candidate.exists() {
        return Some(install_candidate);
    }

    if let Some(metadata) = read_install_metadata(config, model) {
        let metadata_candidate = dir.join(metadata.model_file.filename);
        if metadata_candidate.exists() {
            return Some(metadata_candidate);
        }
    }

    let root = installs_root(config);
    let legacy_candidates = [
        root.join(&model_filename),
        root.join(format!("parakeet-{}.safetensors", model)),
        root.join("model.safetensors"),
    ];
    legacy_candidates
        .into_iter()
        .find(|candidate| candidate.exists())
}

pub fn resolve_tokenizer_file(
    config: &Config,
    model: &str,
    configured_vocab: &str,
) -> Option<PathBuf> {
    let direct = PathBuf::from(configured_vocab);
    if direct.exists() {
        return Some(direct);
    }

    let dir = install_dir(config, model);
    let mut candidates = Vec::new();

    if !matches!(configured_vocab, "" | "tokenizer.vocab" | "vocab.txt") {
        candidates.push(dir.join(configured_vocab));
    }

    if let Some(metadata) = read_install_metadata(config, model) {
        candidates.push(dir.join(metadata.tokenizer_file.filename));
    }

    for filename in tokenizer_filename_candidates(model) {
        candidates.push(dir.join(filename));
    }

    let root = installs_root(config);
    if !matches!(configured_vocab, "" | "tokenizer.vocab" | "vocab.txt") {
        candidates.push(root.join(configured_vocab));
    }
    for filename in tokenizer_filename_candidates(model) {
        candidates.push(root.join(filename));
    }

    let mut deduped = Vec::new();
    for candidate in candidates {
        if !deduped
            .iter()
            .any(|existing: &PathBuf| existing == &candidate)
        {
            deduped.push(candidate);
        }
    }

    deduped.into_iter().find(|candidate| candidate.exists())
}

pub fn tokenizer_filename_candidates(model: &str) -> &'static [&'static str] {
    match model {
        "tdt-ctc-110m" => &[
            "tdt-ctc-110m.tokenizer.vocab",
            "tdt-ctc-110m.vocab",
            "tokenizer.vocab",
            "vocab.txt",
        ],
        "tdt-600m" => &[
            "tdt-600m.tokenizer.vocab",
            "tdt-600m.vocab",
            "tokenizer.vocab",
            "vocab.txt",
        ],
        _ => &["tokenizer.vocab", "vocab.txt"],
    }
}

pub fn valid_model(model: &str) -> bool {
    VALID_PARAKEET_MODELS.contains(&model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_model_prefers_model_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.transcription.model_path = dir.path().to_path_buf();

        let root = installs_root(&config);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("tdt-ctc-110m.safetensors"), b"legacy").unwrap();

        let isolated_dir = install_dir(&config, "tdt-ctc-110m");
        fs::create_dir_all(&isolated_dir).unwrap();
        let isolated_model = isolated_dir.join("tdt-ctc-110m.safetensors");
        fs::write(&isolated_model, b"isolated").unwrap();

        let resolved = resolve_model_file(&config, "tdt-ctc-110m").unwrap();
        assert_eq!(resolved, isolated_model);
    }

    #[test]
    fn metadata_roundtrip_works() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = Config::default();
        config.transcription.model_path = dir.path().to_path_buf();

        let isolated_dir = install_dir(&config, "tdt-ctc-110m");
        fs::create_dir_all(&isolated_dir).unwrap();
        let model_path = isolated_dir.join("tdt-ctc-110m.safetensors");
        let tokenizer_path = isolated_dir.join("tdt-ctc-110m.tokenizer.vocab");
        fs::write(&model_path, b"model-bytes").unwrap();
        fs::write(&tokenizer_path, b"tokenizer-bytes").unwrap();

        let metadata_path =
            write_install_metadata(&config, "tdt-ctc-110m", &model_path, &tokenizer_path).unwrap();
        assert!(metadata_path.exists());

        let metadata = read_install_metadata(&config, "tdt-ctc-110m").unwrap();
        assert_eq!(metadata.model_id, "tdt-ctc-110m");
        assert_eq!(metadata.model_file.filename, "tdt-ctc-110m.safetensors");
        assert_eq!(
            metadata.tokenizer_file.filename,
            "tdt-ctc-110m.tokenizer.vocab"
        );
    }
}
