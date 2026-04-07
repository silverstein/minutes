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
    pub daily_notes: DailyNotesConfig,
    pub security: SecurityConfig,
    pub watch: WatchConfig,
    pub assistant: AssistantConfig,
    pub screen_context: ScreenContextConfig,
    pub calendar: CalendarConfig,
    pub call_detection: CallDetectionConfig,
    pub identity: IdentityConfig,
    pub vault: VaultConfig,
    pub dictation: DictationConfig,
    pub voice: VoiceConfig,
    pub live_transcript: LiveTranscriptConfig,
    pub recording: RecordingConfig,
    pub hooks: HooksConfig,
    pub knowledge: KnowledgeConfig,
    pub palette: PaletteConfig,
}

/// Command palette configuration.
///
/// The palette is the keyboard-first command surface introduced in v0.11.
/// Upgrades from earlier versions default `shortcut_enabled` to `false`
/// so that users who mapped `⌘⇧K` to something else (VS Code Delete Line,
/// JetBrains Push, etc.) are not surprised by a new global shortcut.
/// Fresh installs default to `true` via `Default::default` — see
/// [`Config::load_with_migrations`] for the detection.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PaletteConfig {
    /// Whether the global palette shortcut is enabled.
    pub shortcut_enabled: bool,
    /// The palette shortcut string (e.g., "CmdOrCtrl+Shift+K").
    pub shortcut: String,
}

