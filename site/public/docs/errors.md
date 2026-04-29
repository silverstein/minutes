# Minutes error reference

> Generated file. Do not edit by hand.
> Source: crates/core thiserror definitions
> Last generated: 2026-04-29

This is the generated public catalog of stable Minutes core errors. It intentionally favors actionable, user-facing errors over generic wrapper variants.

- Visible actionable errors: 50
- Hidden low-signal wrappers: 15

# CaptureError

<a id="error-captureerror-devicenotfound-target-os-macos"></a>

## `CaptureError::DeviceNotFound`

Exact message:

> audio device not found — is BlackHole installed? Run: brew install blackhole-2ch

Platform condition: `target_os = "macos"`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-devicenotfound-target-os-macos

<a id="error-captureerror-devicenotfound-target-os-windows"></a>

## `CaptureError::DeviceNotFound`

Exact message:

> audio device not found — is VB-CABLE installed? See https://vb-audio.com/Cable/

Platform condition: `target_os = "windows"`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-devicenotfound-target-os-windows

<a id="error-captureerror-devicenotfound-not-any-target-os-macos-target-os-windows"></a>

## `CaptureError::DeviceNotFound`

Exact message:

> audio device not found — check your ALSA/PulseAudio configuration

Platform condition: `not(any(target_os = "macos", target_os = "windows"))`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-devicenotfound-not-any-target-os-macos-target-os-windows

<a id="error-captureerror-alreadyrecording"></a>

## `CaptureError::AlreadyRecording`

Exact message:

> already recording (PID: {0})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-alreadyrecording

<a id="error-captureerror-notrecording"></a>

## `CaptureError::NotRecording`

Exact message:

> no recording in progress

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-notrecording

<a id="error-captureerror-stalerecording"></a>

## `CaptureError::StaleRecording`

Exact message:

> stale recording found (PID {0} is dead)

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-stalerecording

<a id="error-captureerror-emptyrecording"></a>

## `CaptureError::EmptyRecording`

Exact message:

> recording produced empty audio (0 bytes)

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-captureerror-emptyrecording

# TranscribeError

<a id="error-transcribeerror-modelnotfound"></a>

## `TranscribeError::ModelNotFound`

Exact message:

