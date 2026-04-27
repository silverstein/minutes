use serde::{Deserialize, Serialize};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────────────────────
// Config loading precedence:
//   Compiled defaults → config file override → CLI flag override
//
// Config file is OPTIONAL. minutes works without one.
// ──────────────────────────────────────────────────────────────

/// Desktop-only fallback env var used when the Tauri app hydrates an
/// OpenAI-compatible gateway key from macOS Keychain for non-local endpoints.
pub const OPENAI_COMPATIBLE_DESKTOP_API_KEY_ENV: &str = "MINUTES_OPENAI_COMPATIBLE_API_KEY";

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
    pub privacy: PrivacyConfig,
    pub screen_context: ScreenContextConfig,
    pub desktop_context: DesktopContextConfig,
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
/// Both fresh installs and upgrades default `shortcut_enabled` to
/// `true`. The Tauri desktop app fires a one-shot system notification
/// on the first launch that registers the shortcut so users with a
/// real conflict (VS Code Delete Line, JetBrains Push, etc.) hear
/// about the new binding immediately and can disable it from the
/// Settings UI in one click. The first-run marker file lives at
/// `~/.minutes/palette_first_run_shown` and only commits after the
/// notification is dispatched successfully — see
/// `commands::maybe_show_palette_first_run_notice` for the details.
///
/// An earlier draft of this struct used a different design where
/// `Config::load_with_migrations` flipped `shortcut_enabled` to
/// `false` for upgraders. Dogfood feedback rejected that as
/// undiscoverable: opt-in via `config.toml` is invisible, and the
/// settings UI for the palette didn't exist yet. The current design
/// pairs default-on with a visible Settings UI surface and a
/// first-run notification.
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
        // Both fresh installs and upgrades use this value. The
        // upgrade migration in `load_with_migrations` only persists
        // the section to disk so it's discoverable in `config.toml`;
        // it does NOT flip the bool.
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
    /// Transcription engine: "whisper" (default), "parakeet", or
    /// "apple-speech" (experimental live-transcript-only path on macOS 26+).
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
    /// Maximum number of knowledge-graph phrases to pass via `--boost`.
    /// Set to 0 to disable phrase boosting. Default: off until tuned further.
    pub parakeet_boost_limit: usize,
    /// Score passed to parakeet.cpp `--boost-score` when boost phrases are active.
    pub parakeet_boost_score: f32,
    /// Enable parakeet.cpp fp16 inference on the GPU path.
    ///
    /// This lowers memory use, but on the current process-per-transcription
    /// runtime it can add noticeable cold-start latency because the model is
    /// cast to fp16 on each run.
    pub parakeet_fp16: bool,
    /// Enable the warm Parakeet example-server sidecar path.
    ///
    /// This is beta and remains opt-in until more real-world validation lands.
    pub parakeet_sidecar_enabled: bool,
    /// Clear the persistent parakeet fp16 blacklist before the next sidecar start.
    ///
    /// This is useful after upgrading the parakeet binary to a version that may
    /// have fixed prior fp16 startup crashes.
    pub parakeet_fp16_blacklist_reset: bool,
    /// SentencePiece vocab filename (resolved under model_path/parakeet/).
    ///
    /// If left at the default generic name, Minutes still prefers model-specific
    /// tokenizer files such as `tdt-ctc-110m.tokenizer.vocab` when they exist.
    pub parakeet_vocab: String,
}

pub const VALID_PARAKEET_MODELS: &[&str] = &["tdt-ctc-110m", "tdt-600m"];

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
    /// Correlation threshold (0.0–1.0) above which stem-based diarization
    /// collapses voice + system stems to a single speaker. The check
    /// assumes high cross-stem correlation means one person bleeding into
    /// both sources (self-monitor / headphone leak), but it misfires for
    /// open-speaker mic setups (Studio Display Mic, laptop mic, desk USB
    /// mic near speakers) where the mic acoustically picks up multi-
    /// speaker system audio from a Zoom/Meet call. Raise to 1.0 or higher
    /// to disable the collapse and rely on per-window energy attribution.
    /// Default 0.85 preserves historical behavior.
    pub stem_correlation_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SummarizationConfig {
    pub engine: String,
    pub agent_command: String,
    pub chunk_max_tokens: usize,
    pub ollama_url: String,
    pub ollama_model: String,
    pub openai_compatible_base_url: String,
    pub openai_compatible_model: String,
    pub openai_compatible_api_key_env: String,
    pub mistral_model: String,
    pub language: String,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DesktopContextConfig {
    pub enabled: bool,
    pub capture_window_titles: bool,
    pub capture_browser_context: bool,
    pub allowed_apps: Vec<String>,
    pub denied_apps: Vec<String>,
    pub allowed_domains: Vec<String>,
    pub denied_domains: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    pub hide_from_screen_share: bool,
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

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            hide_from_screen_share: true,
        }
    }
}

