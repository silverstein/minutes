use thiserror::Error;

// ──────────────────────────────────────────────────────────────
// Per-module error enums, unified at crate level via MinutesError.
//
// Pattern:
//   CaptureError, TranscribeError, etc. → MinutesError via #[from]
//   CLI matches on MinutesError for user-facing messages.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum CaptureError {
    #[cfg(target_os = "macos")]
    #[error("audio device not found — is BlackHole installed? Run: brew install blackhole-2ch")]
    DeviceNotFound,

    #[cfg(target_os = "windows")]
    #[error("audio device not found — is VB-CABLE installed? See https://vb-audio.com/Cable/")]
    DeviceNotFound,

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[error("audio device not found — check your ALSA/PulseAudio configuration")]
    DeviceNotFound,

    #[error("already recording (PID: {0})")]
    AlreadyRecording(u32),

    #[error(
        "another Minutes process is starting or stopping capture; this process did not open a second microphone"
    )]
    CaptureOwnerBusy,

    #[error("no recording in progress")]
    NotRecording,

    #[error("stale recording found (PID {0} is dead)")]
    StaleRecording(u32),

    #[error("recording produced empty audio (0 bytes)")]
    EmptyRecording,

    #[error("audio I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum TranscribeError {
    #[error("Transcription model not found. {0}")]
    ModelNotFound(String),

    #[error("failed to load whisper model: {0}")]
    ModelLoadError(String),

    #[error(
        "whisper model at {path} looks truncated: file is {actual_mb:.0} MB but the {model_name} model should be at least {expected_min_mb:.0} MB. \
         A previous download was probably interrupted. Fix: rm \"{path}\" && minutes setup --model {model_name}"
    )]
    ModelTruncated {
        /// Display path of the on-disk model file.
        path: String,
        /// Model name as passed to `minutes setup` (e.g. `medium`, `large-v3`).
        model_name: String,
        /// Observed size of the file on disk, in MB.
        actual_mb: f64,
        /// Conservative lower bound from `expected_whisper_model_size_bytes`,
        /// in MB. Anything below this is treated as truncated.
        expected_min_mb: f64,
    },

    #[error("audio file is empty or has zero duration")]
    EmptyAudio,

    #[error("unsupported audio format: {0}")]
    UnsupportedFormat(String),

    #[error("transcription produced no text (below {0} word minimum)")]
    EmptyTranscript(usize),

    #[error("transcription failed: {0}")]
    TranscriptionFailed(String),

    #[error("engine '{0}' not compiled in — rebuild with: cargo build --features {0}")]
    EngineNotAvailable(String),

    #[error("parakeet binary not found. Install parakeet.cpp and ensure `parakeet` is in PATH.")]
    ParakeetNotFound,

    #[error("parakeet transcription failed: {0}")]
    ParakeetFailed(String),

    #[error(
        "native call capture cannot be transcribed: {reason}. \
         The .mov produced by macOS SCRecordingOutput decodes to ~2x source duration on this \
         container shape; transcription requires the sibling .voice.wav and .system.wav PCM \
         stems to be mixed via ffmpeg first."
    )]
    NativeCaptureStemMixUnavailable { reason: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("another watcher is already running (PID in {0})")]
    AlreadyRunning(String),

    #[error("watch directory does not exist: {0}")]
    DirNotFound(String),

    #[error("failed to move file to {0}: {1}")]
    MoveError(String, std::io::Error),

    #[error("file system watcher error: {0}")]
    NotifyError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("search directory does not exist: {0}")]
    DirNotFound(String),

    #[error("failed to parse frontmatter in {0}: {1}")]
    FrontmatterParseError(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("search index error: {0}")]
    Index(String),
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to parse config file {0}: {1}")]
    ParseError(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum MarkdownError {
    #[error("output directory does not exist and could not be created: {0}")]
    OutputDirError(String),

    #[error("failed to serialize frontmatter: {0}")]
    SerializationError(String),

    #[error("rename refused: {0}")]
    RenameRefused(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault not configured — run: minutes vault setup")]
    NotConfigured,

    #[error("vault path not found: {0}")]
    VaultPathNotFound(String),

    #[cfg(target_os = "macos")]
    #[error("permission denied: {0} — macOS requires Full Disk Access for ~/Documents/")]
    PermissionDenied(String),

    #[cfg(target_os = "windows")]
    #[error("permission denied: {0} — Windows requires Developer Mode or admin for symlinks")]
    PermissionDenied(String),

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("cannot create symlink — directory already exists: {0}")]
    ExistingDirectory(String),

    #[error("symlink creation failed: {0}")]
    SymlinkFailed(String),

    #[error("vault copy failed for {0}: {1}")]
    CopyFailed(String, std::io::Error),

    #[error("broken symlink at {0} (target: {1})")]
    BrokenSymlink(String, String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum PidError {
    #[error("already recording (PID: {0})")]
    AlreadyRecording(u32),

    #[error("no recording in progress")]
    NotRecording,

    #[error("stale PID file (process {0} is dead)")]
    StalePid(u32),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum DictationError {
    #[error("recording in progress — stop recording before dictating")]
    RecordingActive,

    #[error("live transcript in progress — stop it before dictating")]
    LiveTranscriptActive,

    #[error("dictation already active (PID: {0})")]
    AlreadyActive(u32),

    #[error("clipboard write failed: {0}")]
    ClipboardFailed(String),

    #[error("accessibility permission required for auto-paste")]
    AccessibilityDenied,

    #[error("{0}")]
    PermissionBlocked(String),

    #[error("dictation not active")]
    NotActive,

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum LiveTranscriptError {
    #[error("recording in progress — stop recording before starting live transcript")]
    RecordingActive,

    #[error("dictation in progress — stop dictation before starting live transcript")]
    DictationActive,

    #[error("live transcript already active (PID: {0})")]
    AlreadyActive(u32),

    #[error("{0}")]
    PermissionBlocked(String),

    #[error("no live transcript session active")]
    NoActiveSession,
}

#[derive(Debug, Error)]
pub enum TemplateError {
    #[error("template not found: {0}")]
    NotFound(String),

    #[error("invalid template at {path}: {message}")]
    Invalid { path: String, message: String },

    #[error("template at {path} uses field '{field}' not supported by this Minutes version (introduced in a later phase). Upgrade Minutes or remove the field.")]
    UnsupportedField { path: String, field: String },

    #[error("template at {path} has invalid slug '{slug}': must be lowercase alphanumeric with hyphens (e.g. 'standup', '1-on-1')")]
    InvalidSlug { path: String, slug: String },

    #[error("template at {path} has invalid version '{version}': must be semver (e.g. '1.0.0')")]
    InvalidVersion { path: String, version: String },

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Unified error type for the minutes-core crate.
/// CLI matches on this for user-facing error messages.
#[derive(Debug, Error)]
pub enum MinutesError {
    #[error(transparent)]
    Capture(#[from] CaptureError),

    #[error(transparent)]
    Transcribe(#[from] TranscribeError),

    #[error(transparent)]
    Watch(#[from] WatchError),

    #[error(transparent)]
    Search(#[from] SearchError),

    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error(transparent)]
    Markdown(#[from] MarkdownError),

    #[error(transparent)]
    Vault(#[from] VaultError),

    #[error(transparent)]
    Pid(#[from] PidError),

    #[error(transparent)]
    Dictation(#[from] DictationError),

    #[error(transparent)]
    LiveTranscript(#[from] LiveTranscriptError),

    #[error(transparent)]
    Template(#[from] TemplateError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MinutesError>;