impl Default for PaletteConfig {
    fn default() -> Self {
        // Fresh-install default. The upgrade path in
        // `Config::load_with_migrations` rewrites this to `false` for users
        // who already have a config file without a `[palette]` section.
        Self {
            shortcut_enabled: true,
            shortcut: "CmdOrCtrl+Shift+K".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub match_threshold: f32,
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            match_threshold: 0.65,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TranscriptionConfig {
    /// Transcription engine: "whisper" (default) or "parakeet".
    pub engine: String,
    pub model: String,
    pub model_path: PathBuf,
    pub min_words: usize,
    pub language: Option<String>,
    /// Silero VAD model name (resolved under model_path, e.g. "silero-v6.2.0" → ggml-silero-v6.2.0.bin).
    /// Set to empty string to disable VAD (falls back to energy-based silence stripping).
    pub vad_model: String,
    /// Enable noise reduction via nnnoiseless (RNNoise) before transcription.
    /// Requires the `denoise` feature flag. Default: true.
    pub noise_reduction: bool,
    /// Path or name of the parakeet.cpp binary (resolved via PATH if not absolute).
    pub parakeet_binary: String,
    /// Parakeet model type: "tdt-ctc-110m", "tdt-600m".
    pub parakeet_model: String,
    /// SentencePiece vocab filename (resolved under model_path/parakeet/, e.g. "vocab.txt").
    pub parakeet_vocab: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiarizationConfig {
    pub engine: String,
    pub model_path: PathBuf,
    /// Cosine similarity threshold for speaker matching (0.0–1.0).
    /// Lower values merge more aggressively; higher values create more speakers.
    pub threshold: f32,
    /// Speaker embedding model: "cam++" (default) or "cam++-lm".
    /// CAM++_LM has ~12% lower EER on benchmarks but produces lower cosine
    /// similarities, so `voice.match_threshold` must be lowered (~0.1–0.2)
    /// for voice enrollment matching to work reliably.
    pub embedding_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizationConfig {
    pub engine: String,
    pub agent_command: String,
    pub chunk_max_tokens: usize,
    pub ollama_url: String,
    pub ollama_model: String,
    pub mistral_model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    pub engine: String,
    pub qmd_collection: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DailyNotesConfig {
    pub enabled: bool,
    pub path: PathBuf,
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
    /// Files shorter than this duration route as Memo (skip diarization).
    /// Set to 0 to disable duration-based routing (use `type` config instead).
    pub dictation_threshold_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenContextConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    pub keep_after_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AssistantConfig {
    pub agent: String,
    pub agent_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarConfig {
    pub enabled: bool,
}

impl Default for CalendarConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CallDetectionConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub cooldown_minutes: u64,
    pub apps: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentityConfig {
    pub name: Option<String>,
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    pub destination: String,
    pub accumulate: bool,
    pub daily_note_log: bool,
    pub cleanup_engine: String,
    pub auto_paste: bool,
    pub auto_paste_restore: bool,
    pub silence_timeout_ms: u64,
    pub max_utterance_secs: u64,
    pub destination_file: String,
    pub destination_command: String,
    pub model: String,
    pub shortcut_enabled: bool,
    pub shortcut: String,
    pub hotkey_enabled: bool,
    pub hotkey_keycode: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VaultConfig {
    pub enabled: bool,
    /// Root path of the markdown vault (e.g., ~/Documents/life)
    pub path: PathBuf,
    /// Subdirectory inside vault where meetings are placed (e.g., "areas/meetings")
    pub meetings_subdir: String,
    /// Sync strategy: "auto", "symlink", "copy", or "direct"
    pub strategy: String,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            destination: "clipboard".into(),
            accumulate: true,
            daily_note_log: true,
            cleanup_engine: String::new(),
            auto_paste: false,
            auto_paste_restore: true,
            silence_timeout_ms: 2000,
            max_utterance_secs: 120,
            destination_file: String::new(),
            destination_command: String::new(),
            model: "base".into(),
            shortcut_enabled: false,
            shortcut: "CmdOrCtrl+Shift+Space".into(),
            hotkey_enabled: false,
            hotkey_keycode: 57, // Caps Lock
        }
    }
}

impl Default for VaultConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: PathBuf::new(),
            meetings_subdir: "areas/meetings".into(),
            strategy: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// Seconds of continuous silence before sending a reminder notification.
    /// Set to 0 to disable. Default: 300 (5 minutes).
    pub silence_reminder_secs: u64,
    /// Audio level (0–100) below which audio is considered silence.
    /// The level comes from RMS energy of the mic input. Default: 3.
    pub silence_threshold: u32,
    /// Seconds of continuous silence before auto-stopping the recording.
    /// Set to 0 to disable. Default: 1800 (30 minutes).
    pub silence_auto_stop_secs: u64,
    /// Maximum recording duration in seconds. Auto-stops at this limit.
    /// Set to 0 to disable. Default: 28800 (8 hours).
    pub max_duration_secs: u64,
    /// Minimum free disk space (MB) before auto-stopping. Set to 0 to disable.
    /// Default: 500.
    pub min_disk_space_mb: u64,
    /// Audio input device name override. When set, Minutes uses this device
    /// instead of the system default. Use `minutes devices` to list available names.
    pub device: Option<String>,
    /// Automatically infer call intent when a known call app is detected.
    /// Default: false. Process-based detection has high false-positive rates
    /// (e.g. Zoom running but not in a call). Users trigger call capture
    /// explicitly via the call detection banner instead.
    pub auto_call_intent: bool,
    /// Allow Minutes to start a call capture even when the selected input
    /// looks like a plain microphone rather than a system-audio route.
    pub allow_degraded_call_capture: bool,
    /// Multi-source capture: explicit voice + call device names.
    /// When set, `device` is ignored. CLI `--source` flags override this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<SourcesConfig>,
}

/// Multi-source capture configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesConfig {
    /// Voice (microphone) device name, or "default" for system default.
    pub voice: Option<String>,
    /// Call (system audio) device name, or "auto" to detect loopback devices.
    pub call: Option<String>,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            silence_reminder_secs: 300,
            silence_threshold: 3,
            silence_auto_stop_secs: 1800,
            max_duration_secs: 28800,
            min_disk_space_mb: 500,
            device: None,
            auto_call_intent: false,
            allow_degraded_call_capture: false,
            sources: None,
        }
    }
}

/// Knowledge base integration — Karpathy-style LLM wiki maintained from meeting data.
/// After each meeting, extract facts about people and decisions, update person profiles,
/// append to a chronological log, and maintain an index. Opt-in (disabled by default).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    /// Enable knowledge base updates after each meeting.
    pub enabled: bool,
    /// Root path of the knowledge base (e.g., ~/wiki or ~/Documents/life).
    pub path: PathBuf,
    /// Output adapter: "wiki" (flat markdown), "para" (PARA areas/people/), "obsidian" (wiki + [[links]]).
    pub adapter: String,
    /// Fact extraction engine: "agent" (shells out to agent_command), "ollama", "none" (structured data only).
    /// "none" extracts only from YAML frontmatter (decisions, action_items, entities) — no LLM call, zero hallucination risk.
    pub engine: String,
    /// Agent CLI to invoke for extraction. Default: "claude".
    pub agent_command: String,
    /// Chronological append-only log filename inside knowledge path.
    pub log_file: String,
    /// Content-oriented index filename inside knowledge path.
    pub index_file: String,
    /// Minimum confidence for facts to be written. "explicit" (safest), "strong", "inferred", "tentative".
    /// Facts below this threshold are logged but not written to person profiles.
    pub min_confidence: String,
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: PathBuf::new(),
            adapter: "wiki".into(),
            engine: "none".into(),
            agent_command: "claude".into(),
            log_file: "log.md".into(),
            index_file: "index.md".into(),
            min_confidence: "strong".into(),
        }
    }
}