impl Default for DesktopContextConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            capture_window_titles: true,
            capture_browser_context: false,
            allowed_apps: vec![],
            denied_apps: vec![],
            allowed_domains: vec![],
            denied_domains: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CallDetectionConfig {
    pub enabled: bool,
    pub poll_interval_secs: u64,
    pub cooldown_minutes: u64,
    pub apps: Vec<String>,
    /// When a call that the detector started a recording for ends, show a
    /// countdown banner that auto-stops the recording. Default: false.
    /// Named for the behavior (an assistive prompt, not a silent hard stop);
    /// people often keep recording 30-90s past hangup for takeaways.
    pub stop_when_call_ends: bool,
    /// Seconds the user has to cancel auto-stop before it fires.
    /// Only meaningful when `stop_when_call_ends` is true. Default: 30.
    pub call_end_stop_countdown_secs: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentityConfig {
    pub name: Option<String>,
    /// Single primary email. Retained for backwards compatibility with
    /// existing `config.toml` files. New configs should prefer `emails`.
    pub email: Option<String>,
    /// All email addresses the user sends from. Folded onto the canonical
    /// person entity so calendar attendees arriving as
    /// `you@work.com`/`you@personal.com` don't spawn duplicate people.
    pub emails: Vec<String>,
    /// Alternate name forms (nicknames, formal variants) for the user.
    /// Example: name="Mat", aliases=["Mathieu", "Matthew"]. Used for the
    /// same fold as `emails` so `mathieu@x.com` → `Mathieu` → canonical
    /// `Mat`.
    pub aliases: Vec<String>,
}

impl IdentityConfig {
    /// Every string that should map to the user's canonical entity:
    /// the legacy `email`, all `emails`, and all `aliases`. Duplicates
    /// and empties are filtered. Case-insensitive de-duplication.
    pub fn all_user_aliases(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        let mut push = |s: &str| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return;
            }
            if seen.insert(trimmed.to_ascii_lowercase()) {
                out.push(trimmed.to_string());
            }
        };
        if let Some(email) = &self.email {
            push(email);
        }
        for email in &self.emails {
            push(email);
        }
        for alias in &self.aliases {
            push(alias);
        }
        out
    }
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
    /// Standalone live transcript backend selection.
    ///
    /// - `"inherit"` (default): follow `transcription.engine`
    /// - `"whisper"`: force Whisper for standalone live transcript
    /// - `"parakeet"`: force Parakeet for standalone live transcript
    /// - `"apple-speech"`: experimental macOS standalone-live-only path
    pub backend: String,
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
            backend: LIVE_TRANSCRIPT_BACKEND_INHERIT.into(),
            model: String::new(), // empty = use dictation model
            max_utterance_secs: 30,
            save_wav: true,
            shortcut_enabled: false,
            shortcut: "CmdOrCtrl+Shift+L".into(),
        }
    }
}

pub const LIVE_TRANSCRIPT_BACKEND_INHERIT: &str = "inherit";
pub const VALID_LIVE_TRANSCRIPT_BACKENDS: &[&str] = &[
    LIVE_TRANSCRIPT_BACKEND_INHERIT,
    "whisper",
    "parakeet",
    "apple-speech",
];

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
            stop_when_call_ends: false,
            call_end_stop_countdown_secs: 30,
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

fn config_base_dir_from(xdg_config_home: Option<OsString>, home: PathBuf) -> PathBuf {
    match xdg_config_home {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => home.join(".config"),
    }
}

fn config_base_dir() -> PathBuf {
    config_base_dir_from(std::env::var_os("XDG_CONFIG_HOME"), home_dir())
}

