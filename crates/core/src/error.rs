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
    #[error(
        "Whisper model not found. {0}\n\nTo fix this, run:\n\n    minutes setup --model tiny\n"
    )]
    ModelNotFound(String),

    #[error("failed to load whisper model: {0}")]
    ModelLoadError(String),

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

    #[error("no live transcript session active")]
    NoActiveSession,
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

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, MinutesError>;
