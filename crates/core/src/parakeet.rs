use crate::config::{Config, VALID_PARAKEET_MODELS};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Once;
use thiserror::Error;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveParakeetBinaryMode {
    WarnAndFallback,
    Strict,
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ResolveParakeetBinaryError {
    message: String,
}

impl ResolveParakeetBinaryError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

pub fn resolve_parakeet_binary(
    configured_path: &str,
    mode: ResolveParakeetBinaryMode,
) -> Result<PathBuf, ResolveParakeetBinaryError> {
    let configured_path = configured_path.trim();
    let auto_resolution_requested = configured_path.is_empty() || configured_path == "parakeet";

    if !auto_resolution_requested {
        let configured_candidate = PathBuf::from(configured_path);
        if verify_parakeet_binary(&configured_candidate).is_ok() {
            return Ok(configured_candidate);
        }

        return match mode {
            ResolveParakeetBinaryMode::Strict => Err(ResolveParakeetBinaryError::new(format!(
                "Configured parakeet binary '{}' is not executable or failed `--version`.",
                configured_path
            ))),
            ResolveParakeetBinaryMode::WarnAndFallback => match auto_resolve_parakeet_binary() {
                Ok((resolved, candidates_tried)) => {
                    log_configured_fallback_once(configured_path, &resolved);
                    log_auto_resolve_once(&candidates_tried, &resolved);
                    Ok(resolved)
                }
                Err(auto_error) => Err(ResolveParakeetBinaryError::new(format!(
                    "Configured parakeet binary '{}' is not executable or failed `--version`. {}",
                    configured_path, auto_error
                ))),
            },
        };
    }

    let (resolved, candidates_tried) = auto_resolve_parakeet_binary()?;
    log_auto_resolve_once(&candidates_tried, &resolved);
    Ok(resolved)
}

fn auto_resolve_parakeet_binary() -> Result<(PathBuf, Vec<String>), ResolveParakeetBinaryError> {
    let candidates = parakeet_binary_candidates();
    let mut candidates_tried = Vec::new();

    for candidate in candidates {
        candidates_tried.push(candidate.display().to_string());
        if verify_parakeet_binary(&candidate).is_ok() {
            return Ok((candidate, candidates_tried));
        }
    }

    Err(ResolveParakeetBinaryError::new(format!(
        "No working parakeet binary was found. Tried: {}. Run `minutes setup --parakeet` or install parakeet.cpp from https://github.com/Frikallo/parakeet.cpp.",
        if candidates_tried.is_empty() {
            "<none>".to_string()
        } else {
            candidates_tried.join(", ")
        }
    )))
}

fn parakeet_binary_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(homebrew_prefix) = std::env::var_os("HOMEBREW_PREFIX") {
        candidates.push(PathBuf::from(homebrew_prefix).join("bin").join("parakeet"));
    }

    candidates.push(PathBuf::from("/opt/homebrew/bin/parakeet"));
    candidates.push(PathBuf::from("/usr/local/bin/parakeet"));

    if let Some(home_dir) = std::env::var_os("HOME") {
        let home_dir = PathBuf::from(home_dir);
        candidates.push(home_dir.join(".local/bin/parakeet"));
        candidates.push(home_dir.join(".cargo/bin/parakeet"));
    }

    if let Ok(path_binary) = which::which("parakeet") {
        candidates.push(path_binary);
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
    deduped
}

fn verify_parakeet_binary(path: &Path) -> Result<(), ()> {
    if !path.is_file() {
        return Err(());
    }

    let status = std::process::Command::new(path)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| ())?;

    if status.success() {
        Ok(())
    } else {
        Err(())
    }
}

fn log_auto_resolve_once(candidates_tried: &[String], chosen_path: &Path) {
    static LOG_ONCE: Once = Once::new();

    let chosen_path = chosen_path.display().to_string();
    let candidates_tried = candidates_tried.to_vec();

    LOG_ONCE.call_once(move || {
        tracing::info!(
            step = "parakeet_resolve",
            chosen_path = %chosen_path,
            candidates_tried = ?candidates_tried,
            "resolved parakeet binary"
        );
        crate::logging::log_step(
            "parakeet_resolve",
            &chosen_path,
            0,
            serde_json::json!({
                "candidates_tried": candidates_tried,
                "chosen_path": chosen_path,
            }),
        );
    });
}

fn log_configured_fallback_once(configured_path: &str, resolved_path: &Path) {
    static WARN_ONCE: Once = Once::new();

    let configured_path = configured_path.to_string();
    let resolved_path = resolved_path.display().to_string();

    WARN_ONCE.call_once(move || {
        tracing::warn!(
            configured_path = %configured_path,
            found_path = %resolved_path,
            "configured parakeet_binary at {} is not executable; auto-resolving to {}. Update your config to silence this warning.",
            configured_path,
            resolved_path
        );
    });
}