#[cfg(test)]
fn config_path_from(xdg_config_home: Option<OsString>, home: PathBuf) -> PathBuf {
    config_base_dir_from(xdg_config_home, home)
        .join("minutes")
        .join("config.toml")
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
            privacy: PrivacyConfig::default(),
            screen_context: ScreenContextConfig::default(),
            desktop_context: DesktopContextConfig::default(),
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
            parakeet_model: "tdt-600m".into(),
            parakeet_boost_limit: 0,
            parakeet_boost_score: 2.0,
            parakeet_fp16: true,
            parakeet_sidecar_enabled: false,
            parakeet_fp16_blacklist_reset: false,
            parakeet_vocab: "tdt-600m.tokenizer.vocab".into(),
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
            stem_correlation_threshold: 0.85,
        }
    }
}

impl Default for SummarizationConfig {
    fn default() -> Self {
        Self {
            engine: "none".into(),
            agent_command: "claude".into(),
            chunk_max_tokens: 4000,
            ollama_url: "http://localhost:11434".into(),
            ollama_model: "llama3.2".into(),
            openai_compatible_base_url: "http://localhost:11434/v1".into(),
            openai_compatible_model: "llama3.2".into(),
            openai_compatible_api_key_env: String::new(),
            mistral_model: "mistral-large-latest".into(),
            language: "auto".into(),
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
    /// Effective backend for the standalone live transcript path.
    ///
    /// `live_transcript.backend = "inherit"` follows `transcription.engine`,
    /// except for the legacy `transcription.engine = "apple-speech"` case,
    /// which older configs used to express the standalone-live-only Apple
    /// experiment. We keep honoring that value here so non-Tauri consumers
    /// preserve behavior even before the migration has rewritten the file.
    pub fn effective_live_transcript_backend(&self) -> &str {
        let backend = self.live_transcript.backend.trim();
        if backend.is_empty() || backend == LIVE_TRANSCRIPT_BACKEND_INHERIT {
            if self
                .transcription
                .engine
                .eq_ignore_ascii_case("apple-speech")
            {
                "apple-speech"
            } else {
                &self.transcription.engine
            }
        } else {
            &self.live_transcript.backend
        }
    }

    pub fn standalone_live_backend_setting(&self) -> &str {
        let backend = self.live_transcript.backend.trim();
        if backend.is_empty() {
            LIVE_TRANSCRIPT_BACKEND_INHERIT
        } else {
            &self.live_transcript.backend
        }
    }

    /// Standard config file location.
    pub fn config_path() -> PathBuf {
        config_base_dir().join("minutes").join("config.toml")
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
    /// - **Palette section persistence**: if the config file exists but
    ///   has no `[palette]` section, write the section out at its default
    ///   values so the user has a discoverable surface for the new
    ///   command palette. The compiled defaults enable the shortcut on
    ///   both fresh installs and upgrades. The Tauri desktop app fires a
    ///   one-shot system notification on the first launch that registers
    ///   the new shortcut so VS Code / JetBrains / Firefox users who
    ///   already have `⌘⇧K` bound aren't silently hijacked — they're
    ///   informed and can disable the shortcut from the settings UI in
    ///   one click.
    ///
    /// Note: an earlier draft of this migration force-disabled the
    /// shortcut on upgrade. That made the feature undiscoverable
    /// because the only way to enable it was to hand-edit `config.toml`,
    /// which (per dogfood feedback) nobody does. The current design
    /// prefers discoverability + a visible escape hatch over silent
    /// caution. The first-run notification + the settings UI panel are
    /// the consent mechanism, not opt-in defaults.
    ///
    /// Fresh installs (file does not exist) skip every migration and
    /// take the compiled defaults verbatim — the desktop app will
    /// create the config later via `cmd_set_setting` if the user
    /// changes anything.
    pub fn load_with_migrations() -> Self {
        let path = Self::config_path();
        Self::load_with_migrations_from(&path)
    }

    /// Testable form of [`Self::load_with_migrations`]. Reads from
    /// `path`, runs migrations, and writes the migrated config back if
    /// it changed.
    pub fn load_with_migrations_from(path: &Path) -> Self {
        let file_existed = path.exists();
        let raw_toml = if file_existed {
            std::fs::read_to_string(path).ok()
        } else {
            None
        };
        let raw_compat = raw_toml
            .as_deref()
            .map(inspect_raw_toml_compat)
            .unwrap_or_default();

        let mut config = Self::load_from(path);
        let mut migrated_toml: Option<String> = None;

        // Apple Speech migration: older configs overloaded
        // `transcription.engine = "apple-speech"` to mean "standalone live
        // transcript should try Apple Speech". That was always a product/model
        // mismatch because batch and recording-sidecar flows never actually
        // used Apple Speech. Normalize those existing configs to:
        //
        //   [transcription]
        //   engine = "whisper"
        //
        //   [live_transcript]
        //   backend = "apple-speech"
        //
        // We intentionally persist the migrated config back to disk so the
        // desktop app stops carrying the old overload forward after the first
        // upgraded launch.
        if file_existed
            && config
                .transcription
                .engine
                .eq_ignore_ascii_case("apple-speech")
            && raw_toml.as_deref().is_some_and(|raw| {
                !raw_toml_has_setting_in_section(raw, "live_transcript", "backend")
            })
        {
            config.transcription.engine = "whisper".into();
            config.live_transcript.backend = "apple-speech".into();
            migrated_toml = toml::to_string_pretty(&config).ok();
            tracing::info!(
                "live transcript backend migration: moved legacy apple-speech engine setting into [live_transcript].backend at {}",
                path.display()
            );
        }

        // Summarization upgrade safety:
        //
        // 1. Preserve the historical `"auto"` engine for existing sparse
        //    configs that never wrote `[summarization].engine`. This stays
        //    in-memory so we do not force-rewrite otherwise healthy files and
        //    drop comments / unknown keys just to preserve legacy behavior.
        // 2. Clear the desktop-only key env marker out of shared config files.
        if file_existed {
            if raw_compat.preserve_legacy_auto_summarization {
                tracing::info!(
                    "summarization migration: preserving legacy auto engine for sparse config at {}",
                    path.display()
                );
            }

            if raw_compat.clear_desktop_openai_compatible_env_marker {
                tracing::info!(
                    "summarization migration: clearing desktop-only key env marker from shared config at {}",
                    path.display()
                );
                migrated_toml = toml::to_string_pretty(&config).ok();
            }
        }

        // Palette section persistence: if the config file exists but
        // has no `[palette]` section, write the default section out
        // verbatim. We do NOT flip `shortcut_enabled` away from its
        // default — see the doc comment on `load_with_migrations` for
        // why opt-in-on-upgrade is the wrong default.
        //
        // The point of this branch is to make the section visible in
        // the user's `config.toml` so they can find it next time they
        // open the file, AND to give the desktop app's first-run
        // notification logic a stable place to know "the user has now
        // seen this section persisted." `toml::from_str` silently
        // fills missing sections with `Default`, so the parsed struct
        // alone cannot distinguish "user opted out" from "field never
        // seen" — only a text check on the raw TOML can.
        if file_existed && migrated_toml.is_none() {
            if let Some(raw) = raw_toml.as_deref() {
                if !raw_toml_has_section(raw, "palette") {
                    migrated_toml = Some(append_palette_section(raw, &config.palette));
                    tracing::info!(
                        "palette migration: persisting [palette] section in existing config at {}",
                        path.display()
                    );
                }
            }
        }

        if let Some(migrated_toml) = migrated_toml {
            if let Err(e) = std::fs::write(path, migrated_toml) {
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
                Ok(mut config) => {
                    apply_raw_toml_compat(&mut config, inspect_raw_toml_compat(&contents));
                    config
                }
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

#[derive(Debug, Clone, Copy, Default)]
struct RawTomlCompat {
    preserve_legacy_auto_summarization: bool,
    clear_desktop_openai_compatible_env_marker: bool,
}

fn inspect_raw_toml_compat(raw: &str) -> RawTomlCompat {
    RawTomlCompat {
        preserve_legacy_auto_summarization: !raw_toml_has_setting_in_section(
            raw,
            "summarization",
            "engine",
        ),
        clear_desktop_openai_compatible_env_marker: raw_toml_setting_equals_in_section(
            raw,
            "summarization",
            "openai_compatible_api_key_env",
            OPENAI_COMPATIBLE_DESKTOP_API_KEY_ENV,
        ),
    }
}

fn apply_raw_toml_compat(config: &mut Config, compat: RawTomlCompat) {
    if compat.preserve_legacy_auto_summarization {
        config.summarization.engine = "auto".into();
    }
    if compat.clear_desktop_openai_compatible_env_marker {
        config.summarization.openai_compatible_api_key_env.clear();
    }
}

pub fn openai_compatible_base_url_is_local(base_url: &str) -> bool {
    let trimmed = base_url.trim();
    if trimmed.is_empty() {
        return false;
    }

    let without_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    let authority = without_scheme.split('/').next().unwrap_or(without_scheme);
    let host_port = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped)
    } else {
        host_port.split(':').next().unwrap_or(host_port)
    };

    matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "0.0.0.0" | "::1"
    )
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

fn raw_toml_has_setting_in_section(raw: &str, section: &str, key: &str) -> bool {
    let target = format!("[{}]", section);
    let key_prefix = format!("{} =", key);
    let mut in_section = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            in_section = trimmed == target;
            continue;
        }
        if in_section && trimmed.starts_with(&key_prefix) {
            return true;
        }
    }
    false
}

fn raw_toml_setting_equals_in_section(raw: &str, section: &str, key: &str, expected: &str) -> bool {
    let target = format!("[{}]", section);
    let key_prefix = format!("{} =", key);
    let expected_value = toml::Value::String(expected.to_string()).to_string();
    let mut in_section = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if trimmed.starts_with('[') {
            in_section = trimmed == target;
            continue;
        }
        if in_section && trimmed.starts_with(&key_prefix) {
            return trimmed[key_prefix.len()..].trim() == expected_value;
        }
    }
    false
}

fn append_palette_section(raw: &str, palette: &PaletteConfig) -> String {
    let mut output = raw.trim_end_matches('\n').to_string();
    if !output.is_empty() {
        output.push_str("\n\n");
    }
    output.push_str("[palette]\n");
    output.push_str(&format!(
        "shortcut_enabled = {}\n",
        palette.shortcut_enabled
    ));
    output.push_str(&format!(
        "shortcut = {}\n",
        toml::Value::String(palette.shortcut.clone())
    ));
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_config_is_valid() {
        let config = Config::default();
        assert_eq!(config.transcription.engine, "whisper");
        assert_eq!(
            config.live_transcript.backend,
            LIVE_TRANSCRIPT_BACKEND_INHERIT
        );
        assert_eq!(config.transcription.model, "small");
        assert_eq!(config.transcription.min_words, 3);
        assert_eq!(config.transcription.parakeet_binary, "parakeet");
        assert_eq!(config.transcription.parakeet_model, "tdt-600m");
        assert_eq!(config.transcription.parakeet_boost_limit, 0);
        assert_eq!(config.transcription.parakeet_boost_score, 2.0);
        assert!(config.transcription.parakeet_fp16);
        assert!(!config.transcription.parakeet_sidecar_enabled);
        assert_eq!(
            config.transcription.parakeet_vocab,
            "tdt-600m.tokenizer.vocab"
        );
        assert_eq!(config.diarization.engine, "auto");
        assert_eq!(config.summarization.engine, "none");
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
    fn config_path_falls_back_to_home_dot_config_when_xdg_unset() {
        let home = PathBuf::from("/tmp/test-home");
        let path = config_path_from(None, home.clone());

        assert_eq!(path, home.join(".config/minutes/config.toml"));
    }

    #[test]
    fn config_path_uses_xdg_config_home_when_set() {
        let path = config_path_from(
            Some(OsString::from("/tmp/test-config")),
            PathBuf::from("/tmp/test-home"),
        );

        assert_eq!(path, PathBuf::from("/tmp/test-config/minutes/config.toml"));
    }

    #[test]
    fn config_path_falls_back_when_xdg_config_home_is_empty() {
        let home = PathBuf::from("/tmp/test-home");
        let path = config_path_from(Some(OsString::new()), home.clone());

        assert_eq!(path, home.join(".config/minutes/config.toml"));
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
        assert!(!config.transcription.parakeet_sidecar_enabled);
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
    fn effective_live_transcript_backend_inherits_batch_engine_by_default() {
        let mut config = Config::default();
        config.transcription.engine = "parakeet".into();

        assert_eq!(config.standalone_live_backend_setting(), "inherit");
        assert_eq!(config.effective_live_transcript_backend(), "parakeet");
    }

    #[test]
    fn effective_live_transcript_backend_preserves_legacy_apple_engine_configs() {
        let mut config = Config::default();
        config.transcription.engine = "apple-speech".into();

        assert_eq!(config.standalone_live_backend_setting(), "inherit");
        assert_eq!(config.effective_live_transcript_backend(), "apple-speech");
    }

    #[test]
    fn live_transcript_backend_can_be_set_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
engine = "whisper"

[live_transcript]
backend = "apple-speech"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.live_transcript.backend, "apple-speech");
        assert_eq!(config.effective_live_transcript_backend(), "apple-speech");
        assert_eq!(config.transcription.engine, "whisper");
    }

    #[test]
    fn parakeet_sidecar_flag_can_be_enabled_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
parakeet_sidecar_enabled = true
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(config.transcription.parakeet_sidecar_enabled);
    }

    #[test]
    fn parakeet_fp16_blacklist_reset_flag_can_be_enabled_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
parakeet_fp16_blacklist_reset = true
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(config.transcription.parakeet_fp16_blacklist_reset);
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

    // ── Call detection: stop-when-call-ends opt-in ────────────

    #[test]
    fn stop_when_call_ends_is_off_by_default() {
        let config = Config::default();
        assert!(!config.call_detection.stop_when_call_ends);
        assert_eq!(config.call_detection.call_end_stop_countdown_secs, 30);
    }

    #[test]
    fn stop_when_call_ends_round_trips_through_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[call_detection]
enabled = true
stop_when_call_ends = true
call_end_stop_countdown_secs = 45
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(config.call_detection.stop_when_call_ends);
        assert_eq!(config.call_detection.call_end_stop_countdown_secs, 45);
        // Sibling fields still populated from defaults.
        assert_eq!(config.call_detection.poll_interval_secs, 1);
        assert!(!config.call_detection.apps.is_empty());
    }