/// Hooks configuration — shell commands triggered by pipeline events.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Shell command to run after a recording is processed.
    /// The transcript file path is appended as the last argument.
    /// Example: "/path/to/script.sh" → executed as: /path/to/script.sh /path/to/meeting.md
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_record: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LiveTranscriptConfig {
    /// Whisper model to use for live transcription.
    /// Empty string means "use the dictation model".
    pub model: String,
    /// Maximum utterance length in seconds before force-finalizing.
    pub max_utterance_secs: u64,
    /// Whether to save raw WAV alongside JSONL for post-meeting reprocessing.
    pub save_wav: bool,
    /// Whether the keyboard shortcut is enabled.
    pub shortcut_enabled: bool,
    /// The keyboard shortcut string (e.g., "CmdOrCtrl+Shift+L").
    pub shortcut: String,
}

impl Default for LiveTranscriptConfig {
    fn default() -> Self {
        Self {
            model: String::new(), // empty = use dictation model
            max_utterance_secs: 30,
            save_wav: true,
            shortcut_enabled: false,
            shortcut: "CmdOrCtrl+Shift+L".into(),
        }
    }
}

impl Default for ScreenContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_secs: 30,
            keep_after_summary: false,
        }
    }
}

impl Default for AssistantConfig {
    fn default() -> Self {
        Self {
            agent: "claude".into(),
            agent_args: vec![],
        }
    }
}

impl Default for CallDetectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 1,
            cooldown_minutes: 5,
            apps: vec!["zoom.us".into(), "Microsoft Teams".into(), "Webex".into()],
        }
    }
}

// ── Defaults ─────────────────────────────────────────────────

fn home_dir() -> PathBuf {
    // Check env vars directly first — dirs::home_dir() on Windows uses
    // SHGetKnownFolderPath which ignores runtime env var changes, breaking
    // test isolation via with_temp_home().
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home);
    }
    #[cfg(windows)]
    if let Some(up) = std::env::var_os("USERPROFILE") {
        return PathBuf::from(up);
    }
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
            daily_notes: DailyNotesConfig::default(),
            security: SecurityConfig::default(),
            watch: WatchConfig::default(),
            assistant: AssistantConfig::default(),
            screen_context: ScreenContextConfig::default(),
            calendar: CalendarConfig::default(),
            call_detection: CallDetectionConfig::default(),
            identity: IdentityConfig::default(),
            vault: VaultConfig::default(),
            dictation: DictationConfig::default(),
            voice: VoiceConfig::default(),
            live_transcript: LiveTranscriptConfig::default(),
            recording: RecordingConfig::default(),
            hooks: HooksConfig::default(),
            knowledge: KnowledgeConfig::default(),
            palette: PaletteConfig::default(),
        }
    }
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            engine: "whisper".into(),
            model: "small".into(),
            model_path: minutes_dir().join("models"),
            min_words: 3,
            language: None,
            vad_model: "silero-v6.2.0".into(),
            noise_reduction: true,
            parakeet_binary: "parakeet".into(),
            parakeet_model: "tdt-ctc-110m".into(),
            parakeet_vocab: "vocab.txt".into(),
        }
    }
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        Self {
            engine: "auto".into(),
            model_path: minutes_dir().join("models").join("diarization"),
            threshold: 0.4,
            embedding_model: "cam++".into(),
        }
    }
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            engine: "auto".into(),
            agent_command: "claude".into(),
            chunk_max_tokens: 4000,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3.2".into(),
            mistral_model: "mistral-large-latest".into(),
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