// ── Word-to-sentence grouping ───────────────────────────────────

#[cfg(feature = "parakeet")]
use crate::transcribe::ParakeetCliSegment;

#[cfg(feature = "parakeet")]
const GAP_THRESHOLD_SECS: f64 = 0.8;
#[cfg(feature = "parakeet")]
const WORD_CAP: usize = 30;

/// Group word-level parakeet segments into sentence-level segments.
///
/// Flush rules (evaluated after each word):
/// 1. Punctuation flush — previous word ends with `.` `!` `?` `。` `！` `？`.
/// 2. Gap flush — gap to next word exceeds `GAP_THRESHOLD_SECS`.
/// 3. Word-cap flush — buffer reaches `WORD_CAP` words (runaway safety net).
/// 4. Trailing flush — final word always flushes any remaining buffer.
#[cfg(feature = "parakeet")]
pub fn group_word_segments(words: &[ParakeetCliSegment]) -> Vec<ParakeetCliSegment> {
    let mut grouped = Vec::new();
    let mut current: Option<ParakeetCliSegment> = None;
    let mut word_count: usize = 0;

    for word in words {
        // Multi-word segments (already grouped upstream) pass through as-is.
        if word.text.chars().any(char::is_whitespace) {
            if let Some(segment) = current.take() {
                grouped.push(segment);
                word_count = 0;
            }
            grouped.push(word.clone());
            continue;
        }

        match current.as_mut() {
            None => {
                current = Some(word.clone());
                word_count = 1;
            }
            Some(segment) => {
                let gap = word.start_secs - segment.end_secs;
                let ends_sentence = segment
                    .text
                    .chars()
                    .last()
                    .map(|c| matches!(c, '.' | '!' | '?' | '。' | '！' | '？'))
                    .unwrap_or(false);

                if gap > GAP_THRESHOLD_SECS || ends_sentence || word_count >= WORD_CAP {
                    grouped.push(segment.clone());
                    *segment = word.clone();
                    word_count = 1;
                } else {
                    if !segment.text.is_empty() {
                        segment.text.push(' ');
                    }
                    segment.text.push_str(&word.text);
                    segment.end_secs = word.end_secs;
                    segment.confidence = match (segment.confidence, word.confidence) {
                        (Some(left), Some(right)) => Some((left + right) / 2.0),
                        (Some(left), None) => Some(left),
                        (None, Some(right)) => Some(right),
                        (None, None) => None,
                    };
                    word_count += 1;
                }
            }
        }
    }

    if let Some(segment) = current {
        grouped.push(segment);
    }

    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn set_env_var(key: &str, value: &std::ffi::OsStr) -> Option<std::ffi::OsString> {
        let previous = env::var_os(key);
        env::set_var(key, value);
        previous
    }

    fn restore_env_var(key: &str, previous: Option<std::ffi::OsString>) {
        if let Some(previous) = previous {
            env::set_var(key, previous);
        } else {
            env::remove_var(key);
        }
    }

    #[cfg(unix)]
    fn write_fake_parakeet_binary(path: &Path) {
        fs::write(path, "#!/bin/sh\nprintf 'parakeet 0.1.0\\n'\n").unwrap();
        let mut permissions = fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions).unwrap();
    }

    #[cfg(windows)]
    fn write_fake_parakeet_binary(path: &Path) {
        fs::write(path, "@echo off\r\necho parakeet 0.1.0\r\n").unwrap();
    }

    #[cfg(windows)]
    fn fake_parakeet_filename() -> &'static str {
        "parakeet.bat"
    }

    #[cfg(not(windows))]
    fn fake_parakeet_filename() -> &'static str {
        "parakeet"
    }

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

    #[test]
    fn resolve_parakeet_binary_prefers_working_configured_path() {
        let _env_lock = crate::test_home_env_lock();
        let dir = tempfile::TempDir::new().unwrap();
        let binary_path = dir.path().join(fake_parakeet_filename());
        write_fake_parakeet_binary(&binary_path);

        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let resolved = resolve_parakeet_binary(
            binary_path.to_str().unwrap(),
            ResolveParakeetBinaryMode::WarnAndFallback,
        )
        .unwrap();
        assert_eq!(resolved, binary_path);

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[test]
    fn resolve_parakeet_binary_auto_finds_homebrew_candidate() {
        let _env_lock = crate::test_home_env_lock();
        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();
        let brew_bin_dir = homebrew_prefix.path().join("bin");
        fs::create_dir_all(&brew_bin_dir).unwrap();
        let binary_path = brew_bin_dir.join(fake_parakeet_filename());
        write_fake_parakeet_binary(&binary_path);

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let resolved =
            resolve_parakeet_binary("parakeet", ResolveParakeetBinaryMode::WarnAndFallback)
                .unwrap();
        assert_eq!(resolved, binary_path);

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[test]
    fn resolve_parakeet_binary_errors_when_nothing_is_available() {
        let _env_lock = crate::test_home_env_lock();
        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let error = resolve_parakeet_binary("parakeet", ResolveParakeetBinaryMode::WarnAndFallback)
            .unwrap_err();
        assert!(error.to_string().contains("minutes setup --parakeet"));

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[test]
    fn resolve_parakeet_binary_can_fail_strictly_for_broken_config() {
        let _env_lock = crate::test_home_env_lock();
        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();
        let missing_binary = home.path().join("missing-parakeet");

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let error = resolve_parakeet_binary(
            missing_binary.to_str().unwrap(),
            ResolveParakeetBinaryMode::Strict,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Configured parakeet binary"));

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[test]
    fn resolve_parakeet_binary_falls_back_from_broken_config_when_alternative_exists() {
        let _env_lock = crate::test_home_env_lock();
        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();
        let missing_binary = home.path().join("missing-parakeet");
        let brew_bin_dir = homebrew_prefix.path().join("bin");
        fs::create_dir_all(&brew_bin_dir).unwrap();
        let fallback_binary = brew_bin_dir.join(fake_parakeet_filename());
        write_fake_parakeet_binary(&fallback_binary);

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let resolved = resolve_parakeet_binary(
            missing_binary.to_str().unwrap(),
            ResolveParakeetBinaryMode::WarnAndFallback,
        )
        .unwrap();
        assert_eq!(resolved, fallback_binary);

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[test]
    fn resolve_parakeet_binary_combines_error_when_fallback_also_fails() {
        let _env_lock = crate::test_home_env_lock();
        let home = tempfile::TempDir::new().unwrap();
        let homebrew_prefix = tempfile::TempDir::new().unwrap();
        let empty_path = tempfile::TempDir::new().unwrap();
        let missing_binary = home.path().join("missing-parakeet");

        let old_home = set_env_var("HOME", home.path().as_os_str());
        let old_homebrew_prefix =
            set_env_var("HOMEBREW_PREFIX", homebrew_prefix.path().as_os_str());
        let old_path = set_env_var("PATH", empty_path.path().as_os_str());

        let error = resolve_parakeet_binary(
            missing_binary.to_str().unwrap(),
            ResolveParakeetBinaryMode::WarnAndFallback,
        )
        .unwrap_err();
        assert!(error.to_string().contains("Configured parakeet binary"));
        assert!(error.to_string().contains("minutes setup --parakeet"));

        restore_env_var("PATH", old_path);
        restore_env_var("HOMEBREW_PREFIX", old_homebrew_prefix);
        restore_env_var("HOME", old_home);
    }

    #[cfg(feature = "parakeet")]
    fn seg(text: &str, start: f64, end: f64) -> ParakeetCliSegment {
        ParakeetCliSegment {
            text: text.into(),
            start_secs: start,
            end_secs: end,
            confidence: None,
        }
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn punctuation_flush_breaks_on_terminator() {
        let words = vec![
            seg("Hello", 0.0, 0.4),
            seg("world.", 0.4, 0.9),
            seg("Again", 0.9, 1.3),
        ];
        let grouped = group_word_segments(&words);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[0].text, "Hello world.");
        assert_eq!(grouped[1].text, "Again");
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn gap_flush_breaks_on_long_pause() {
        let words = vec![
            seg("one", 0.0, 0.3),
            seg("two", 1.2, 1.5), // 0.9s gap > 0.8s threshold
            seg("three", 62.0, 62.3),
        ];
        let grouped = group_word_segments(&words);
        assert_eq!(grouped.len(), 3);
        assert_eq!(grouped[0].text, "one");
        assert_eq!(grouped[1].text, "two");
        assert_eq!(grouped[2].text, "three");
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn trailing_flush_emits_final_segment() {
        let words = vec![seg("solitary", 5.0, 5.4)];
        let grouped = group_word_segments(&words);
        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].text, "solitary");
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn word_cap_flush_prevents_runaway() {
        let words: Vec<_> = (0..35)
            .map(|i| seg("word", i as f64 * 0.1, i as f64 * 0.1 + 0.08))
            .collect();
        let grouped = group_word_segments(&words);
        assert!(
            grouped.len() >= 2,
            "30-word cap should split into multiple segments: got {}",
            grouped.len()
        );
    }
}
