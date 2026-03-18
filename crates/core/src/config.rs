use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────────────────────
// Config loading precedence:
//   Compiled defaults → config file override → CLI flag override
//
// Config file is OPTIONAL. minutes works without one.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub output_dir: PathBuf,
    pub transcription: TranscriptionConfig,
    pub diarization: DiarizationConfig,
    pub summarization: SummarizationConfig,
    pub search: SearchConfig,
    pub security: SecurityConfig,
    pub watch: WatchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub model: String,
    pub model_path: PathBuf,
    pub min_words: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiarizationConfig {
    pub engine: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizationConfig {
    pub engine: String,
    pub chunk_max_tokens: usize,
    pub ollama_url: String,
    pub ollama_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub engine: String,
    pub qmd_collection: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub allowed_audio_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchConfig {
    pub paths: Vec<PathBuf>,
    pub extensions: Vec<String>,
    pub r#type: String,
    pub diarize: bool,
    pub delete_source: bool,
    pub settle_delay_ms: u64,
}

// ── Defaults ─────────────────────────────────────────────────

fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

fn minutes_dir() -> PathBuf {
    home_dir().join(".minutes")
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output_dir: home_dir().join("meetings"),
            transcription: TranscriptionConfig::default(),
            diarization: DiarizationConfig::default(),
            summarization: SummarizationConfig::default(),
            search: SearchConfig::default(),
            security: SecurityConfig::default(),
            watch: WatchConfig::default(),
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            model: "small".into(),
            model_path: minutes_dir().join("models"),
            min_words: 10,
        }
    }
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        Self {
            engine: "none".into(),
        }
    }
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            engine: "none".into(),
            chunk_max_tokens: 4000,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3.2".into(),
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            engine: "builtin".into(),
            qmd_collection: None,
        }
    }
}

// SecurityConfig::default() derives empty vec for allowed_audio_dirs.
// Empty = allow all paths (permissive default for local CLI use).
// Set explicitly in config.toml for MCP/networked use:
//   allowed_audio_dirs = ["~/.minutes/inbox", "~/meetings"]

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            paths: vec![minutes_dir().join("inbox")],
            extensions: vec![
                "m4a".into(),
                "wav".into(),
                "mp3".into(),
                "ogg".into(),
                "webm".into(),
            ],
            r#type: "memo".into(),
            diarize: false,
            delete_source: false,
            settle_delay_ms: 2000,
        }
    }
}

// ── Loading ──────────────────────────────────────────────────

impl Config {
    /// Standard config file location.
    pub fn config_path() -> PathBuf {
        // Prefer ~/.config/minutes/ (Unix convention, documented in README)
        let unix_path = home_dir()
            .join(".config")
            .join("minutes")
            .join("config.toml");
        if unix_path.exists() {
            return unix_path;
        }
        // Fall back to platform-native (~/Library/Application Support/ on macOS)
        let native_path = dirs::config_dir()
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("minutes")
            .join("config.toml");
        if native_path.exists() {
            return native_path;
        }
        // Default to Unix-standard path
        unix_path
    }

    /// Load config from file, falling back to defaults.
    /// If the config file doesn't exist, returns defaults silently.
    /// If the config file exists but is invalid, logs a warning and returns defaults.
    pub fn load() -> Self {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    /// Load config from a specific path. Used for testing.
    pub fn load_from(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }

        match std::fs::read_to_string(path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => config,
                Err(e) => {
                    tracing::warn!(
                        "invalid config at {}: {}. Using defaults.",
                        path.display(),
                        e
                    );
                    Self::default()
                }
            },
            Err(e) => {
                tracing::warn!(
                    "could not read config at {}: {}. Using defaults.",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    /// Ensure required directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.output_dir)?;
        std::fs::create_dir_all(self.output_dir.join("memos"))?;
        std::fs::create_dir_all(minutes_dir())?;
        std::fs::create_dir_all(minutes_dir().join("inbox"))?;
        std::fs::create_dir_all(minutes_dir().join("inbox").join("processed"))?;
        std::fs::create_dir_all(minutes_dir().join("inbox").join("failed"))?;
        std::fs::create_dir_all(minutes_dir().join("logs"))?;
        Ok(())
    }

    /// Path to the minutes state directory (~/.minutes/).
    pub fn minutes_dir() -> PathBuf {
        minutes_dir()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.transcription.model, "small");
        assert_eq!(config.transcription.min_words, 10);
        assert_eq!(config.diarization.engine, "none");
        assert_eq!(config.summarization.engine, "none");
        assert_eq!(config.search.engine, "builtin");
        assert_eq!(config.watch.settle_delay_ms, 2000);
        assert!(!config.watch.extensions.is_empty());
    }

    #[test]
    fn missing_config_file_returns_defaults() {
        let config = Config::load_from(Path::new("/nonexistent/config.toml"));
        assert_eq!(config.transcription.model, "small");
    }

    #[test]
    fn partial_config_merges_with_defaults() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "large-v3"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.model, "large-v3");
        // Other fields should be defaults
        assert_eq!(config.transcription.min_words, 10);
        assert_eq!(config.diarization.engine, "none");
    }

    #[test]
    fn invalid_toml_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "this is not valid toml {{{").unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.model, "small");
    }
}