impl Default for DailyNotesConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            path: home_dir().join("meetings").join("daily"),
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
            dictation_threshold_secs: 120,
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

    /// Load the config file with first-run and upgrade migrations applied.
    ///
    /// This is the entry point the Tauri desktop app uses at startup. The
    /// CLI and non-app consumers can continue to use [`Self::load`] if they
    /// do not need migration side effects.
    ///
    /// Currently runs:
    /// - **Palette upgrade migration**: if the config file exists but has
    ///   no `[palette]` section, the user is upgrading from a version that
    ///   had no palette. Force `palette.shortcut_enabled = false` and
    ///   persist the change so the next load is a no-op. This prevents a
    ///   silent global-shortcut hijack on upgrade — VS Code, JetBrains, and
    ///   Firefox all bind `⌘⇧K` to app-level commands.
    ///
    /// Fresh installs (file does not exist) skip every migration and take
    /// the compiled defaults verbatim.
    pub fn load_with_migrations() -> Self {
        let path = Self::config_path();
        Self::load_with_migrations_from(&path)
    }

    /// Testable form of [`Self::load_with_migrations`]. Reads from `path`,
    /// runs migrations, and writes the migrated config back if it changed.
    pub fn load_with_migrations_from(path: &Path) -> Self {
        let file_existed = path.exists();
        let raw_toml = if file_existed {
            std::fs::read_to_string(path).ok()
        } else {
            None
        };

        let mut config = Self::load_from(path);
        let mut changed = false;

        // Palette upgrade: detect the absence of a `[palette]` section in a
        // pre-existing config. `toml::from_str` silently fills missing
        // sections with `Default`, so the parsed struct alone does not tell
        // us whether the user explicitly chose their setting or inherited
        // the default. A text check on the raw TOML is the only way to
        // distinguish "new field, never seen" from "user set it to true".
        if file_existed {
            if let Some(raw) = raw_toml.as_deref() {
                if !raw_toml_has_section(raw, "palette") {
                    config.palette.shortcut_enabled = false;
                    changed = true;
                    tracing::info!(
                        "palette upgrade migration: disabled shortcut for existing config at {}",
                        path.display()
                    );
                }
            }
        }

        if changed {
            if let Err(e) = config.save_to(path) {
                tracing::warn!(
                    "failed to persist config migration to {}: {}",
                    path.display(),
                    e
                );
            }
        }

        config
    }

    /// Load config from a specific path. Used for testing and by
    /// [`Self::load_with_migrations_from`].
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

    /// Save config to the standard config file location.
    /// Creates the config directory and file if they don't exist.
    pub fn save(&self) -> std::io::Result<()> {
        let path = Self::config_path();
        Self::save_to(self, &path)
    }

    /// Save config to a specific path.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let contents = toml::to_string_pretty(self)
            .map_err(|e| std::io::Error::other(format!("TOML serialize: {}", e)))?;
        std::fs::write(path, contents)?;
        tracing::info!(path = %path.display(), "config saved");
        Ok(())
    }

    /// Ensure required directories exist.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.output_dir)?;
        std::fs::create_dir_all(self.output_dir.join("memos"))?;
        if self.daily_notes.enabled {
            std::fs::create_dir_all(&self.daily_notes.path)?;
        }
        std::fs::create_dir_all(minutes_dir())?;
        std::fs::create_dir_all(minutes_dir().join("inbox"))?;
        std::fs::create_dir_all(minutes_dir().join("inbox").join("processed"))?;
        std::fs::create_dir_all(minutes_dir().join("inbox").join("failed"))?;
        std::fs::create_dir_all(minutes_dir().join("logs"))?;

        // Block macOS Spotlight from indexing sensitive transcript data
        for dir in [&self.output_dir, &minutes_dir()] {
            let marker = dir.join(".metadata_never_index");
            if !marker.exists() {
                std::fs::write(&marker, "").ok();
            }
        }

        Ok(())
    }

    /// Path to the minutes state directory (~/.minutes/).
    pub fn minutes_dir() -> PathBuf {
        minutes_dir()
    }
}

/// Return `true` iff the raw TOML text contains a top-level `[section]`
/// header. This is a deliberately primitive text check — we cannot use
/// `toml::from_str` to answer this question because serde's `#[serde(default)]`
/// silently fills missing sections with their default values, so a parsed
/// struct never tells you whether a key was present in the file.
///
/// The check:
/// - Ignores leading whitespace
/// - Skips lines that start with `#` (comments)
/// - Does not try to understand inline tables, dotted keys, or array tables
///   like `[[section]]` — those are not how Minutes writes its config, and
///   `toml::to_string_pretty` always emits bare `[section]` headers for the
///   sections we own
fn raw_toml_has_section(raw: &str, section: &str) -> bool {
    let target = format!("[{}]", section);
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed == target {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.transcription.engine, "whisper");
        assert_eq!(config.transcription.model, "small");
        assert_eq!(config.transcription.min_words, 3);
        assert_eq!(config.transcription.parakeet_binary, "parakeet");
        assert_eq!(config.transcription.parakeet_model, "tdt-ctc-110m");
        assert_eq!(config.transcription.parakeet_vocab, "vocab.txt");
        assert_eq!(config.diarization.engine, "auto");
        assert_eq!(config.summarization.engine, "auto");
        assert_eq!(config.search.engine, "builtin");
        assert!(!config.daily_notes.enabled);
        assert!(config.dictation.accumulate);
        assert!(config.call_detection.enabled);
        assert_eq!(config.watch.settle_delay_ms, 2000);
        assert!(!config.watch.extensions.is_empty());
        assert!(!config.recording.auto_call_intent);
        assert!(!config.recording.allow_degraded_call_capture);
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
        assert_eq!(config.transcription.min_words, 3);
        assert_eq!(config.diarization.engine, "auto");
        assert!(!config.daily_notes.enabled);
        assert!(config.dictation.accumulate);
    }

    #[test]
    fn default_language_is_none() {
        let config = Config::default();
        assert_eq!(config.transcription.language, None);
    }

    #[test]
    fn language_can_be_set_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