    #[test]
    fn stop_when_call_ends_omitted_keeps_default_off() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[call_detection]
enabled = true
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(!config.call_detection.stop_when_call_ends);
        assert_eq!(config.call_detection.call_end_stop_countdown_secs, 30);
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
    fn raw_toml_has_setting_in_section_matches_exact_key() {
        let raw = r#"
[live_transcript]
backend = "apple-speech"
shortcut = "CmdOrCtrl+Shift+L"
"#;

        assert!(raw_toml_has_setting_in_section(
            raw,
            "live_transcript",
            "backend"
        ));
        assert!(!raw_toml_has_setting_in_section(
            raw,
            "live_transcript",
            "missing"
        ));
    }

    #[test]
    fn append_palette_section_preserves_existing_text() {
        let raw = "# keep this comment\nunknown_key = 7\n";
        let appended = append_palette_section(raw, &PaletteConfig::default());

        assert!(appended.starts_with("# keep this comment\nunknown_key = 7\n"));
        assert!(raw_toml_has_section(&appended, "palette"));
        assert!(appended.contains("shortcut_enabled = true"));
        assert!(appended.contains("shortcut = \"CmdOrCtrl+Shift+K\""));
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
    fn upgrade_persists_palette_section_at_default_enabled() {
        // Existing config without a [palette] section: the migration
        // writes the section out at the compiled default, which is
        // ENABLED. Discoverability beats silent opt-out. The desktop
        // app's first-run notification (registered separately on the
        // first launch that sees the migration ran) gives users an
        // explicit consent surface.
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
            config.palette.shortcut_enabled,
            "upgrade path should keep palette shortcut ENABLED at the default"
        );

        // The migration should have persisted the section to disk so
        // the user can find it next time they open the file AND so
        // the next load is a stable fixpoint.
        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(
            raw_toml_has_section(&reloaded, "palette"),
            "migration should persist a [palette] section to disk"
        );
        assert!(
            reloaded.contains("[transcription]\nmodel = \"small\""),
            "migration should preserve existing config text, got:\n{}",
            reloaded
        );
        assert!(
            reloaded.contains("shortcut_enabled = true"),
            "persisted migration must encode shortcut_enabled = true, got:\n{}",
            reloaded
        );

        // Second load must be a stable fixpoint.
        let second = Config::load_with_migrations_from(&config_path);
        assert!(second.palette.shortcut_enabled);
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

    #[test]
    fn upgrade_preserves_comments_and_unknown_keys_when_adding_palette() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            "# top comment\nmystery = \"keep-me\"\n\n[transcription]\nmodel = \"small\"\n",
        )
        .unwrap();