> Transcription model not found. {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-modelnotfound

<a id="error-transcribeerror-modelloaderror"></a>

## `TranscribeError::ModelLoadError`

Exact message:

> failed to load whisper model: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-modelloaderror

<a id="error-transcribeerror-emptyaudio"></a>

## `TranscribeError::EmptyAudio`

Exact message:

> audio file is empty or has zero duration

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-emptyaudio

<a id="error-transcribeerror-unsupportedformat"></a>

## `TranscribeError::UnsupportedFormat`

Exact message:

> unsupported audio format: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-unsupportedformat

<a id="error-transcribeerror-emptytranscript"></a>

## `TranscribeError::EmptyTranscript`

Exact message:

> transcription produced no text (below {0} word minimum)

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-emptytranscript

<a id="error-transcribeerror-transcriptionfailed"></a>

## `TranscribeError::TranscriptionFailed`

Exact message:

> transcription failed: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-transcriptionfailed

<a id="error-transcribeerror-enginenotavailable"></a>

## `TranscribeError::EngineNotAvailable`

Exact message:

> engine '{0}' not compiled in — rebuild with: cargo build --features {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-enginenotavailable

<a id="error-transcribeerror-parakeetnotfound"></a>

## `TranscribeError::ParakeetNotFound`

Exact message:

> parakeet binary not found. Install parakeet.cpp and ensure `parakeet` is in PATH.

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-parakeetnotfound

<a id="error-transcribeerror-parakeetfailed"></a>

## `TranscribeError::ParakeetFailed`

Exact message:

> parakeet transcription failed: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-transcribeerror-parakeetfailed

# WatchError

<a id="error-watcherror-alreadyrunning"></a>

## `WatchError::AlreadyRunning`

Exact message:

> another watcher is already running (PID in {0})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-watcherror-alreadyrunning

<a id="error-watcherror-dirnotfound"></a>

## `WatchError::DirNotFound`

Exact message:

> watch directory does not exist: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-watcherror-dirnotfound

<a id="error-watcherror-moveerror"></a>

## `WatchError::MoveError`

Exact message:

> failed to move file to {0}: {1}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-watcherror-moveerror

<a id="error-watcherror-notifyerror"></a>

## `WatchError::NotifyError`

Exact message:

> file system watcher error: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-watcherror-notifyerror

# SearchError

<a id="error-searcherror-dirnotfound"></a>

## `SearchError::DirNotFound`

Exact message:

> search directory does not exist: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-searcherror-dirnotfound

<a id="error-searcherror-frontmatterparseerror"></a>

## `SearchError::FrontmatterParseError`

Exact message:

> failed to parse frontmatter in {0}: {1}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-searcherror-frontmatterparseerror

<a id="error-searcherror-index"></a>

## `SearchError::Index`

Exact message:

> search index error: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-searcherror-index

# ConfigError

<a id="error-configerror-parseerror"></a>

## `ConfigError::ParseError`

Exact message:

> failed to parse config file {0}: {1}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-configerror-parseerror

# MarkdownError

<a id="error-markdownerror-outputdirerror"></a>

## `MarkdownError::OutputDirError`

Exact message:

> output directory does not exist and could not be created: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-markdownerror-outputdirerror

<a id="error-markdownerror-serializationerror"></a>

## `MarkdownError::SerializationError`

Exact message:

> failed to serialize frontmatter: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-markdownerror-serializationerror

<a id="error-markdownerror-renamerefused"></a>

## `MarkdownError::RenameRefused`

Exact message:

> rename refused: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-markdownerror-renamerefused

# VaultError

<a id="error-vaulterror-notconfigured"></a>

## `VaultError::NotConfigured`

Exact message:

> vault not configured — run: minutes vault setup

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-notconfigured

<a id="error-vaulterror-vaultpathnotfound"></a>

## `VaultError::VaultPathNotFound`

Exact message:

> vault path not found: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-vaultpathnotfound

<a id="error-vaulterror-permissiondenied-target-os-macos"></a>

## `VaultError::PermissionDenied`

Exact message:

> permission denied: {0} — macOS requires Full Disk Access for ~/Documents/

Platform condition: `target_os = "macos"`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-permissiondenied-target-os-macos

<a id="error-vaulterror-permissiondenied-target-os-windows"></a>

## `VaultError::PermissionDenied`

Exact message:

> permission denied: {0} — Windows requires Developer Mode or admin for symlinks

Platform condition: `target_os = "windows"`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-permissiondenied-target-os-windows

<a id="error-vaulterror-permissiondenied-not-any-target-os-macos-target-os-windows"></a>

## `VaultError::PermissionDenied`

Exact message:

> permission denied: {0}

Platform condition: `not(any(target_os = "macos", target_os = "windows"))`

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-permissiondenied-not-any-target-os-macos-target-os-windows

<a id="error-vaulterror-existingdirectory"></a>

## `VaultError::ExistingDirectory`

Exact message:

> cannot create symlink — directory already exists: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-existingdirectory

<a id="error-vaulterror-symlinkfailed"></a>

## `VaultError::SymlinkFailed`

Exact message:

> symlink creation failed: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-symlinkfailed

<a id="error-vaulterror-copyfailed"></a>

## `VaultError::CopyFailed`

Exact message:

> vault copy failed for {0}: {1}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-copyfailed

<a id="error-vaulterror-brokensymlink"></a>

## `VaultError::BrokenSymlink`

Exact message:

> broken symlink at {0} (target: {1})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-vaulterror-brokensymlink

# PidError

<a id="error-piderror-alreadyrecording"></a>

## `PidError::AlreadyRecording`

Exact message:

> already recording (PID: {0})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-piderror-alreadyrecording

<a id="error-piderror-notrecording"></a>

## `PidError::NotRecording`

Exact message:

> no recording in progress

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-piderror-notrecording

<a id="error-piderror-stalepid"></a>

## `PidError::StalePid`

Exact message:

> stale PID file (process {0} is dead)

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-piderror-stalepid

# DictationError

<a id="error-dictationerror-recordingactive"></a>

## `DictationError::RecordingActive`

Exact message:

> recording in progress — stop recording before dictating

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-recordingactive

<a id="error-dictationerror-livetranscriptactive"></a>

## `DictationError::LiveTranscriptActive`

Exact message:

> live transcript in progress — stop it before dictating

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-livetranscriptactive

<a id="error-dictationerror-alreadyactive"></a>

## `DictationError::AlreadyActive`

Exact message:

> dictation already active (PID: {0})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-alreadyactive

<a id="error-dictationerror-clipboardfailed"></a>

## `DictationError::ClipboardFailed`

Exact message:

> clipboard write failed: {0}

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-clipboardfailed

<a id="error-dictationerror-accessibilitydenied"></a>

## `DictationError::AccessibilityDenied`

Exact message:

> accessibility permission required for auto-paste

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-accessibilitydenied

<a id="error-dictationerror-notactive"></a>

## `DictationError::NotActive`

Exact message:

> dictation not active

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-dictationerror-notactive

# LiveTranscriptError

<a id="error-livetranscripterror-recordingactive"></a>

## `LiveTranscriptError::RecordingActive`

Exact message:

> recording in progress — stop recording before starting live transcript

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-livetranscripterror-recordingactive

<a id="error-livetranscripterror-dictationactive"></a>

## `LiveTranscriptError::DictationActive`

Exact message:

> dictation in progress — stop dictation before starting live transcript

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-livetranscripterror-dictationactive

<a id="error-livetranscripterror-alreadyactive"></a>

## `LiveTranscriptError::AlreadyActive`

Exact message:

> live transcript already active (PID: {0})

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-livetranscripterror-alreadyactive

<a id="error-livetranscripterror-noactivesession"></a>

## `LiveTranscriptError::NoActiveSession`

Exact message:

> no live transcript session active

Source: `crates/core/src/error.rs`

Reference URL: https://useminutes.app/docs/errors#error-livetranscripterror-noactivesession

# GraphError

<a id="error-grapherror-dirnotfound"></a>

## `GraphError::DirNotFound`

Exact message:

> meetings directory does not exist: {0}

Source: `crates/core/src/graph.rs`

Reference URL: https://useminutes.app/docs/errors#error-grapherror-dirnotfound