language = "es"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.language, Some("es".into()));
    }

    #[test]
    fn omitted_language_defaults_to_none() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "tiny"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.language, None);
    }

    #[test]
    fn invalid_toml_returns_defaults() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "this is not valid toml {{{").unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.model, "small");
        assert!(config.dictation.accumulate);
    }

    #[test]
    fn parakeet_config_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
engine = "parakeet"
parakeet_model = "tdt-600m"
parakeet_binary = "/usr/local/bin/parakeet"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.engine, "parakeet");
        assert_eq!(config.transcription.parakeet_model, "tdt-600m");
        assert_eq!(
            config.transcription.parakeet_binary,
            "/usr/local/bin/parakeet"
        );
        // Other fields should be defaults
        assert_eq!(config.transcription.model, "small");
        assert_eq!(config.transcription.min_words, 3);
    }

    #[test]
    fn omitted_engine_defaults_to_whisper() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "tiny"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.transcription.engine, "whisper");
        assert_eq!(config.transcription.parakeet_binary, "parakeet");
    }

    #[test]
    fn dictation_accumulate_can_be_disabled_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[dictation]
accumulate = false
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(!config.dictation.accumulate);
    }

    // ── Palette config + upgrade migration ────────────────────

    #[test]
    fn palette_default_is_enabled() {
        let config = Config::default();
        assert!(config.palette.shortcut_enabled);
        assert_eq!(config.palette.shortcut, "CmdOrCtrl+Shift+K");
    }

    #[test]
    fn raw_toml_has_section_matches_top_level_headers() {
        assert!(raw_toml_has_section("[palette]\nx = 1\n", "palette"));
        assert!(raw_toml_has_section("# header\n[palette]", "palette"));
        assert!(raw_toml_has_section(
            "[other]\nx=1\n\n[palette]\ny=2\n",
            "palette"
        ));
    }

    #[test]
    fn raw_toml_has_section_ignores_commented_headers() {
        assert!(!raw_toml_has_section("# [palette]\n", "palette"));
        assert!(!raw_toml_has_section("  # [palette]\n", "palette"));
    }

    #[test]
    fn raw_toml_has_section_rejects_non_matching_sections() {
        assert!(!raw_toml_has_section("[dictation]\n", "palette"));
        assert!(!raw_toml_has_section("[palette.inner]\n", "palette"));
    }

    #[test]
    fn fresh_install_keeps_palette_enabled() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        // No file exists — fresh install path.
        let config = Config::load_with_migrations_from(&config_path);
        assert!(
            config.palette.shortcut_enabled,
            "fresh install should default palette shortcut to ENABLED"
        );
        // And the migration should NOT have created a file out of thin air.
        // Fresh installs leave config creation to a later save() call.
        assert!(
            !config_path.exists(),
            "migration should not materialize a config file on fresh install"
        );
    }

    #[test]
    fn upgrade_disables_palette_when_section_missing() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "small"
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert!(
            !config.palette.shortcut_enabled,
            "upgrade path should default palette shortcut to DISABLED"
        );

        // The migration should have persisted its decision so the next
        // load is a no-op instead of running the same migration twice.
        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            raw_toml_has_section(&reloaded, "palette"),
            "migration should persist a [palette] section to disk"
        );
        assert!(
            reloaded.contains("shortcut_enabled = false"),
            "persisted migration must encode shortcut_enabled = false, got:\n{}",
            reloaded
        );

        // Second load must be a stable fixpoint.
        let second = Config::load_with_migrations_from(&config_path);
        assert!(!second.palette.shortcut_enabled);
    }

    #[test]
    fn upgrade_respects_explicit_palette_section() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "small"

[palette]
shortcut_enabled = true
shortcut = "CmdOrCtrl+Shift+K"
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert!(
            config.palette.shortcut_enabled,
            "explicit [palette] section must not be overridden by migration"
        );

        // And the on-disk file should be unchanged (no write storm).
        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(reloaded.contains("shortcut_enabled = true"));
    }

    #[test]
    fn upgrade_respects_user_disabled_palette_section() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[palette]
shortcut_enabled = false
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert!(!config.palette.shortcut_enabled);
    }
}