        let _ = Config::load_with_migrations_from(&config_path);
        let reloaded = std::fs::read_to_string(&config_path).unwrap();

        assert!(reloaded.contains("# top comment"));
        assert!(reloaded.contains("mystery = \"keep-me\""));
        assert!(raw_toml_has_section(&reloaded, "palette"));
    }

    #[test]
    fn upgrade_migrates_legacy_apple_engine_into_live_backend() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
engine = "apple-speech"
model = "small"
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert_eq!(config.transcription.engine, "whisper");
        assert_eq!(config.live_transcript.backend, "apple-speech");
        assert_eq!(config.effective_live_transcript_backend(), "apple-speech");

        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(reloaded.contains("engine = \"whisper\""));
        assert!(reloaded.contains("[live_transcript]"));
        assert!(reloaded.contains("backend = \"apple-speech\""));
    }

    #[test]
    fn upgrade_does_not_override_explicit_live_backend_setting() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
engine = "apple-speech"

[live_transcript]
backend = "whisper"
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert_eq!(config.transcription.engine, "apple-speech");
        assert_eq!(config.live_transcript.backend, "whisper");
        assert_eq!(config.effective_live_transcript_backend(), "whisper");
    }

    #[test]
    fn upgrade_preserves_legacy_auto_summarization_when_engine_is_missing() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[transcription]
model = "small"

[palette]
shortcut_enabled = true
"#,
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert_eq!(config.summarization.engine, "auto");

        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(reloaded.contains("[transcription]\nmodel = \"small\""));
        assert!(reloaded.contains("[palette]\nshortcut_enabled = true"));
        assert!(!reloaded.contains("[summarization]"));
    }

    #[test]
    fn load_from_preserves_legacy_auto_summarization_when_engine_is_missing() {
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

        let config = Config::load_from(&config_path);
        assert_eq!(config.summarization.engine, "auto");
    }

    #[test]
    fn upgrade_clears_desktop_only_openai_compatible_env_marker() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[summarization]
engine = "openai-compatible"
openai_compatible_base_url = "https://openrouter.ai/api/v1"
openai_compatible_model = "openai/gpt-4o-mini"
openai_compatible_api_key_env = "{}"
"#,
                OPENAI_COMPATIBLE_DESKTOP_API_KEY_ENV
            ),
        )
        .unwrap();

        let config = Config::load_with_migrations_from(&config_path);
        assert!(config
            .summarization
            .openai_compatible_api_key_env
            .is_empty());

        let reloaded = std::fs::read_to_string(&config_path).unwrap();
        assert!(reloaded.contains("openai_compatible_api_key_env = \"\""));
    }

    #[test]
    fn load_from_clears_desktop_only_openai_compatible_env_marker() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            format!(
                r#"
[summarization]
engine = "openai-compatible"
openai_compatible_api_key_env = "{}"
"#,
                OPENAI_COMPATIBLE_DESKTOP_API_KEY_ENV
            ),
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert!(config
            .summarization
            .openai_compatible_api_key_env
            .is_empty());
    }

    #[test]
    fn openai_compatible_base_url_detects_local_hosts() {
        assert!(openai_compatible_base_url_is_local(
            "http://localhost:11434/v1"
        ));
        assert!(openai_compatible_base_url_is_local(
            "http://127.0.0.1:11434/v1"
        ));
        assert!(openai_compatible_base_url_is_local("http://[::1]:11434/v1"));
        assert!(!openai_compatible_base_url_is_local(
            "https://openrouter.ai/api/v1"
        ));
    }

    #[test]
    fn summarization_language_defaults_to_auto() {
        let config = Config::default();
        assert_eq!(config.summarization.language, "auto");
    }

    #[test]
    fn summarization_language_can_be_set_from_toml() {
        let dir = TempDir::new().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(
            &config_path,
            r#"
[summarization]
language = "fr"
"#,
        )
        .unwrap();

        let config = Config::load_from(&config_path);
        assert_eq!(config.summarization.language, "fr");
    }
}
