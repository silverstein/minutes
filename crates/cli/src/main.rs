use anyhow::Result;
use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use minutes_core::apple_speech::{
    self, AppleSpeechBenchmarkArtifactPaths, AppleSpeechBenchmarkRequest,
};
use minutes_core::autoresearch::{
    self, DecodeHintEvalArtifactPaths, DecodeHintEvalComparisonArtifactPaths,
    DecodeHintEvalComparisonRequest, DecodeHintEvalOptions, DecodeHintEvalRequest,
};
use minutes_core::capture::RecordingIntent;
use minutes_core::config::{ConsentMode, VALID_PARAKEET_MODELS};
use minutes_core::markdown::ConsentBasis;
use minutes_core::parakeet;
use minutes_core::{CaptureMode, Config, ContentType};
use serde::{Deserialize, Serialize};

mod dashboard;
mod demo_data;
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Bundled native Silero VAD weights for parakeet.cpp's `--vad` path.
const PARAKEET_NATIVE_VAD_WEIGHTS: &[u8] =
    include_bytes!("../assets/parakeet/silero_vad_v5.safetensors");

#[derive(Serialize)]
struct AutomationRunRecord {
    kind: String,
    status: String,
    output_path: String,
    delivery_target: Option<String>,
    delivery_payload_path: Option<String>,
    generated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonMeta {
    schema_version: u32,
    generated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct JsonEnvelope<T: Serialize> {
    ok: bool,
    command: String,
    data: T,
    meta: JsonMeta,
}

#[derive(Serialize)]
struct TranscribeSegmentOutput {
    start: f64,
    end: f64,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    speaker: Option<String>,
}

#[derive(Serialize)]
struct TranscribeOutput {
    text: String,
    language: String,
    segments: Vec<TranscribeSegmentOutput>,
    duration_ms: u64,
}

#[derive(Serialize)]
struct ContextSummaryOutput {
    session: Option<minutes_core::context_store::ContextSession>,
    links: Vec<minutes_core::context_store::ContextLink>,
    events: Vec<minutes_core::context_store::ContextEvent>,
    top_apps: Vec<ContextCount>,
    top_windows: Vec<ContextCount>,
    window: ContextWindow,
}

#[derive(Serialize)]
struct ContextSearchOutput {
    results: Vec<minutes_core::context_store::ContextEvent>,
}

#[derive(Serialize)]
struct ContextMomentOutput {
    session: Option<minutes_core::context_store::ContextSession>,
    links: Vec<minutes_core::context_store::ContextLink>,
    events: Vec<minutes_core::context_store::ContextEvent>,
    window: ContextWindow,
}

#[derive(Serialize)]
struct ContextWindow {
    start: String,
    end: String,
}

#[derive(Serialize)]
struct ContextCount {
    name: String,
    count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg(feature = "parakeet")]
struct ParakeetHelperEnvelope<T: Serialize> {
    ok: bool,
    command: String,
    #[serde(flatten)]
    transcript: T,
    meta: JsonMeta,
}

fn json_meta() -> JsonMeta {
    JsonMeta {
        schema_version: 1,
        generated_at: Local::now().to_rfc3339(),
    }
}

fn json_envelope<T: Serialize>(command: &str, data: T) -> JsonEnvelope<T> {
    JsonEnvelope {
        ok: true,
        command: command.into(),
        data,
        meta: json_meta(),
    }
}

#[cfg(feature = "parakeet")]
fn parakeet_helper_envelope<T: Serialize>(
    command: &str,
    transcript: T,
) -> ParakeetHelperEnvelope<T> {
    ParakeetHelperEnvelope {
        ok: true,
        command: command.into(),
        transcript,
        meta: json_meta(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InterruptAction {
    Continue,
    ForceExit(i32),
}

fn handle_graceful_interrupt_with_shutdown(
    stop_flag: &std::sync::atomic::AtomicBool,
    first_message: &str,
    shutdown: impl Fn(),
) -> InterruptAction {
    use std::sync::atomic::Ordering;

    shutdown();
    if stop_flag.load(Ordering::Relaxed) {
        eprintln!("\nForce quit.");
        InterruptAction::ForceExit(1)
    } else {
        eprintln!("\n{}", first_message);
        stop_flag.store(true, Ordering::Relaxed);
        InterruptAction::Continue
    }
}

fn handle_graceful_interrupt(
    stop_flag: &std::sync::atomic::AtomicBool,
    first_message: &str,
) -> InterruptAction {
    handle_graceful_interrupt_with_shutdown(stop_flag, first_message, || {
        minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
    })
}

fn install_parakeet_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
        previous(panic_info);
    }));
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecordingConsent {
    basis: ConsentBasis,
    notice: Option<String>,
    reminder: Option<String>,
    warning: Option<String>,
}

fn parse_recording_consent_basis(raw: &str) -> Result<ConsentBasis> {
    raw.parse::<ConsentBasis>()
        .map_err(|error| anyhow::anyhow!("{}", error))
}

fn default_recording_consent_basis(config: &Config) -> Result<Option<ConsentBasis>> {
    config
        .consent
        .default_basis
        .as_deref()
        .map(parse_recording_consent_basis)
        .transpose()
}

fn prepare_recording_consent(
    config: &Config,
    consent_arg: Option<&str>,
    consent_notice: Option<&str>,
    stdin_is_tty: bool,
    prompt_for_consent: impl FnOnce() -> Result<bool>,
) -> Result<RecordingConsent> {
    let resolved_basis = if let Some(raw_basis) = consent_arg {
        Some(parse_recording_consent_basis(raw_basis)?)
    } else {
        default_recording_consent_basis(config)?
    };
    let explicit_notice = consent_notice
        .map(str::trim)
        .filter(|notice| !notice.is_empty())
        .map(str::to_string);

    match config.consent.mode {
        ConsentMode::Off => Ok(RecordingConsent {
            basis: resolved_basis.unwrap_or(ConsentBasis::Unattested),
            notice: explicit_notice,
            reminder: None,
            warning: None,
        }),
        ConsentMode::Remind => Ok(RecordingConsent {
            basis: resolved_basis.unwrap_or(ConsentBasis::Unattested),
            notice: explicit_notice,
            reminder: Some(config.consent.disclosure_script.clone()),
            warning: None,
        }),
        ConsentMode::Require if !stdin_is_tty => Ok(RecordingConsent {
            basis: resolved_basis.unwrap_or(ConsentBasis::Unattested),
            notice: explicit_notice,
            reminder: Some(config.consent.disclosure_script.clone()),
            warning: Some(
                "consent gate skipped: non-interactive session; recording as unattested".into(),
            ),
        }),
        ConsentMode::Require => {
            if let Some(basis) = resolved_basis {
                return Ok(RecordingConsent {
                    basis,
                    notice: explicit_notice,
                    reminder: None,
                    warning: None,
                });
            }
            if prompt_for_consent()? {
                Ok(RecordingConsent {
                    basis: ConsentBasis::VerbalAllParties,
                    notice: explicit_notice,
                    reminder: None,
                    warning: None,
                })
            } else {
                anyhow::bail!(
                    "Recording not started. Pass --consent <basis> or set [consent] mode = \"remind\" to use a reminder instead."
                );
            }
        }
    }
}

fn prompt_for_recording_consent() -> Result<bool> {
    eprint!("Has everyone present been notified and do they consent? [y/N] ");
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    Ok(matches!(
        input.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

/// minutes — conversation memory for AI assistants.
/// Every meeting, every idea, every voice note — searchable by your AI.
#[derive(Parser)]
#[command(
    name = "minutes",
    version,
    long_version = concat!(
        env!("CARGO_PKG_VERSION"),
        "\nhttps://github.com/silverstein/minutes/releases/tag/v",
        env!("CARGO_PKG_VERSION"),
    ),
    about,
    long_about = None,
)]
struct Cli {
    /// Enable verbose output (debug logs to stderr)
    #[arg(short, long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start recording audio (foreground process, Ctrl-C or `minutes stop` to finish)
    Record {
        /// Optional title for this recording
        #[arg(short, long)]
        title: Option<String>,

        /// Pre-meeting context (what this meeting is about)
        #[arg(short, long)]
        context: Option<String>,

        /// Live capture mode: meeting or quick-thought
        #[arg(long, default_value = "meeting", value_parser = ["meeting", "quick-thought"])]
        mode: String,

        /// Recording intent: auto, memo, room, or call.
        #[arg(long, default_value = "auto", value_parser = ["auto", "memo", "room", "call"])]
        intent: String,

        /// Allow Minutes to continue with a mic-only capture even if a call
        /// is detected and no system-audio route is configured.
        #[arg(long)]
        allow_degraded: bool,

        /// Skip the system-audio readiness probe for this run. Requires a
        /// reason, which is written into recording_health.
        #[arg(long, value_name = "REASON")]
        skip_audio_probe: Option<String>,

        /// Consent basis to stamp into meeting frontmatter.
        /// Values: verbal_all_parties, notice_in_invite, recorded_disclosed, na, unattested.
        #[arg(long, value_name = "BASIS")]
        consent: Option<String>,

        /// Exact disclosure text to stamp into meeting frontmatter.
        #[arg(long, value_name = "TEXT")]
        consent_notice: Option<String>,

        /// Transcription language (e.g. "en", "ur", "es"). Overrides config.toml setting.
        #[arg(short, long)]
        language: Option<String>,

        /// Audio input device name. Use `minutes devices` to list available devices.
        /// Overrides the [recording] device setting in config.toml.
        #[arg(short = 'D', long)]
        device: Option<String>,

        /// Capture source (repeatable). Specify two for multi-source capture.
        /// Example: --source "Yeti Nano" --source "BlackHole 2ch"
        #[arg(long)]
        source: Vec<String>,

        /// Shorthand for --intent call with auto-detected system audio device.
        #[arg(long)]
        call: bool,

        /// Skip live recording — use an existing WAV file as mock recording
        /// output and process it with full diagnostic logging.
        #[arg(long, value_name = "WAV_FILE")]
        diagnose: Option<PathBuf>,

        /// Start with the microphone muted. System audio still captures.
        /// Useful for passive attendance (webinars, all-hands). Toggle
        /// mid-recording with `minutes mic-toggle`.
        #[arg(long)]
        mute_mic: bool,

        /// Template slug to apply to summarization (e.g. "standup", "1-on-1").
        /// Use `minutes template list` to see available templates.
        #[arg(long)]
        template: Option<String>,
    },

    /// Toggle microphone mute for an active dual-source recording. System
    /// audio continues capturing; only the mic stream is silenced.
    MicToggle {
        /// Force a specific state instead of toggling. Use "on" to mute
        /// or "off" to unmute; omit to flip the current state.
        #[arg(long, value_parser = ["on", "off"])]
        state: Option<String>,
    },

    /// Add a note to the current recording
    Note {
        /// The note text
        text: String,

        /// Annotate an existing meeting file instead of the current recording
        #[arg(short, long)]
        meeting: Option<PathBuf>,
    },

    /// Stop recording and process the audio
    Stop,

    /// Start or stop a no-capture sensitive meeting
    Sensitive {
        #[command(subcommand)]
        action: SensitiveAction,
    },

    /// Keep a recording alive (reset auto-stop timers)
    Extend,

    /// Hidden worker that processes queued jobs.
    #[command(hide = true)]
    ProcessQueue,

    /// Hidden structured Parakeet helper used by Minutes internals.
    #[command(hide = true)]
    ParakeetHelper {
        #[arg(long)]
        binary: String,
        #[arg(long)]
        model_path: PathBuf,
        #[arg(long)]
        audio_path: PathBuf,
        #[arg(long)]
        vocab_path: PathBuf,
        #[arg(long)]
        model_id: String,
        #[arg(long, default_value_t = false)]
        gpu: bool,
        /// Run parakeet in fp16 mode. Mirrors the `--fp16` flag forwarded by
        /// `transcribe::transcribe_with_parakeet` when
        /// `transcription.parakeet_fp16` is enabled — without this flag the
        /// helper invocation fails clap parsing every utterance and the
        /// caller silently falls back to spawning parakeet directly. See
        /// issue #163.
        #[arg(long, default_value_t = false)]
        fp16: bool,
        #[arg(long)]
        vad_path: Option<PathBuf>,
        #[arg(long, default_value_t = 0.5)]
        vad_threshold: f32,
    },

    /// Hidden Parakeet benchmark for helper-vs-direct comparisons.
    #[command(hide = true)]
    ParakeetBenchmark {
        #[arg(long)]
        binary: String,
        #[arg(long)]
        model_path: PathBuf,
        #[arg(long)]
        audio_path: PathBuf,
        #[arg(long)]
        vocab_path: PathBuf,
        #[arg(long)]
        model_id: String,
        #[arg(long, default_value_t = false)]
        gpu: bool,
        #[arg(long)]
        vad_path: Option<PathBuf>,
        #[arg(long, default_value_t = 0.5)]
        vad_threshold: f32,
    },

    /// Hidden preflight for call-aware recording start decisions.
    #[command(hide = true)]
    PreflightRecord {
        #[arg(long, default_value = "meeting", value_parser = ["meeting", "quick-thought"])]
        mode: String,

        #[arg(long, default_value = "auto", value_parser = ["auto", "memo", "room", "call"])]
        intent: String,

        #[arg(long)]
        allow_degraded: bool,

        #[arg(long)]
        json: bool,
    },

    /// Experimental local-first research loops for maintainers.
    #[command(hide = true)]
    Autoresearch {
        #[command(subcommand)]
        action: AutoresearchAction,
    },

    /// Evaluate Apple's SpeechAnalyzer stack on macOS.
    AppleSpeech {
        #[command(subcommand)]
        action: AppleSpeechAction,
    },

    /// Print Minutes CLI capabilities as JSON for MCP feature detection.
    ///
    /// Emits a stable schema describing what this CLI build supports. The
    /// MCP server probes this at boot (see #183 phase 2) and uses the
    /// feature flags to decide which tools to expose without comparing
    /// version strings.
    Capabilities {
        /// Output raw JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },

    /// Check if a recording is in progress
    Status,

    /// Inspect background processing jobs
    Jobs {
        /// Include completed and failed jobs
        #[arg(long)]
        all: bool,

        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,

        /// Maximum number of jobs to return
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },

    /// Show effective Minutes paths from the loaded config
    Paths {
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },

    /// Show raw-audio storage and retention policy
    Storage {
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },

    /// Preview or apply raw-audio cleanup
    Cleanup {
        /// Delete cleanup candidates. Without this flag, cleanup is preview-only.
        #[arg(long)]
        apply: bool,

        /// Override successful-audio retention window for this run (for example: 14d, 30d)
        #[arg(long, value_name = "DURATION")]
        older_than: Option<String>,

        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },

    /// Search meeting transcripts and voice memos
    Search {
        /// Text to search for
        query: String,

        /// Filter by type: meeting or memo
        #[arg(short = 't', long)]
        content_type: Option<String>,

        /// Filter by date (ISO format, e.g., 2026-03-17)
        #[arg(short, long)]
        since: Option<String>,

        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Return structured intent records instead of prose snippets
        #[arg(long)]
        intents_only: bool,

        /// Filter structured intents by kind
        #[arg(long, value_parser = ["action-item", "decision", "open-question", "commitment"])]
        intent_kind: Option<String>,

        /// Filter structured intents by owner / person
        #[arg(long)]
        owner: Option<String>,

        /// Output format: text (human-readable) or json (one JSON object per line)
        #[arg(long, default_value = "text", value_parser = ["text", "json"])]
        format: String,

        /// Force a full re-walk + reindex before searching. Catches edge cases
        /// where mtime alone misses a content change (e.g., editor wrote with
        /// the same mtime). Slower; default Auto is usually enough.
        #[arg(long, conflicts_with = "no_sync")]
        sync: bool,

        /// Skip filesystem sync entirely; query the index as-is. Useful for
        /// piped or scripted CLI calls where freshness doesn't matter and
        /// every millisecond counts.
        #[arg(long, conflicts_with = "sync")]
        no_sync: bool,

        /// Include meetings designated `sensitivity: restricted` (excluded
        /// by default). The override is recorded on the event bus.
        #[arg(long)]
        include_restricted: bool,
    },

    /// Show open action items across all meetings
    Actions {
        /// Filter by assignee name
        #[arg(short, long)]
        assignee: Option<String>,

        /// Include meetings designated `sensitivity: restricted` (excluded
        /// by default). The override is recorded on the event bus.
        #[arg(long)]
        include_restricted: bool,
    },

    /// Flag conflicting decisions and stale commitments across meetings
    Consistency {
        /// Filter stale commitments by owner / person
        #[arg(long)]
        owner: Option<String>,

        /// Flag commitments this many days old or older
        #[arg(long, default_value = "7")]
        stale_after_days: i64,
    },

    /// Build a first-pass profile for a person across meetings
    Person {
        /// Person / attendee name to profile
        name: String,
    },

    /// Show relationship overview: top contacts, commitments, losing-touch alerts
    People {
        /// Subcommand (e.g. `merge`); omit to list the relationship graph
        #[command(subcommand)]
        action: Option<PeopleAction>,

        /// Force full index rebuild from markdown files
        #[arg(long)]
        rebuild: bool,

        /// Output raw JSON instead of formatted table
        #[arg(long)]
        json: bool,

        /// Maximum number of people to show
        #[arg(short, long, default_value = "15")]
        limit: usize,
    },

    /// Manage local names and terms used for future transcripts, search, and graph canonicalization
    Vocabulary {
        #[command(subcommand)]
        action: VocabularyAction,
    },

    /// List open and stale commitments from the conversation graph
    Commitments {
        /// Filter by person name or slug
        #[arg(short, long)]
        person: Option<String>,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Research a topic across meetings, decisions, and open follow-ups
    Research {
        /// Topic or question to investigate across meetings
        query: String,

        /// Filter by type: meeting or memo
        #[arg(short = 't', long)]
        content_type: Option<String>,

        /// Filter by date (ISO format, e.g., 2026-03-17)
        #[arg(short, long)]
        since: Option<String>,

        /// Filter by attendee / person
        #[arg(short, long)]
        attendee: Option<String>,

        /// Include meetings designated `sensitivity: restricted` (excluded
        /// by default). The override is recorded on the event bus.
        #[arg(long)]
        include_restricted: bool,
    },

    /// List recent meetings and voice memos
    List {
        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Filter by type: meeting or memo
        #[arg(short = 't', long)]
        content_type: Option<String>,

        /// Force a full re-walk + reindex before listing.
        #[arg(long, conflicts_with = "no_sync")]
        sync: bool,

        /// Skip filesystem sync entirely; query the index as-is.
        #[arg(long, conflicts_with = "sync")]
        no_sync: bool,

        /// Include meetings designated `sensitivity: restricted` (excluded
        /// by default). The override is recorded on the event bus.
        #[arg(long)]
        include_restricted: bool,
    },

    /// Export meetings as CSV (to stdout or file)
    Export {
        /// Filter by type: meeting or memo
        #[arg(short = 't', long)]
        content_type: Option<String>,

        /// Write CSV to a file instead of stdout
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Ingest meetings into the knowledge base (extract facts, update person profiles)
    Ingest {
        /// Path to a meeting .md file, or omit to process all meetings
        path: Option<PathBuf>,

        /// Process all meetings in the output directory
        #[arg(long)]
        all: bool,

        /// Show what would be extracted without writing anything
        #[arg(long)]
        dry_run: bool,
    },

    /// Clean up hallucinated repetitions in existing transcripts
    Clean {
        /// Path to meeting .md file, or "all" to clean all meetings
        meeting: String,

        /// Actually modify the files (default: dry-run showing what would change)
        #[arg(long)]
        apply: bool,
    },

    /// Re-run speaker mapping on an existing meeting (recovery for failed/missing maps)
    RedoSpeakerMapping {
        /// Path to meeting .md file, or a search term to find one
        meeting: String,

        /// Actually write the new speaker map (default: dry-run showing what would change)
        #[arg(long)]
        apply: bool,

        /// Override the summarization engine for this run (e.g. "agent", "ollama", "mistral")
        #[arg(long)]
        engine: Option<String>,

        /// Emit machine-readable JSON instead of human output
        #[arg(long)]
        json: bool,
    },

    /// Process an audio file through the pipeline
    Process {
        /// Path to audio file (.wav, .m4a, .mp3)
        path: PathBuf,

        /// Content type: meeting or memo
        #[arg(short = 't', long, default_value = "memo")]
        content_type: String,

        /// Optional context note (e.g., "idea about onboarding while driving")
        #[arg(short = 'n', long)]
        note: Option<String>,

        /// Optional title
        #[arg(long)]
        title: Option<String>,

        /// Transcription language (e.g. "en", "ur", "es"). Overrides config.toml setting.
        #[arg(short, long)]
        language: Option<String>,

        /// Template slug to apply to summarization (e.g. "standup", "1-on-1").
        /// Use `minutes template list` to see available templates.
        #[arg(long)]
        template: Option<String>,
    },

    /// Transcribe an audio file to text without writing meeting files or summarizing.
    /// Useful for integrations: audio in, transcript (or JSON) out.
    Transcribe {
        /// Audio file (.wav, .m4a, .mp3, .ogg, .webm, .mp4)
        path: PathBuf,

        /// Output a JSON envelope to stdout instead of plain text
        #[arg(long)]
        json: bool,

        /// Transcription language override (e.g. "en", "es"). Uses config value if omitted.
        #[arg(short, long)]
        language: Option<String>,

        /// Run speaker diarization and annotate each segment with a speaker label
        #[arg(long)]
        diarize: bool,
    },

    /// Manage summarization templates (list, show, validate)
    Template {
        #[command(subcommand)]
        cmd: TemplateCmd,
    },

    /// Watch a folder for new audio files and process them automatically
    Watch {
        /// Directory to watch (default: ~/.minutes/inbox/)
        dir: Option<PathBuf>,

        /// Transcription language (e.g. "en", "ur", "es"). Overrides config.toml setting.
        #[arg(short, long)]
        language: Option<String>,
    },

    /// Download whisper model and set up minutes
    Setup {
        /// Model to download: tiny, base, small, medium, large-v3
        #[arg(short, long, default_value = "small")]
        model: String,

        /// List available models
        #[arg(long)]
        list: bool,

        /// Download speaker diarization models (~34 MB)
        #[arg(long)]
        diarization: bool,

        /// Download parakeet.cpp model for alternative transcription engine
        #[arg(long)]
        parakeet: bool,

        /// Parakeet model to download: tdt-ctc-110m, tdt-600m
        #[arg(long, default_value = "tdt-600m")]
        parakeet_model: String,

        /// Download the sherpa-onnx parakeet-v3 model for the opt-in `engine-sherpa` engine (~670 MB)
        #[arg(long)]
        sherpa: bool,

        /// Install the bundled 5-meeting fixture corpus for demoing search, graph, and MCP flows
        #[arg(long)]
        demo: bool,
    },

    /// Inspect or register the meetings directory as a QMD collection
    Qmd {
        /// Action: status or register
        #[arg(value_parser = ["status", "register"])]
        action: String,

        /// Collection name to use in QMD
        #[arg(long, default_value = "minutes")]
        collection: String,
    },

    /// Run a file-backed automation primitive (designed for launchd/cron)
    Automate {
        /// Automation kind: weekly-summary or proactive-context
        #[arg(value_parser = ["weekly-summary", "proactive-context"])]
        kind: String,

        /// Write markdown output to this file instead of the default automation-runs directory
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// Optional draft-only delivery payload to generate beside the markdown artifact
        #[arg(long, value_parser = ["slack-json", "email-markdown"])]
        delivery_target: Option<String>,

        /// Output the run record JSON as well
        #[arg(long)]
        json: bool,
    },

    /// Dictate: speak and get text in your clipboard + daily note
    Dictate {
        /// Output to stdout instead of clipboard
        #[arg(long)]
        stdout: bool,

        /// Only write to daily note (no clipboard)
        #[arg(long)]
        note_only: bool,

        /// Transcription language (e.g. "en", "ur", "es"). Overrides config.toml setting.
        #[arg(short, long)]
        language: Option<String>,

        /// Audio input device name. Use `minutes devices` to list available devices.
        /// Overrides the [recording] device setting in config.toml.
        #[arg(short = 'D', long)]
        device: Option<String>,
    },

    /// List available audio input devices
    Devices,

    /// List audio input devices grouped by category (Microphones / System Audio / Virtual)
    Sources,

    /// Install, restart, uninstall, or check the folder watcher as a login service.
    /// `install` is idempotent — run it again after upgrading the binary to point
    /// launchd at the new path. `restart` reloads without rewriting the plist.
    Service {
        /// Action: install, uninstall, restart, or status
        #[arg(value_parser = ["install", "uninstall", "restart", "status"])]
        action: String,
    },

    /// Show recent logs
    Logs {
        /// Show only errors
        #[arg(long)]
        errors: bool,

        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,
    },

    /// Check system health (model, mic, calendar, disk, watcher)
    Health {
        /// Output raw JSON instead of formatted table
        #[arg(long)]
        json: bool,
    },

    /// Run a demo recording to verify the pipeline works (uses bundled audio, no mic needed)
    Demo {
        /// Seed 5 realistic sample meetings (Snow Crash theme) to try search, people, and actions
        #[arg(long)]
        full: bool,
        /// Remove demo meetings created by --full
        #[arg(long)]
        clean: bool,
        /// Run a cross-meeting query to preview the agent experience without Claude
        #[arg(long)]
        query: bool,
    },

    /// Output the JSON Schema for the meeting frontmatter format
    Schema,

    /// Get a meeting by filename slug or path
    Get {
        /// Filename slug (e.g., "2026-03-17-advisor-call") or full meeting path
        slug: String,

        /// Emit structured JSON with overlay-applied speaker_map instead of raw markdown
        #[arg(long)]
        json: bool,

        /// When used with --json, omit raw_markdown to keep payloads small
        #[arg(long)]
        compact_json: bool,
    },

    /// Show recent events from the event log
    Events {
        /// Maximum number of events
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Only events with this event_type
        #[arg(long)]
        event_type: Option<String>,

        /// Only events since this date (ISO format)
        #[arg(long)]
        since: Option<String>,

        /// Stream events as newline-delimited JSON and keep waiting for new events
        #[arg(long)]
        follow: bool,

        /// Start after this event sequence cursor
        #[arg(long)]
        since_seq: Option<u64>,
    },

    /// Get private, real-time meeting coaching
    #[command(visible_alias = "coach")]
    Copilot {
        #[command(subcommand)]
        action: CopilotAction,
    },

    /// Append an allowlisted agent.annotation event without mutating meeting markdown
    AgentAnnotate {
        /// Stable agent identifier from ~/.minutes/agents.allow
        #[arg(long)]
        agent_id: String,

        /// Tool or model names used to produce the annotation
        #[arg(long = "tool")]
        tools: Vec<String>,

        /// Annotation subtype, e.g. coaching, correction, risk, summary
        #[arg(long, default_value = "commentary")]
        subkind: String,

        /// Target meeting identifier, if known
        #[arg(long)]
        meeting_id: Option<String>,

        /// Target meeting markdown path, if known
        #[arg(long)]
        meeting_path: Option<String>,

        /// Start offset of the target span in milliseconds
        #[arg(long)]
        span_start_ms: Option<u64>,

        /// End offset of the target span in milliseconds
        #[arg(long)]
        span_end_ms: Option<u64>,

        /// Annotation body
        #[arg(long)]
        body: String,

        /// Citation or source reference; may be repeated
        #[arg(long = "citation")]
        citations: Vec<String>,

        /// Confidence label
        #[arg(long, default_value = "medium")]
        confidence: String,

        /// JSON object describing provenance
        #[arg(long)]
        provenance: Option<String>,
    },

    /// Query structured meeting insights (decisions, commitments, questions)
    Insights {
        /// Filter by insight type: decision, commitment, question
        #[arg(short, long)]
        kind: Option<String>,

        /// Minimum confidence: tentative, inferred, strong, explicit
        #[arg(short, long)]
        confidence: Option<String>,

        /// Filter by participant name (partial match)
        #[arg(short, long)]
        participant: Option<String>,

        /// Only insights since this date (YYYY-MM-DD)
        #[arg(long)]
        since: Option<String>,

        /// Maximum number of results
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Only show actionable insights (Strong or Explicit confidence)
        #[arg(short, long)]
        actionable: bool,
    },

    /// Query meeting-adjacent desktop context from the local sidecar store
    Context {
        #[command(subcommand)]
        action: ContextAction,
    },

    /// Import meetings from another app, or recover-process an audio file
    Import {
        /// Source app (granola), or an audio file path to process as a meeting
        from: String,

        /// Directory containing exported meetings (default: ~/.granola-archivist/output/)
        #[arg(short, long)]
        dir: Option<PathBuf>,

        /// Dry run: show what would be imported without copying
        #[arg(long)]
        dry_run: bool,
    },

    /// Connect your Obsidian/Logseq vault to Minutes
    Vault {
        #[command(subcommand)]
        action: VaultAction,
    },

    /// Enroll your voice for automatic speaker identification
    Enroll {
        /// Enroll from an existing audio file instead of recording
        #[arg(long)]
        file: Option<PathBuf>,
        /// Recording duration in seconds (default: 10)
        #[arg(long, default_value = "10")]
        duration: u64,
    },

    /// List and manage enrolled voice profiles
    Voices {
        /// Delete your voice profile
        #[arg(long)]
        delete: bool,
        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Start a live transcript session (real-time meeting transcription)
    Live {
        /// Transcription language (e.g. "en", "ur", "es"). Overrides config.toml setting.
        #[arg(short, long)]
        language: Option<String>,

        /// Audio input device name. Use `minutes devices` to list available devices.
        /// Overrides the [recording] device setting in config.toml.
        #[arg(short = 'D', long)]
        device: Option<String>,
    },

    /// Read the live transcript (delta reads from an active or recent session)
    Transcript {
        /// Lines since line number N, or duration like "5m", "30s"
        #[arg(long)]
        since: Option<String>,

        /// Show session status only
        #[arg(long)]
        status: bool,

        /// Output format: text or json
        #[arg(long, default_value = "json", value_parser = ["text", "json"])]
        format: String,
    },

    /// Open the Meeting Intelligence Dashboard in your browser
    Dashboard {
        /// Port to serve on (default: 3141)
        #[arg(short, long, default_value = "3141")]
        port: u16,

        /// Don't open the browser automatically
        #[arg(long)]
        no_open: bool,
    },

    /// Delete a meeting or voice memo (moves to archive by default, or permanently with --force)
    Delete {
        /// Filename slug or path of the meeting to delete (e.g., "2026-03-17-standup")
        meeting: String,

        /// Also delete the original .wav audio file
        #[arg(long)]
        with_audio: bool,

        /// Permanently delete instead of archiving
        #[arg(long)]
        force: bool,
    },

    /// Confirm or correct speaker attributions for a meeting
    Confirm {
        /// Path to the meeting markdown file
        #[arg(long)]
        meeting: PathBuf,

        /// Non-interactive: specify speaker label to confirm (e.g., SPEAKER_1)
        #[arg(long)]
        speaker: Option<String>,

        /// Non-interactive: name to assign to the speaker
        #[arg(long)]
        name: Option<String>,

        /// Save confirmed speaker's voice profile for future meetings
        #[arg(long)]
        save_voice: bool,
    },
}

#[derive(Subcommand)]
enum SensitiveAction {
    /// Start a no-capture sensitive meeting
    Start {
        /// Title for the sensitive meeting artifact
        #[arg(short, long)]
        title: Option<String>,
    },
    /// Stop the active sensitive meeting and write its artifact
    Stop,
}

#[derive(Subcommand)]
enum PeopleAction {
    /// Confirm that several people are the same person and collapse them.
    ///
    /// Records a durable person alias (in the local vocabulary) so future
    /// transcripts, search, and graph rebuilds resolve every variant spelling to
    /// the canonical one. The first argument is the surviving canonical name; the
    /// rest are its variants. Accepts slugs (from the "Possible name variants"
    /// suggestions) or exact names.
    Merge {
        /// Canonical (surviving) person: a slug or exact name.
        canonical: String,
        /// Variant people to fold into the canonical: slugs or exact names.
        #[arg(required = true)]
        aliases: Vec<String>,
        /// Skip the automatic graph rebuild (run `minutes people --rebuild` later).
        #[arg(long)]
        no_rebuild: bool,
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum VocabularyAction {
    /// List local vocabulary entries
    List {
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
    /// Add a local vocabulary entry or alias
    Add {
        /// Entry kind
        #[arg(long, default_value = "term", value_parser = ["person", "organization", "project", "term", "acronym"])]
        kind: String,

        /// Canonical spelling to prefer
        canonical: String,

        /// Alias or common misrecognition. Repeat for multiple aliases.
        #[arg(long = "alias")]
        aliases: Vec<String>,

        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
    /// Remove a vocabulary entry by id
    Remove {
        /// Entry id from `minutes vocabulary list`
        id: String,

        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
    /// Suggest vocabulary entries from a meeting file
    Suggest {
        /// Meeting markdown file to inspect
        meeting: PathBuf,

        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
    /// Rebuild derived indexes that use vocabulary
    Rebuild {
        /// Output raw JSON instead of formatted text
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum VaultAction {
    /// Detect vaults and set up sync
    Setup {
        /// Vault root path (skip auto-detection)
        #[arg(short, long)]
        path: Option<PathBuf>,

        /// Force a specific strategy: symlink, copy, or direct
        #[arg(short, long, value_parser = ["symlink", "copy", "direct"])]
        strategy: Option<String>,

        /// Subdirectory inside the vault for meetings (default: "areas/meetings")
        #[arg(long)]
        subdir: Option<String>,
    },
    /// Check vault sync health
    Status,
    /// Remove vault configuration
    Unlink,
    /// Copy all existing meetings to vault (catch-up for copy strategy)
    Sync,
}

#[derive(Subcommand)]
enum AutoresearchAction {
    /// Compare decode-hint baseline vs candidate runs against a local corpus.
    #[command(name = "decode-hints")]
    Run {
        /// Path to the local corpus manifest JSON
        #[arg(long)]
        corpus: PathBuf,

        /// Output root for local research artifacts (defaults to ~/.minutes/research/decode-hints)
        #[arg(long)]
        out: Option<PathBuf>,

        /// Force a specific engine for every case (for example: whisper, parakeet)
        #[arg(long)]
        engine: Option<String>,

        /// Print the full JSON report envelope to stdout
        #[arg(long)]
        json: bool,
    },

    /// Compare two prior decode-hint eval reports or run directories.
    #[command(name = "compare-decode-hints")]
    Compare {
        /// Left/base report path or run directory
        #[arg(long)]
        left: PathBuf,

        /// Right/candidate report path or run directory
        #[arg(long)]
        right: PathBuf,

        /// Output root for local comparison artifacts (defaults to ~/.minutes/research/decode-hints-comparisons)
        #[arg(long)]
        out: Option<PathBuf>,

        /// Print the full JSON comparison envelope to stdout
        #[arg(long)]
        json: bool,
    },

    /// List recent decode-hint eval and comparison runs.
    #[command(name = "list-decode-hints")]
    List {
        /// Maximum number of recent runs to show
        #[arg(long, default_value_t = 10)]
        limit: usize,

        /// Print the full JSON listing to stdout
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AppleSpeechAction {
    /// Probe Apple speech capability and asset readiness on the current Mac.
    Capabilities {
        /// Print the full JSON capability payload to stdout
        #[arg(long)]
        json: bool,
    },

    /// Run the Apple-vs-current benchmark corpus and write artifacts.
    Benchmark {
        /// Path to the benchmark corpus manifest JSON
        #[arg(long)]
        corpus: PathBuf,

        /// Output root for local research artifacts (defaults to ~/.minutes/research/apple-speech)
        #[arg(long)]
        out: Option<PathBuf>,

        /// Print the full JSON report envelope to stdout
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum CopilotAction {
    /// Turn Coach on for the current meeting
    Start {
        /// What you want help with in this meeting
        #[arg(long)]
        goal: String,

        /// How to show suggestions (defaults to the choice in config.toml)
        #[arg(long, value_parser = ["tui", "stdout"])]
        surface: Option<String>,

        /// Per-session coaching policy (defaults to [copilot].mode)
        #[arg(long, value_parser = ["sales", "discovery", "interview", "negotiation", "difficult-conversation", "decision", "generic"])]
        mode: Option<String>,

        /// Own a standalone live capture in this process, enabling ephemeral
        /// partial coaching when the backend and config support it
        #[arg(long)]
        live: bool,
    },
    /// Show what Coach is doing in plain language
    Status,
    /// Pause suggestions without affecting the recording
    Pause,
    /// Resume suggestions
    Resume,
    /// Stop Coach without affecting the recording
    Stop,
    /// Rate a rendered nudge; adaptation is bounded to the active session
    Feedback {
        /// Nudge ID shown by the TUI or stdout JSON
        #[arg(long)]
        nudge_id: String,

        /// Feedback signal
        #[arg(long, value_parser = ["helpful", "not-helpful", "dismissed"])]
        rating: String,
    },
    /// Replay the versioned coaching corpus and enforce quality baselines
    Eval {
        /// Load versioned JSON fixtures from this directory instead of the built-in corpus
        #[arg(long)]
        fixtures: Option<PathBuf>,

        /// Evaluate the real coaching prompt against this Ollama model
        #[arg(long)]
        model: Option<String>,

        /// Replay fixture order without sleeping to transcript offsets
        #[arg(long)]
        accelerated: bool,

        /// Print the complete machine-readable report instead of the table
        #[arg(long)]
        json: bool,
    },
    /// Set up the private AI Coach needs
    Setup {
        /// Force a specific Ollama model instead of the hardware recommendation
        #[arg(long, conflicts_with = "retune")]
        model: Option<String>,

        /// Re-run hardware selection even when a custom model is configured
        #[arg(long)]
        retune: bool,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// List installed templates (project + user + bundled)
    List,
    /// Print the contents of a template by slug
    Show {
        /// Template slug (e.g. "standup", "1-on-1")
        slug: String,
    },
    /// Validate a template file (schema check, no execution)
    Validate {
        /// Path to a `.md` template file
        path: PathBuf,
    },
}

#[derive(Subcommand)]
enum ContextAction {
    /// Report observed desktop and screen-context state for the current session
    Status {
        /// Explicit context session id
        #[arg(long)]
        session: Option<String>,

        /// Artifact path already linked to a context session
        #[arg(long)]
        path: Option<PathBuf>,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Retrieve verified screenshots linked to a context session
    Screen {
        /// Explicit context session id
        #[arg(long)]
        session: Option<String>,

        /// Artifact path already linked to a context session
        #[arg(long)]
        path: Option<PathBuf>,

        /// Select the nearest screenshot around this RFC3339 timestamp
        #[arg(long)]
        at: Option<String>,

        /// Maximum screenshots to return (1-3)
        #[arg(short, long, default_value = "1")]
        limit: usize,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Summarize desktop context for a session, artifact, or explicit time window
    ActivitySummary {
        /// Explicit context session id
        #[arg(long)]
        session: Option<String>,

        /// Artifact path already linked to a context session
        #[arg(long)]
        path: Option<PathBuf>,

        /// Window start (RFC3339 timestamp)
        #[arg(long)]
        start: Option<String>,

        /// Window end (RFC3339 timestamp)
        #[arg(long)]
        end: Option<String>,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Search app/window/browser-title context events
    Search {
        /// Text query
        query: String,

        /// Maximum number of results
        #[arg(short, long, default_value = "20")]
        limit: usize,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },

    /// Show the local rewind around a session, linked artifact, or timestamp
    GetMoment {
        /// Explicit context session id
        #[arg(long)]
        session: Option<String>,

        /// Linked artifact path
        #[arg(long)]
        path: Option<PathBuf>,

        /// Explicit timestamp (RFC3339)
        #[arg(long)]
        at: Option<String>,

        /// Minutes before the anchor
        #[arg(long, default_value = "10")]
        before_minutes: i64,

        /// Minutes after the anchor
        #[arg(long, default_value = "10")]
        after_minutes: i64,

        /// Output raw JSON
        #[arg(long)]
        json: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging.
    //
    // Default filter: app code at INFO (or DEBUG with --verbose), but the
    // whisper.cpp + ggml C-level loggers at WARN. The C loggers are chatty
    // by default — `whisper_vad_detect_speech: detect speech (X.XXs duration)`
    // fires roughly once per 100ms during a recording (issue #163). Demoting
    // them to warn keeps real errors visible without flooding the terminal.
    // RUST_LOG, when set, overrides this default entirely.
    let app_level = if cli.verbose { "debug" } else { "info" };
    let default_filter = format!("{app_level},whisper_rs=warn,ggml=warn");
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();
    // Route whisper.cpp + ggml stderr through the tracing subscriber we just
    // installed. Safe to call multiple times; only the first call has effect.
    // Without this, the C-level logs leak to raw stderr and bypass the filter.
    minutes_core::install_whisper_logging_hooks();

    let mut config = Config::load();
    install_parakeet_panic_hook();

    // Rotate old log files at startup
    minutes_core::logging::rotate_logs().ok();

    let result = match cli.command {
        Commands::Record {
            title,
            context,
            mode,
            intent,
            allow_degraded,
            skip_audio_probe,
            consent,
            consent_notice,
            language,
            device,
            source,
            call,
            diagnose,
            mute_mic,
            template,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            // Validate the template now so the user gets immediate feedback
            // if they typo a slug rather than discovering it after a recording.
            if let Some(slug) = template.as_deref() {
                minutes_core::TemplateResolver::new()
                    .resolve(slug)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
            }

            resolve_recording_device_overrides(&mut config, &source, device, call)?;

            // Pre-arm the mute sentinel so the recording starts with the mic
            // muted. The record loop picks this up on its first iteration.
            if mute_mic {
                minutes_core::streaming::set_mic_muted_with_sentinel(true);
                eprintln!("[minutes] Starting with microphone muted (system audio only).");
            }

            if call && source.len() < 2 {
                // --call with auto-detect: resolve loopback device
                if let Some(loopback) = minutes_core::capture::detect_loopback_device() {
                    eprintln!(
                        "[minutes] Detected system audio device: {}\n\
                         Starting CLI dual-source call capture.\n\
                         {}\n\
                         If you intended a mic-only fallback instead, omit `--call` and choose one explicit input device.",
                        loopback,
                        desktop_call_capture_workaround()
                    );
                } else {
                    eprintln!(
                        "[minutes] No system audio device detected.\n\
                         To capture call audio, install a loopback driver:\n\
                           macOS: brew install blackhole-2ch\n\
                         {}\n\
                         Without a loopback route, the CLI can only record a single input device.",
                        desktop_call_capture_workaround()
                    );
                }
            }

            if let Some(wav_path) = diagnose {
                cmd_diagnose(&wav_path, title.as_deref(), &config)
            } else {
                let effective_intent = if call && intent == "auto" {
                    "call"
                } else {
                    &intent
                };
                cmd_record(
                    title,
                    context,
                    &mode,
                    effective_intent,
                    allow_degraded,
                    skip_audio_probe.as_deref(),
                    consent.as_deref(),
                    consent_notice.as_deref(),
                    template,
                    &config,
                )
            }
        }
        Commands::Note { text, meeting } => cmd_note(&text, meeting.as_deref(), &config),
        Commands::Stop => cmd_stop(&config),
        Commands::Sensitive { action } => cmd_sensitive(action, &config),
        Commands::Extend => {
            if !minutes_core::pid::status().recording {
                eprintln!("No active recording to extend.");
                std::process::exit(1);
            }
            minutes_core::capture::write_extend_sentinel()?;
            eprintln!("Recording extended — auto-stop timers reset.");
            Ok(())
        }
        Commands::MicToggle { state } => cmd_mic_toggle(state.as_deref()),
        Commands::ProcessQueue => cmd_process_queue(&config),
        Commands::ParakeetHelper {
            binary,
            model_path,
            audio_path,
            vocab_path,
            model_id,
            gpu,
            fp16,
            vad_path,
            vad_threshold,
        } => cmd_parakeet_helper(
            &binary,
            &model_path,
            &audio_path,
            &vocab_path,
            &model_id,
            gpu,
            fp16,
            vad_path.as_deref(),
            vad_threshold,
            &config,
        ),
        Commands::ParakeetBenchmark {
            binary,
            model_path,
            audio_path,
            vocab_path,
            model_id,
            gpu,
            vad_path,
            vad_threshold,
        } => cmd_parakeet_benchmark(
            &binary,
            &model_path,
            &audio_path,
            &vocab_path,
            &model_id,
            gpu,
            vad_path.as_deref(),
            vad_threshold,
            &config,
        ),
        Commands::PreflightRecord {
            mode,
            intent,
            allow_degraded,
            json,
        } => cmd_preflight_record(&mode, &intent, allow_degraded, json, &config),
        Commands::Status => cmd_status(),
        Commands::Jobs { all, json, limit } => cmd_jobs(all, json, limit),
        Commands::Paths { json } => cmd_paths(json, &config),
        Commands::Storage { json } => cmd_storage(json, &config),
        Commands::Cleanup {
            apply,
            older_than,
            json,
        } => cmd_cleanup(apply, older_than.as_deref(), json, &config),
        Commands::Autoresearch { action } => match action {
            AutoresearchAction::Run {
                corpus,
                out,
                engine,
                json,
            } => cmd_autoresearch_decode_hints(&corpus, out.as_deref(), engine.as_deref(), json),
            AutoresearchAction::Compare {
                left,
                right,
                out,
                json,
            } => cmd_autoresearch_compare_decode_hints(&left, &right, out.as_deref(), json),
            AutoresearchAction::List { limit, json } => {
                cmd_autoresearch_list_decode_hints(limit, json)
            }
        },
        Commands::AppleSpeech { action } => match action {
            AppleSpeechAction::Capabilities { json } => cmd_apple_speech_capabilities(json),
            AppleSpeechAction::Benchmark { corpus, out, json } => {
                cmd_apple_speech_benchmark(&corpus, out.as_deref(), json, &config)
            }
        },
        Commands::Capabilities { json } => cmd_capabilities(json),
        Commands::Search {
            query,
            content_type,
            since,
            limit,
            intents_only,
            intent_kind,
            owner,
            format,
            sync,
            no_sync,
            include_restricted,
        } => cmd_search(
            &query,
            content_type,
            since,
            limit,
            intents_only,
            intent_kind,
            owner,
            &format,
            resolve_sync_mode(sync, no_sync),
            include_restricted,
            &config,
        ),
        Commands::Actions {
            assignee,
            include_restricted,
        } => cmd_actions(assignee.as_deref(), include_restricted, &config),
        Commands::Consistency {
            owner,
            stale_after_days,
        } => cmd_consistency(owner.as_deref(), stale_after_days, &config),
        Commands::Person { name } => cmd_person(&name, &config),
        Commands::People {
            action,
            rebuild,
            json,
            limit,
        } => match action {
            Some(PeopleAction::Merge {
                canonical,
                aliases,
                no_rebuild,
                json: merge_json,
            }) => cmd_people_merge(
                &canonical,
                &aliases,
                no_rebuild,
                // Accept `--json` whether typed on `people` or on `merge`, so the
                // otherwise-inert parent flag isn't silently ignored.
                json || merge_json,
                &config,
            ),
            None => cmd_people(rebuild, json, limit, &config),
        },
        Commands::Vocabulary { action } => cmd_vocabulary(action, &config),
        Commands::Commitments { person, json } => cmd_commitments(person.as_deref(), json, &config),
        Commands::Research {
            query,
            content_type,
            since,
            attendee,
            include_restricted,
        } => cmd_research(
            &query,
            content_type,
            since,
            attendee,
            include_restricted,
            &config,
        ),
        Commands::List {
            limit,
            content_type,
            sync,
            no_sync,
            include_restricted,
        } => cmd_list(
            limit,
            content_type,
            resolve_sync_mode(sync, no_sync),
            include_restricted,
            &config,
        ),
        Commands::Export {
            content_type,
            output,
        } => cmd_export(content_type, output, &config),
        Commands::Ingest { path, all, dry_run } => cmd_ingest(path, all, dry_run, &config),
        Commands::Clean { meeting, apply } => cmd_clean(&meeting, apply, &config),
        Commands::RedoSpeakerMapping {
            meeting,
            apply,
            engine,
            json,
        } => cmd_redo_speaker_mapping(&meeting, apply, engine, json, &config),
        Commands::Process {
            path,
            content_type,
            note,
            title,
            language,
            template,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            let trace_content_type = match content_type.as_str() {
                "meeting" => ContentType::Meeting,
                "memo" => ContentType::Memo,
                other => anyhow::bail!("unknown content type: {}. Use 'meeting' or 'memo'.", other),
            };
            let _trace_guard = minutes_core::process_trace::start_process_trace(
                &path,
                trace_content_type,
                &config,
            );
            let mut entered_pipeline = false;
            let result = (|| -> Result<()> {
                // Save note as context for the pipeline
                if let Some(ref n) = note {
                    minutes_core::notes::save_context(n)?;
                }
                let resolved_template = match template.as_deref() {
                    Some(slug) => Some(
                        minutes_core::TemplateResolver::new()
                            .resolve(slug)
                            .map_err(|e| anyhow::anyhow!("{}", e))?,
                    ),
                    None => None,
                };
                entered_pipeline = true;
                let result = cmd_process(
                    &path,
                    &content_type,
                    title.as_deref(),
                    resolved_template.as_ref(),
                    &config,
                );
                if note.is_some() {
                    minutes_core::notes::cleanup();
                }
                result
            })();
            if !entered_pipeline {
                if let Err(error) = &result {
                    minutes_core::process_trace::stage_with_extra(
                        "process.error",
                        serde_json::json!({"error": error.to_string()}),
                    );
                }
            }
            result
        }
        Commands::Transcribe {
            path,
            json,
            language,
            diarize: do_diarize,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            cmd_transcribe(&path, json, do_diarize, &config)
        }
        Commands::Template { cmd } => cmd_template(cmd),
        Commands::Watch { dir, language } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            cmd_watch(dir.as_deref(), &config)
        }
        Commands::Dictate {
            stdout,
            note_only,
            language,
            device,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            if let Some(dev) = device {
                config.recording.device = Some(dev);
            }
            cmd_dictate(stdout, note_only, &config)
        }
        Commands::Devices => cmd_devices(),
        Commands::Sources => cmd_sources(),
        Commands::Setup {
            model,
            list,
            diarization,
            parakeet,
            parakeet_model,
            sherpa,
            demo,
        } => {
            if demo {
                cmd_setup_demo()
            } else if parakeet {
                cmd_setup_parakeet(&parakeet_model)
            } else if sherpa {
                cmd_setup_sherpa(&config)
            } else {
                cmd_setup(&model, list, diarization)
            }
        }
        Commands::Qmd { action, collection } => cmd_qmd(&action, &collection, &config),
        Commands::Automate {
            kind,
            output,
            delivery_target,
            json,
        } => cmd_automate(&kind, output, delivery_target.as_deref(), json, &config),
        Commands::Service { action } => {
            #[cfg(target_os = "macos")]
            {
                cmd_service(&action)
            }
            #[cfg(target_os = "linux")]
            {
                cmd_service_linux(&action)
            }
            #[cfg(not(any(target_os = "macos", target_os = "linux")))]
            {
                let _ = action;
                eprintln!("On Windows, use Task Scheduler to schedule:");
                eprintln!("  minutes watch                              (always running)");
                eprintln!("  minutes automate weekly-summary --json     (weekly)");
                eprintln!("  minutes automate proactive-context --json  (daily)");
                Ok(())
            }
        }
        Commands::Logs { errors, lines } => cmd_logs(errors, lines),
        Commands::Health { json } => cmd_health(json),
        Commands::Demo { full, clean, query } => {
            if clean {
                let removed = demo_data::clean_demo_meetings(&config.output_dir)?;
                if removed > 0 {
                    eprintln!("\nRemoved {} demo meeting(s).", removed);
                    if full {
                        eprintln!();
                        cmd_demo_full(&config)?;
                    }
                } else {
                    eprintln!("No demo meetings found to remove.");
                    if full {
                        cmd_demo_full(&config)?;
                    }
                }
                Ok(())
            } else if query {
                demo_data::query_demo(&config.output_dir)
            } else if full {
                cmd_demo_full(&config)
            } else {
                cmd_demo(&config)
            }
        }
        Commands::Schema => cmd_schema(),
        Commands::Get {
            slug,
            json,
            compact_json,
        } => cmd_get(&slug, json, compact_json, &config),
        Commands::Events {
            limit,
            event_type,
            since,
            follow,
            since_seq,
        } => cmd_events(limit, event_type, since, follow, since_seq, &config),
        Commands::Copilot { action } => cmd_copilot(action, &mut config),
        Commands::AgentAnnotate {
            agent_id,
            tools,
            subkind,
            meeting_id,
            meeting_path,
            span_start_ms,
            span_end_ms,
            body,
            citations,
            confidence,
            provenance,
        } => cmd_agent_annotate(
            agent_id,
            tools,
            subkind,
            meeting_id,
            meeting_path,
            span_start_ms,
            span_end_ms,
            body,
            citations,
            confidence,
            provenance,
        ),
        Commands::Insights {
            kind,
            confidence,
            participant,
            since,
            limit,
            actionable,
        } => cmd_insights(kind, confidence, participant, since, limit, actionable),
        Commands::Context { action } => cmd_context(action),
        Commands::Import { from, dir, dry_run } => {
            cmd_import(&from, dir.as_deref(), dry_run, &config)
        }
        Commands::Vault { action } => match action {
            VaultAction::Setup {
                path,
                strategy,
                subdir,
            } => cmd_vault_setup(path, strategy, subdir, config),
            VaultAction::Status => cmd_vault_status(&config),
            VaultAction::Unlink => cmd_vault_unlink(config),
            VaultAction::Sync => cmd_vault_sync(&config),
        },
        Commands::Enroll { file, duration } => cmd_enroll(file.as_deref(), duration, &config),
        Commands::Voices { delete, json } => cmd_voices(delete, json),
        Commands::Delete {
            meeting,
            with_audio,
            force,
        } => cmd_delete(&meeting, with_audio, force, &config),
        Commands::Confirm {
            meeting,
            speaker,
            name,
            save_voice,
        } => cmd_confirm(
            &meeting,
            speaker.as_deref(),
            name.as_deref(),
            save_voice,
            &config,
        ),
        Commands::Live { language, device } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            if let Some(dev) = device {
                config.recording.device = Some(dev);
            }
            cmd_live(&config)
        }
        Commands::Transcript {
            since,
            status,
            format,
        } => cmd_transcript(since.as_deref(), status, &format),
        Commands::Dashboard { port, no_open } => dashboard::serve(&config, port, !no_open),
    };

    minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
    result
}

fn cmd_note(text: &str, meeting: Option<&Path>, config: &Config) -> Result<()> {
    if let Some(meeting_path) = meeting {
        // Post-meeting annotation
        minutes_core::notes::validate_meeting_path(meeting_path, &config.output_dir)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        minutes_core::notes::annotate_meeting(meeting_path, text)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        eprintln!("Note added to {}", meeting_path.display());
    } else {
        // Note during active recording
        match minutes_core::notes::add_note(text) {
            Ok(line) => eprintln!("{}", line),
            Err(e) => anyhow::bail!("{}", e),
        }
    }
    Ok(())
}

fn cmd_sensitive(action: SensitiveAction, config: &Config) -> Result<()> {
    match action {
        SensitiveAction::Start { title } => {
            let session = minutes_core::sensitive::start(title.as_deref())
                .map_err(|error| anyhow::anyhow!("{}", error))?;
            eprintln!(
                "Sensitive meeting started: {}. Add markers with `minutes note \"...\"`.",
                session.title
            );
            Ok(())
        }
        SensitiveAction::Stop => {
            let debrief = if std::io::stdin().is_terminal() {
                Some(prompt_sensitive_debrief()?)
            } else {
                None
            };
            let result = minutes_core::sensitive::stop(debrief, config)
                .map_err(|error| anyhow::anyhow!("{}", error))?;
            eprintln!("Sensitive meeting saved: {}", result.path.display());
            if result.path.exists() && !std::io::stdin().is_terminal() {
                eprintln!(
                    "[minutes] Debrief pending. Run `/minutes-debrief` from your assistant when ready."
                );
            }
            Ok(())
        }
    }
}

fn prompt_sensitive_debrief() -> Result<minutes_core::sensitive::SensitiveDebrief> {
    eprintln!("Sensitive meeting debrief. Leave a prompt blank to skip it.");
    let summary = prompt_optional_line("Summary: ")?;
    let decisions = prompt_repeated_lines("Decision");
    let action_items = prompt_repeated_lines("Action item");
    Ok(minutes_core::sensitive::SensitiveDebrief {
        summary,
        decisions,
        action_items,
    })
}

fn prompt_optional_line(prompt: &str) -> Result<Option<String>> {
    eprint!("{}", prompt);
    std::io::stderr().flush()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

fn prompt_repeated_lines(label: &str) -> Vec<String> {
    let mut values = Vec::new();
    loop {
        eprint!("{} (blank when done): ", label);
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            break;
        }
        let trimmed = input.trim();
        if trimmed.is_empty() {
            break;
        }
        values.push(trimmed.to_string());
    }
    values
}

fn capture_mode_from_str(mode: &str) -> Result<CaptureMode> {
    match mode {
        "meeting" => Ok(CaptureMode::Meeting),
        "quick-thought" => Ok(CaptureMode::QuickThought),
        other => anyhow::bail!(
            "unknown recording mode: {}. Use 'meeting' or 'quick-thought'.",
            other
        ),
    }
}

fn parse_recording_intent(intent: &str) -> Result<Option<RecordingIntent>> {
    match intent {
        "auto" => Ok(None),
        "memo" => Ok(Some(RecordingIntent::Memo)),
        "room" => Ok(Some(RecordingIntent::Room)),
        "call" => Ok(Some(RecordingIntent::Call)),
        other => anyhow::bail!(
            "unknown recording intent: {}. Use auto, memo, room, or call.",
            other
        ),
    }
}

fn cleanup_live_capture_state() {
    minutes_core::pid::remove().ok();
    minutes_core::pid::clear_recording_metadata().ok();
    minutes_core::notes::cleanup();
}

fn desktop_call_capture_workaround() -> &'static str {
    "For native dual-source call capture, use the Minutes desktop app. The published Homebrew cask/update feed is Apple Silicon-only right now, but Intel Macs on macOS 15+ can still build the desktop app from source (see README \"Desktop app\")."
}

fn normalize_source_override(source: Option<&str>) -> Option<String> {
    match source.map(str::trim) {
        Some("") | None => None,
        Some(value) if value.eq_ignore_ascii_case("default") => None,
        Some(value) => minutes_core::capture::canonicalize_input_device_setting(value),
    }
}

fn resolve_recording_device_overrides(
    config: &mut Config,
    source: &[String],
    device: Option<String>,
    call: bool,
) -> Result<()> {
    if source.len() >= 2 {
        let voice = normalize_source_override(source.first().map(String::as_str));
        let call_source = normalize_source_override(source.get(1).map(String::as_str))
            .ok_or_else(|| anyhow::anyhow!("dual-source capture requires a call/system source"))?;
        config.recording.sources = Some(minutes_core::config::SourcesConfig {
            voice,
            call: Some(call_source),
        });
        config.recording.device = None;
        return Ok(());
    }

    if call {
        if let Some(loopback) = minutes_core::capture::detect_loopback_device() {
            let voice = source
                .first()
                .map(String::as_str)
                .and_then(|value| normalize_source_override(Some(value)))
                .or(device.clone());
            config.recording.sources = Some(minutes_core::config::SourcesConfig {
                voice,
                call: Some(loopback),
            });
            config.recording.device = None;
            return Ok(());
        }
    }

    if !source.is_empty() {
        config.recording.sources = None;
        config.recording.device = normalize_source_override(source.first().map(String::as_str));
        return Ok(());
    }

    if let Some(dev) = device {
        config.recording.sources = None;
        config.recording.device = minutes_core::capture::canonicalize_input_device_setting(&dev);
        return Ok(());
    }

    if let Some(sources) = config.recording.sources.clone() {
        match (sources.voice.as_deref(), sources.call.as_deref()) {
            (Some(voice), Some(call)) => {
                config.recording.device = None;
                config.recording.sources = Some(minutes_core::config::SourcesConfig {
                    voice: normalize_source_override(Some(voice)),
                    call: Some(call.to_string()),
                });
            }
            (Some(voice), None) => {
                config.recording.sources = None;
                config.recording.device = normalize_source_override(Some(voice));
            }
            (None, Some(call)) => {
                config.recording.device = None;
                config.recording.sources = Some(minutes_core::config::SourcesConfig {
                    voice: None,
                    call: Some(call.to_string()),
                });
            }
            (None, None) => {
                config.recording.sources = None;
            }
        }
    }

    Ok(())
}

fn cmd_preflight_record(
    mode: &str,
    intent: &str,
    allow_degraded: bool,
    json: bool,
    config: &Config,
) -> Result<()> {
    let capture_mode = capture_mode_from_str(mode)?;
    let requested_intent = parse_recording_intent(intent)?;
    let preflight = minutes_core::capture::preflight_recording(
        capture_mode,
        requested_intent,
        allow_degraded,
        config,
    )
    .map_err(|error| anyhow::anyhow!("{}", error))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&preflight)?);
    } else if let Some(reason) = &preflight.blocking_reason {
        anyhow::bail!("{}", reason);
    } else {
        println!(
            "{} intent ready on '{}'.",
            preflight.intent.as_str(),
            preflight.input_device
        );
        for warning in &preflight.warnings {
            eprintln!("warning: {}", warning);
        }
    }
    Ok(())
}

fn check_meeting_system_audio_probe(
    capture_mode: CaptureMode,
    skip_audio_probe: Option<&str>,
    config: &Config,
) -> Result<Option<minutes_core::markdown::RecordingHealth>> {
    if capture_mode != CaptureMode::Meeting {
        return Ok(None);
    }

    if minutes_core::capture::resolve_system_audio_probe_device(config)
        .map_err(|error| anyhow::anyhow!("{}", error))?
        .is_none()
    {
        if skip_audio_probe.is_some() {
            anyhow::bail!("--skip-audio-probe was provided, but no system-audio source is configured for this recording");
        }
        return Ok(None);
    }

    if let Some(reason) = skip_audio_probe {
        let reason = reason.trim();
        if reason.is_empty() {
            anyhow::bail!("--skip-audio-probe requires a non-empty reason");
        }
        eprintln!(
            "[minutes] System-audio readiness probe skipped for this recording: {}",
            reason
        );
        return Ok(Some(
            minutes_core::health::recording_health_for_skipped_system_audio_probe(reason),
        ));
    }

    match minutes_core::health::probe_system_audio_capture(config)
        .map_err(|error| anyhow::anyhow!("{}", error))?
    {
        None => Ok(None),
        Some((route, result)) if result.failure_kind.is_none() => {
            if let Some(device) = route.device_name.as_deref() {
                eprintln!(
                    "[minutes] System-audio readiness probe passed on '{}'.",
                    device
                );
            }
            Ok(None)
        }
        Some((route, result)) => {
            let health = minutes_core::health::recording_health_for_system_audio_probe_failure(
                Some(&route),
                &result,
            );
            let detail = health
                .capture_warnings
                .first()
                .map(|warning| warning.message.as_str())
                .unwrap_or("System-audio readiness probe failed.");
            anyhow::bail!(
                "{} Use --skip-audio-probe \"<reason>\" for this run only if you intentionally want to record despite this degraded system-audio signal.",
                detail
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_record(
    title: Option<String>,
    context: Option<String>,
    mode: &str,
    intent: &str,
    allow_degraded: bool,
    skip_audio_probe: Option<&str>,
    consent: Option<&str>,
    consent_notice: Option<&str>,
    template_slug: Option<String>,
    config: &Config,
) -> Result<()> {
    // Ensure directories exist
    config.ensure_dirs()?;
    let capture_mode = capture_mode_from_str(mode)?;
    let requested_intent = parse_recording_intent(intent)?;

    let preflight = minutes_core::capture::preflight_recording(
        capture_mode,
        requested_intent,
        allow_degraded,
        config,
    )
    .map_err(|error| anyhow::anyhow!("{}", error))?;
    if let Some(reason) = &preflight.blocking_reason {
        anyhow::bail!("{}", reason);
    }
    for warning in &preflight.warnings {
        eprintln!("[minutes] {}", warning);
    }

    let startup_recording_health =
        check_meeting_system_audio_probe(capture_mode, skip_audio_probe, config)?;

    // Check for conflicting live transcript session. `inspect_pid_file` so a
    // standalone session holding the PID under a mandatory Windows lock is seen —
    // otherwise a recording could start alongside it and clobber the shared
    // `live-transcript.jsonl`. See #258.
    let lt_pid = minutes_core::pid::live_transcript_pid_path();
    if minutes_core::pid::inspect_pid_file(&lt_pid).is_active() {
        anyhow::bail!("live transcript in progress — run `minutes stop` first");
    }
    minutes_core::sensitive::ensure_inactive_for_recording()
        .map_err(|error| anyhow::anyhow!("{}", error))?;

    let recording_consent = if capture_mode == CaptureMode::Meeting {
        eprintln!("Recording + transcribing locally — audio stays on your device.");
        let resolved = prepare_recording_consent(
            config,
            consent,
            consent_notice,
            std::io::stdin().is_terminal(),
            prompt_for_recording_consent,
        )?;
        if let Some(warning) = resolved.warning.as_deref() {
            eprintln!("[minutes] {}", warning);
        }
        if let Some(script) = resolved.reminder.as_deref() {
            eprintln!("[minutes] Reminder: ensure everyone present consents where required.");
            eprintln!("[minutes] Disclosure script: {}", script);
        }
        Some(resolved)
    } else {
        None
    };

    // Check if already recording
    let recording_started_at = Local::now();
    minutes_core::pid::create().map_err(|e| anyhow::anyhow!("{}", e))?;
    // Re-check sensitive-session exclusivity now that the recording PID (the
    // atomic flock anchor) is held: a sensitive session may have started in
    // the window between the early check above and pid::create. With the PID
    // held, a concurrent sensitive start is blocked by its own recording
    // check, so this re-check closes the interleaving in both directions
    // (review F3). On conflict, release the PID and bail.
    if let Err(error) = minutes_core::sensitive::ensure_inactive_for_recording() {
        let _ = minutes_core::pid::remove();
        anyhow::bail!("{}", error);
    }
    let context_session_id = minutes_core::desktop_context::maybe_start_capture_session(
        &config.desktop_context,
        config.screen_context.enabled,
        capture_mode,
        title.clone(),
        recording_started_at,
    );
    minutes_core::pid::write_recording_metadata_with_context(
        capture_mode,
        context_session_id.as_deref(),
    )
    .ok();
    let _desktop_context_collector = context_session_id.as_ref().and_then(|session_id| {
        match minutes_core::desktop_context::DesktopContextCollector::start(
            session_id.clone(),
            minutes_core::desktop_context::DesktopContextSessionKind::Recording,
            config.desktop_context.clone(),
        ) {
            Ok(collector) => Some(collector),
            Err(error) => {
                tracing::warn!(error = %error, "desktop context collector unavailable for CLI recording");
                None
            }
        }
    });

    // Save recording start time (for timestamping notes)
    minutes_core::notes::save_recording_start()?;
    minutes_core::notes::clear_consent();
    if let Some(recording_consent) = recording_consent.as_ref() {
        minutes_core::notes::save_consent(
            Some(recording_consent.basis),
            recording_consent.notice.as_deref(),
        )?;
    }

    // Save pre-meeting context if provided
    if let Some(ref ctx) = context {
        minutes_core::notes::save_context(ctx)?;
        eprintln!("Context saved: {}", ctx);
    }

    match capture_mode {
        CaptureMode::Meeting => {
            eprintln!("Recording meeting... (press Ctrl-C or run `minutes stop` to finish)");
            eprintln!("  Tip: add notes with `minutes note \"your note\"` in another terminal");
        }
        CaptureMode::QuickThought => {
            eprintln!("Recording quick thought... (press Ctrl-C or run `minutes stop` to finish)");
            eprintln!("  Tip: speak one idea clearly — it will save as a normal memo artifact");
        }
        CaptureMode::Dictation => {
            eprintln!("Use `minutes dictate` for dictation mode.");
        }
        CaptureMode::LiveTranscript => {
            eprintln!("Use `minutes live` for live transcript mode.");
        }
    }

    // Set up stop flag for signal handler (double Ctrl+C to force quit)
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = std::sync::Arc::clone(&stop_flag);
    ctrlc::set_handler(move || {
        if let InterruptAction::ForceExit(code) = handle_graceful_interrupt(
            &stop_clone,
            "Stopping recording... (Ctrl+C again to force quit)",
        ) {
            std::process::exit(code);
        }
    })?;

    // Ignore SIGTERM — `minutes stop` uses sentinel file for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    // Record audio from default input device
    let wav_path = minutes_core::pid::current_wav_path();
    minutes_core::capture::record_to_wav_with_lifecycle(
        &wav_path,
        stop_flag,
        config,
        Some(minutes_core::capture::RecordingStartedContext {
            session_id: context_session_id.clone(),
            source: "capture".into(),
            capabilities: vec![
                "audio.capture".into(),
                "live.utterance.final".into(),
                format!("mode.{}", capture_mode.noun().replace(' ', "-")),
                format!("intent.{}", preflight.intent.as_str()),
            ],
        }),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;
    let recording_finished_at = Local::now();
    let user_notes = minutes_core::notes::read_notes();
    let pre_context = minutes_core::notes::read_context();
    let (consent, consent_notice) = if capture_mode == CaptureMode::Meeting {
        minutes_core::notes::load_consent()
    } else {
        (None, None)
    };
    // Don't block the stop path with a calendar query (can take 10s if Calendar.app hangs).
    // The pipeline already falls back to events_overlapping_now() during background processing.
    let calendar_event = None;
    let queued = (|| -> Result<(minutes_core::jobs::ProcessingJob, String)> {
        let job = minutes_core::jobs::queue_live_capture_with_recording_health(
            capture_mode,
            title.clone(),
            &wav_path,
            user_notes,
            pre_context,
            Some(recording_started_at),
            Some(recording_finished_at),
            context_session_id.clone(),
            calendar_event,
            template_slug.clone(),
            consent,
            consent_notice,
            startup_recording_health.clone(),
        )?;

        let queued_result = serde_json::to_string_pretty(&serde_json::json!({
            "status": "queued",
            "job_id": job.id,
            "title": job.title,
            "mode": mode,
        }))?;

        if let Err(error) = std::fs::write(minutes_core::pid::last_result_path(), &queued_result) {
            tracing::warn!(error = %error, "failed to persist queued result summary");
        }

        minutes_core::pid::set_processing_status(
            job.stage.as_deref(),
            Some(capture_mode),
            job.title.as_deref(),
            Some(&job.id),
            minutes_core::jobs::active_job_count(),
        )
        .ok();

        spawn_queue_worker()?;
        Ok((job, queued_result))
    })();

    if let Err(error) = &queued {
        if let Some(session_id) = context_session_id.as_deref() {
            if let Err(mark_error) = minutes_core::context_store::mark_capture_session_failed(
                session_id,
                Some(recording_finished_at),
                &error.to_string(),
                None,
            ) {
                tracing::warn!(
                    session_id,
                    error = %mark_error,
                    "failed to mark context session after queue error"
                );
            }
        }
    }

    cleanup_live_capture_state();

    let (job, queued_result) = queued?;
    eprintln!(
        "Queued {} processing{}.",
        capture_mode.noun(),
        job.title
            .as_ref()
            .map(|title| format!(" for {}", title))
            .unwrap_or_default()
    );
    println!("{}", queued_result);

    Ok(())
}

fn spawn_queue_worker() -> Result<()> {
    if minutes_core::jobs::worker_active() {
        return Ok(());
    }

    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("process-queue")
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        )
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let _ = child.id();
    Ok(())
}

fn cmd_mic_toggle(force_state: Option<&str>) -> Result<()> {
    let new_state = match force_state {
        Some("on") => minutes_core::streaming::set_mic_muted_with_sentinel(true),
        Some("off") => minutes_core::streaming::set_mic_muted_with_sentinel(false),
        _ => minutes_core::streaming::toggle_mic_mute_with_sentinel(),
    };
    if new_state {
        println!("mic muted — system audio still capturing");
    } else {
        println!("mic unmuted");
    }
    if !minutes_core::pid::status().recording {
        eprintln!(
            "[minutes] No active recording — the sentinel is set and will take effect on the next dual-source `minutes record`."
        );
    }
    Ok(())
}

fn cmd_stop(config: &Config) -> Result<()> {
    match minutes_core::pid::check_recording() {
        Ok(Some(pid)) => {
            let capture_mode = minutes_core::pid::read_recording_metadata()
                .map(|meta| meta.mode)
                .unwrap_or(CaptureMode::Meeting);
            eprintln!("Stopping recording (PID {})...", pid);

            // Write sentinel file (cross-platform stop mechanism)
            minutes_core::pid::write_stop_sentinel()
                .map_err(|e| anyhow::anyhow!("failed to write stop sentinel: {}", e))?;

            // On Unix, also send SIGTERM for instant stop
            #[cfg(unix)]
            {
                if minutes_core::desktop_control::desktop_app_owns_pid(pid) {
                    tracing::info!(
                        pid,
                        "recording is owned by the desktop app; using sentinel-only stop"
                    );
                } else {
                    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    if rc != 0 {
                        let err = std::io::Error::last_os_error();
                        tracing::warn!(
                            "SIGTERM failed (PID {}): {} — sentinel file will stop recording",
                            pid,
                            err
                        );
                    }
                }
            }

            // Poll for PID file removal with progress feedback
            let timeout = std::time::Duration::from_secs(120);
            let start = std::time::Instant::now();
            let pid_path = minutes_core::pid::pid_path();

            eprint!("Processing {}", capture_mode.noun());
            while pid_path.exists() && start.elapsed() < timeout {
                std::thread::sleep(std::time::Duration::from_secs(1));
                eprint!(".");
            }
            eprintln!();

            if pid_path.exists() {
                anyhow::bail!("recording process did not stop within 120 seconds");
            }

            // Read result from the recording process
            let result_path = minutes_core::pid::last_result_path();
            if result_path.exists() {
                let result = std::fs::read_to_string(&result_path)?;
                println!("{}", result);
                std::fs::remove_file(&result_path).ok();

                // Update relationship graph index
                if let Err(e) = minutes_core::graph::rebuild_index(config) {
                    tracing::warn!(error = %e, "graph index rebuild failed (non-fatal)");
                }
            } else {
                let active_jobs = minutes_core::jobs::active_jobs();
                if let Some(job) = active_jobs.first() {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&serde_json::json!({
                            "status": "queued",
                            "job_id": job.id,
                            "title": job.title,
                            "mode": job.mode,
                        }))?
                    );
                } else {
                    eprintln!("Recording stopped but no result file found.");
                }
            }

            Ok(())
        }
        Ok(None) => {
            // No batch recording — check for live transcript session.
            // `inspect_pid_file` so a session holding the PID under a mandatory
            // Windows lock is detected (the PID is unreadable there, but the stop
            // sentinel — polled inline by the live loop — stops it on any
            // platform). See #258.
            let lt_pid_path = minutes_core::pid::live_transcript_pid_path();
            let lt_state = minutes_core::pid::inspect_pid_file(&lt_pid_path);
            if lt_state.is_active() {
                match lt_state.pid() {
                    Some(pid) => eprintln!("Stopping live transcript (PID {})...", pid),
                    None => eprintln!("Stopping live transcript..."),
                }
                minutes_core::pid::write_stop_sentinel()
                    .map_err(|e| anyhow::anyhow!("failed to write stop sentinel: {}", e))?;
                #[cfg(unix)]
                if let Some(pid) = lt_state.pid() {
                    let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                    if rc != 0 {
                        tracing::warn!("SIGTERM failed for live transcript PID {}", pid);
                    }
                }
                // Poll for PID removal
                let start = std::time::Instant::now();
                eprint!("Finalizing live transcript");
                while lt_pid_path.exists() && start.elapsed() < std::time::Duration::from_secs(30) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    eprint!(".");
                }
                eprintln!();
                if lt_pid_path.exists() {
                    anyhow::bail!("live transcript process did not stop within 30 seconds");
                }
                eprintln!("Live transcript stopped.");
                Ok(())
            } else {
                eprintln!("No recording or live transcript in progress.");
                Ok(())
            }
        }
        Err(e) => Err(anyhow::anyhow!("{}", e)),
    }
}

fn cmd_process_queue(config: &Config) -> Result<()> {
    minutes_core::jobs::process_pending_jobs(config, |_| {})?;
    Ok(())
}

fn cmd_status() -> Result<()> {
    let status = minutes_core::pid::status();
    let json = serde_json::to_string_pretty(&status)?;
    println!("{}", json);
    Ok(())
}

fn cmd_jobs(include_terminal: bool, json_mode: bool, limit: usize) -> Result<()> {
    let jobs = minutes_core::jobs::display_jobs(Some(limit), include_terminal);

    if json_mode {
        println!("{}", serde_json::to_string_pretty(&jobs)?);
        return Ok(());
    }

    if jobs.is_empty() {
        println!("No processing jobs.");
        return Ok(());
    }

    for job in jobs {
        let mode = match job.mode {
            CaptureMode::Meeting => "meeting",
            CaptureMode::QuickThought => "quick thought",
            CaptureMode::Dictation => "dictation",
            CaptureMode::LiveTranscript => "live transcript",
        };
        let title = job.title.unwrap_or_else(|| match job.mode {
            CaptureMode::Meeting => "Meeting recording".into(),
            CaptureMode::QuickThought => "Quick thought".into(),
            CaptureMode::Dictation => "Dictation".into(),
            CaptureMode::LiveTranscript => "Live transcript".into(),
        });
        let state = match job.state {
            minutes_core::jobs::JobState::Queued => "queued",
            minutes_core::jobs::JobState::Transcribing => "transcribing",
            minutes_core::jobs::JobState::TranscriptOnly => "transcript-ready",
            minutes_core::jobs::JobState::Diarizing => "diarizing",
            minutes_core::jobs::JobState::Summarizing => "summarizing",
            minutes_core::jobs::JobState::Saving => "saving",
            minutes_core::jobs::JobState::NeedsReview => "needs-review",
            minutes_core::jobs::JobState::Complete => "complete",
            minutes_core::jobs::JobState::Failed => "failed",
        };

        println!("{}  {}  {}", job.id, state, title);
        println!("  mode: {}", mode);
        if let Some(stage) = job.stage {
            println!("  stage: {}", stage);
        }
        if let Some(path) = job.output_path {
            println!("  output: {}", path);
        }
        if let Some(words) = job.word_count {
            println!("  words: {}", words);
        }
        if let Some(error) = job.error {
            println!("  error: {}", error);
        }
        println!("  created: {}", job.created_at.to_rfc3339());
        println!("  audio: {}", job.audio_path);
        println!();
    }

    Ok(())
}

fn automation_runs_dir() -> PathBuf {
    Config::minutes_dir().join("automation-runs")
}

fn build_weekly_summary_markdown(config: &Config) -> Result<String> {
    let since = (Local::now() - chrono::Duration::days(7)).to_rfc3339();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: Some(since),
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
        include_restricted: false,
    };

    let meetings = minutes_core::search::search("", config, &filters)?;
    let consistency = minutes_core::search::consistency_report(config, None, 7)?;
    let open_actions = minutes_core::search::find_open_actions(config, None, false)?;

    let recent_titles = if meetings.is_empty() {
        "- No meetings or memos in the last 7 days.".to_string()
    } else {
        meetings
            .iter()
            .take(6)
            .map(|meeting| format!("- {} ({})", meeting.title, meeting.date))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let decision_conflicts = if consistency.decision_conflicts.is_empty() {
        "- No conflicting decision arcs detected.".to_string()
    } else {
        consistency
            .decision_conflicts
            .iter()
            .take(5)
            .map(|conflict| format!("- {} -> {}", conflict.topic, conflict.latest.what))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let stale_commitments = if consistency.stale_commitments.is_empty() {
        "- No stale commitments detected.".to_string()
    } else {
        consistency
            .stale_commitments
            .iter()
            .take(5)
            .map(|item| {
                format!(
                    "- {}{}",
                    item.entry.what,
                    item.entry
                        .who
                        .as_ref()
                        .map(|who| format!(" ({who})"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let open_actions_block = if open_actions.is_empty() {
        "- No open action items found.".to_string()
    } else {
        open_actions
            .iter()
            .take(6)
            .map(|item| {
                format!(
                    "- {}: {}{}",
                    item.assignee,
                    item.task,
                    item.due
                        .as_ref()
                        .map(|due| format!(" (due {due})"))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Ok(format!(
        "# Weekly Summary\n\n## Volume\n\n- {} meeting or memo artifact(s) in the last 7 days.\n\n## Recent Meetings\n\n{}\n\n## Decision Arcs\n\n{}\n\n## Stale Commitments\n\n{}\n\n## Open Actions\n\n{}\n",
        meetings.len(),
        recent_titles,
        decision_conflicts,
        stale_commitments,
        open_actions_block
    ))
}

fn build_proactive_context_markdown(config: &Config) -> Result<String> {
    let since = (Local::now() - chrono::Duration::days(7)).to_rfc3339();
    let filters = minutes_core::search::SearchFilters {
        content_type: None,
        since: Some(since),
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
        include_restricted: false,
    };
    let recent_results = minutes_core::search::search("", config, &filters)?;
    let recent_meetings = recent_results
        .iter()
        .filter(|item| item.content_type != "memo")
        .take(4)
        .map(|item| format!("- {} ({})", item.title, item.date))
        .collect::<Vec<_>>();
    let recent_memos = recent_results
        .iter()
        .filter(|item| item.content_type == "memo")
        .take(4)
        .map(|item| format!("- {} ({})", item.title, item.date))
        .collect::<Vec<_>>();
    let consistency = minutes_core::search::consistency_report(config, None, 7)?;
    let stale = consistency
        .stale_commitments
        .iter()
        .take(4)
        .map(|item| {
            format!(
                "- {}{}",
                item.entry.what,
                item.entry
                    .who
                    .as_ref()
                    .map(|who| format!(" ({who})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>();

    Ok(format!(
        "# Proactive Context\n\n## Recent Meetings\n\n{}\n\n## Recent Memos\n\n{}\n\n## Stale Commitments\n\n{}\n",
        if recent_meetings.is_empty() { "- No recent meetings.".to_string() } else { recent_meetings.join("\n") },
        if recent_memos.is_empty() { "- No recent memos.".to_string() } else { recent_memos.join("\n") },
        if stale.is_empty() { "- No stale commitments.".to_string() } else { stale.join("\n") },
    ))
}

fn build_delivery_payload(
    kind: &str,
    target: &str,
    source_path: &Path,
    markdown: &str,
) -> Result<String> {
    let source = source_path.display().to_string();
    match target {
        "slack-json" => Ok(serde_json::to_string_pretty(&serde_json::json!({
            "delivery_target": "slack-json",
            "kind": kind,
            "source_artifact": source,
            "mode": "draft-only",
            "text": markdown,
        }))?),
        "email-markdown" => Ok(format!(
            "# Email Draft Payload\n\n- delivery_target: email-markdown\n- kind: {kind}\n- source_artifact: {source}\n- mode: draft-only\n\n## Body\n\n{markdown}"
        )),
        other => anyhow::bail!("unsupported delivery target: {}", other),
    }
}

fn cmd_automate(
    kind: &str,
    output: Option<PathBuf>,
    delivery_target: Option<&str>,
    json: bool,
    config: &Config,
) -> Result<()> {
    let markdown = match kind {
        "weekly-summary" => build_weekly_summary_markdown(config)?,
        "proactive-context" => build_proactive_context_markdown(config)?,
        other => anyhow::bail!("unsupported automation kind: {}", other),
    };

    let output_path = output.unwrap_or_else(|| {
        automation_runs_dir().join(format!(
            "{}-{}.md",
            Local::now().format("%Y-%m-%d-%H%M%S"),
            kind
        ))
    });
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output_path, markdown)?;

    let delivery_payload_path = if let Some(target) = delivery_target {
        let ext = if target == "slack-json" {
            "delivery.json"
        } else {
            "delivery.md"
        };
        let payload_path = output_path.with_extension(ext);
        let payload = build_delivery_payload(
            kind,
            target,
            &output_path,
            &std::fs::read_to_string(&output_path)?,
        )?;
        std::fs::write(&payload_path, payload)?;
        Some(payload_path)
    } else {
        None
    };

    let record = AutomationRunRecord {
        kind: kind.to_string(),
        status: "ok".into(),
        output_path: output_path.display().to_string(),
        delivery_target: delivery_target.map(str::to_string),
        delivery_payload_path: delivery_payload_path
            .as_ref()
            .map(|path| path.display().to_string()),
        generated_at: Local::now().to_rfc3339(),
    };

    let run_record_path = output_path.with_extension("json");
    std::fs::write(&run_record_path, serde_json::to_string_pretty(&record)?)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&record)?);
    } else {
        eprintln!("Automation run complete: {}", kind);
        eprintln!("  markdown: {}", output_path.display());
        eprintln!("  record: {}", run_record_path.display());
        if let Some(ref payload_path) = delivery_payload_path {
            eprintln!("  delivery payload: {}", payload_path.display());
        }
        println!("{}", serde_json::to_string_pretty(&record)?);
    }

    Ok(())
}

#[derive(Serialize)]
struct PathsReport {
    config_path: PathBuf,
    minutes_dir: PathBuf,
    output_dir: PathBuf,
}

fn cmd_paths(json: bool, config: &Config) -> Result<()> {
    let report = PathsReport {
        config_path: Config::config_path(),
        minutes_dir: Config::minutes_dir(),
        output_dir: config.output_dir.clone(),
    };

    if json {
        let envelope = json_envelope("minutes paths", report);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        println!("config_path: {}", report.config_path.display());
        println!("minutes_dir: {}", report.minutes_dir.display());
        println!("output_dir: {}", report.output_dir.display());
    }

    Ok(())
}

#[derive(Serialize)]
struct CleanupError {
    path: PathBuf,
    error: String,
}

#[derive(Serialize)]
struct CleanupReport {
    plan: minutes_core::retention::RetentionPlan,
    applied: bool,
    removed: Vec<PathBuf>,
    errors: Vec<CleanupError>,
}

fn cmd_storage(json: bool, config: &Config) -> Result<()> {
    let plan = minutes_core::retention::preview_audio_retention(config, Local::now());
    if json {
        let envelope = json_envelope("minutes storage", plan);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        print_storage_summary(&plan, config);
    }
    Ok(())
}

fn cmd_cleanup(apply: bool, older_than: Option<&str>, json: bool, config: &Config) -> Result<()> {
    let mut effective_config = config.clone();
    if let Some(value) = older_than {
        effective_config.retention.successful_audio_days = parse_retention_days(value)?;
    }

    let plan = minutes_core::retention::preview_audio_retention(&effective_config, Local::now());
    let mut report = CleanupReport {
        plan,
        applied: apply,
        removed: Vec::new(),
        errors: Vec::new(),
    };

    if apply {
        for item in
            report.plan.items.iter().filter(|item| {
                item.action == minutes_core::retention::RetentionAction::DeleteCandidate
            })
        {
            match std::fs::remove_file(&item.path) {
                Ok(()) => report.removed.push(item.path.clone()),
                Err(error) => report.errors.push(CleanupError {
                    path: item.path.clone(),
                    error: error.to_string(),
                }),
            }
        }
    }

    if json {
        let envelope = json_envelope("minutes cleanup", report);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        print_cleanup_summary(&report, &effective_config);
    }
    Ok(())
}

fn print_storage_summary(plan: &minutes_core::retention::RetentionPlan, config: &Config) {
    println!("Minutes storage");
    println!("  output_dir: {}", plan.output_dir.display());
    println!(
        "  raw audio: {} across {} file(s)",
        format_bytes(plan.totals.raw_audio_bytes),
        plan.totals.raw_audio_files
    );
    println!(
        "  cleanup candidates: {} across {} file(s)",
        format_bytes(plan.totals.delete_candidate_bytes),
        plan.totals.delete_candidate_files
    );
    println!(
        "  policy: successful audio {}d, failed/needs-review audio {}d, restricted audio {}d, pinned audio kept",
        config.retention.successful_audio_days,
        config.retention.failed_audio_days,
        config.retention.restricted_audio_days
    );
}

fn print_cleanup_summary(report: &CleanupReport, config: &Config) {
    if report.applied {
        println!("Minutes cleanup applied");
        println!("  removed: {} file(s)", report.removed.len());
        if !report.errors.is_empty() {
            println!("  errors: {} file(s)", report.errors.len());
        }
    } else {
        println!("Minutes cleanup preview");
        println!("  no files deleted; pass --apply to remove candidates");
    }
    println!(
        "  candidates: {} across {} file(s)",
        format_bytes(report.plan.totals.delete_candidate_bytes),
        report.plan.totals.delete_candidate_files
    );
    println!(
        "  policy: successful audio {}d, failed/needs-review audio {}d, restricted audio {}d",
        config.retention.successful_audio_days,
        config.retention.failed_audio_days,
        config.retention.restricted_audio_days
    );
}

fn parse_retention_days(value: &str) -> Result<u32> {
    let trimmed = value.trim().to_ascii_lowercase();
    let digits = trimmed
        .strip_suffix("days")
        .or_else(|| trimmed.strip_suffix("day"))
        .or_else(|| trimmed.strip_suffix('d'))
        .unwrap_or(&trimmed)
        .trim();
    let days = digits
        .parse::<u32>()
        .map_err(|_| anyhow::anyhow!("invalid duration '{}'; use values like 14d or 30d", value))?;
    Ok(days)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{:.1} {}", value, UNITS[unit])
    }
}

fn owner_display(
    who: Option<&str>,
    who_original: Option<&str>,
    who_provenance: Option<&str>,
) -> String {
    let owner = who.unwrap_or("unassigned");
    match (who_original, who_provenance) {
        (Some(original), Some(provenance)) if original != owner => {
            format!("{owner} ({provenance}: {original})")
        }
        _ => owner.to_string(),
    }
}

#[allow(clippy::too_many_arguments)]
/// Resolve `--sync` / `--no-sync` clap flags into a `SyncMode`. Both flags
/// have `conflicts_with` so clap rejects passing both; the unset case falls
/// through to `Auto` (per-file mtime+size scan), which is the right default
/// for a CLI invocation that wants fresh data without forcing a full rebuild.
fn resolve_sync_mode(sync: bool, no_sync: bool) -> minutes_core::search_index::SyncMode {
    if sync {
        minutes_core::search_index::SyncMode::Force
    } else if no_sync {
        minutes_core::search_index::SyncMode::Skip
    } else {
        minutes_core::search_index::SyncMode::Auto
    }
}

/// Record an explicit `--include-restricted` override on the event bus
/// before any results are returned, so the bypass is never silent (consent
/// layer Wave 2). Best-effort by design: a failed append warns on stderr but
/// never blocks the caller (headless/hook discipline).
fn record_sensitivity_override(surface: &str, query: Option<&str>) {
    if let Err(error) = minutes_core::events::append_event_strict(
        minutes_core::events::MinutesEvent::SensitivityOverride {
            surface: surface.to_string(),
            query: query.map(|q| q.to_string()),
        },
    ) {
        eprintln!(
            "WARN: could not record sensitivity.override event: {}",
            error
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn cmd_search(
    query: &str,
    content_type: Option<String>,
    since: Option<String>,
    limit: usize,
    intents_only: bool,
    intent_kind: Option<String>,
    owner: Option<String>,
    format: &str,
    sync_mode: minutes_core::search_index::SyncMode,
    include_restricted: bool,
    config: &Config,
) -> Result<()> {
    let json_mode = format == "json";
    if include_restricted {
        record_sensitivity_override("cli.search", Some(query));
    }
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee: None,
        intent_kind: intent_kind.as_deref().map(parse_intent_kind).transpose()?,
        owner,
        recorded_by: None,
        include_restricted,
    };

    if intents_only {
        let results = minutes_core::search::search_intents(query, config, &filters)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let limited: Vec<_> = results.into_iter().take(limit).collect();

        if limited.is_empty() {
            if json_mode {
                // In JSON mode, output nothing (empty JSONL)
            } else {
                eprintln!("No intent records found for \"{}\"", query);
                println!("[]");
            }
            return Ok(());
        }

        if json_mode {
            // JSONL: one JSON object per line
            for result in &limited {
                println!("{}", serde_json::to_string(result)?);
            }
        } else {
            for result in &limited {
                let who = owner_display(
                    result.who.as_deref(),
                    result.who_original.as_deref(),
                    result.who_provenance.as_deref(),
                );
                let due = result.by_date.as_deref().unwrap_or("no due date");
                eprintln!(
                    "\n{} — {} [{}]",
                    result.date, result.title, result.content_type
                );
                eprintln!(
                    "  {:?}: {} (@{}, {}, {})",
                    result.kind, result.what, who, result.status, due
                );
                eprintln!("  {}", result.path.display());
            }

            let json = serde_json::to_string_pretty(&limited)?;
            println!("{}", json);
        }
        return Ok(());
    }

    let results = minutes_core::search::search_with_mode(query, config, &filters, sync_mode)?;
    let limited: Vec<_> = results.into_iter().take(limit).collect();

    if limited.is_empty() {
        if json_mode {
            // In JSON mode, output nothing (empty JSONL)
        } else {
            eprintln!("No results found for \"{}\"", query);
            println!("[]");
        }
        return Ok(());
    }

    if json_mode {
        // JSONL: one JSON object per line
        for result in &limited {
            println!("{}", serde_json::to_string(result)?);
        }
    } else {
        for result in &limited {
            eprintln!(
                "\n{} — {} [{}]",
                result.date, result.title, result.content_type
            );
            if !result.snippet.is_empty() {
                eprintln!("  {}", result.snippet);
            }
            eprintln!("  {}", result.path.display());
        }

        // Also output JSON for programmatic use
        let json = serde_json::to_string_pretty(&limited)?;
        println!("{}", json);
    }
    Ok(())
}

fn cmd_actions(assignee: Option<&str>, include_restricted: bool, config: &Config) -> Result<()> {
    if include_restricted {
        record_sensitivity_override("cli.actions", assignee);
    }
    let results = minutes_core::search::find_open_actions(config, assignee, include_restricted)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    if results.is_empty() {
        eprintln!("No open action items found.");
        println!("[]");
        return Ok(());
    }

    eprintln!("Open action items ({}):", results.len());
    for item in &results {
        let due = item.due.as_deref().unwrap_or("no due date");
        eprintln!("  @{}: {} ({})", item.assignee, item.task, due);
        eprintln!("    from: {} — {}", item.meeting_date, item.meeting_title);
    }

    let json = serde_json::to_string_pretty(&results)?;
    println!("{}", json);
    Ok(())
}

fn cmd_list(
    limit: usize,
    content_type: Option<String>,
    sync_mode: minutes_core::search_index::SyncMode,
    include_restricted: bool,
    config: &Config,
) -> Result<()> {
    // List delegates to search with an empty query — DRY, no duplicated file walking
    cmd_search(
        "",
        content_type,
        None,
        limit,
        false,
        None,
        None,
        "text",
        sync_mode,
        include_restricted,
        config,
    )
}

fn cmd_export(
    content_type: Option<String>,
    output: Option<PathBuf>,
    config: &Config,
) -> Result<()> {
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
        include_restricted: true,
    };

    // Reuse search with empty query to get all meetings
    let results = minutes_core::search::search("", config, &filters)?;

    // Build CSV writer (to file or stdout)
    let mut wtr: Box<dyn std::io::Write> = if let Some(ref path) = output {
        Box::new(std::fs::File::create(path)?)
    } else {
        Box::new(std::io::stdout())
    };

    let mut csv_wtr = csv::Writer::from_writer(&mut wtr);
    csv_wtr.write_record(["date", "title", "type", "duration", "path"])?;

    for result in &results {
        // Parse frontmatter to get duration
        let content = std::fs::read_to_string(&result.path).unwrap_or_default();
        let (fm_str, _) = minutes_core::markdown::split_frontmatter(&content);
        let duration =
            minutes_core::markdown::extract_field(fm_str, "duration").unwrap_or_default();

        csv_wtr.write_record([
            &result.date,
            &result.title,
            &result.content_type,
            &duration,
            &result.path.display().to_string(),
        ])?;
    }

    csv_wtr.flush()?;

    let count = results.len();
    if let Some(ref path) = output {
        eprintln!("Exported {} meetings to {}", count, path.display());
    } else {
        eprintln!("Exported {} meetings", count);
    }

    Ok(())
}

fn cmd_consistency(owner: Option<&str>, stale_after_days: i64, config: &Config) -> Result<()> {
    let report = minutes_core::search::consistency_report(config, owner, stale_after_days)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    if report.decision_conflicts.is_empty() && report.stale_commitments.is_empty() {
        eprintln!("No consistency issues found.");
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if !report.decision_conflicts.is_empty() {
        eprintln!("Decision conflicts ({}):", report.decision_conflicts.len());
        for conflict in &report.decision_conflicts {
            eprintln!("  topic: {}", conflict.topic);
            eprintln!(
                "  latest: {} — {}",
                conflict.latest.title, conflict.latest.what
            );
            for previous in &conflict.previous {
                eprintln!("  previous: {} — {}", previous.title, previous.what);
            }
            eprintln!("  {}", conflict.latest.path.display());
        }
    }

    if !report.stale_commitments.is_empty() {
        eprintln!("\nStale commitments ({}):", report.stale_commitments.len());
        for stale in &report.stale_commitments {
            let who = owner_display(
                stale.entry.who.as_deref(),
                stale.entry.who_original.as_deref(),
                stale.entry.who_provenance.as_deref(),
            );
            let due = stale.entry.by_date.as_deref().unwrap_or("no due date");
            let reasons = stale.reasons.join(", ");
            eprintln!(
                "  {:?}: {} (@{}, {}, {} days old, {} meetings since)",
                stale.kind, stale.entry.what, who, due, stale.age_days, stale.meetings_since
            );
            eprintln!("    why: {}", reasons);
            if let Some(follow_up) = &stale.latest_follow_up {
                eprintln!(
                    "    latest follow-up: {} — {}",
                    follow_up.date, follow_up.title
                );
            }
            eprintln!("  from: {} — {}", stale.entry.date, stale.entry.title);
            eprintln!("  {}", stale.entry.path.display());
        }
    }

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn cmd_person(name: &str, config: &Config) -> Result<()> {
    let profile =
        minutes_core::search::person_profile(config, name).map_err(|e| anyhow::anyhow!("{}", e))?;

    if profile.recent_meetings.is_empty()
        && profile.open_intents.is_empty()
        && profile.recent_decisions.is_empty()
    {
        eprintln!("No profile data found for {}.", name);
        println!("{}", serde_json::to_string_pretty(&profile)?);
        return Ok(());
    }

    eprintln!("Profile for {}:", profile.name);
    if !profile.top_topics.is_empty() {
        eprintln!(
            "  Top topics: {}",
            profile
                .top_topics
                .iter()
                .map(|topic| format!("{} ({})", topic.topic, topic.count))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !profile.open_intents.is_empty() {
        eprintln!("  Open commitments/actions: {}", profile.open_intents.len());
    }
    if !profile.recent_decisions.is_empty() {
        eprintln!("  Recent decisions: {}", profile.recent_decisions.len());
    }
    if !profile.recent_meetings.is_empty() {
        eprintln!("  Recent meetings:");
        for meeting in &profile.recent_meetings {
            eprintln!("    {} — {}", meeting.date, meeting.title);
        }
    }

    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

/// Shell-quote a token for a copy-pasteable command (slugs rarely need it).
fn shell_quote_arg(s: &str) -> String {
    if !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Render the ready-to-run merge command for a suggested cluster. `slugs[0]` is
/// the highest-evidence spelling (canonical); the rest fold into it.
fn format_merge_command(slugs: &[String]) -> String {
    let quoted: Vec<String> = slugs.iter().map(|s| shell_quote_arg(s)).collect();
    format!("minutes people merge {}", quoted.join(" "))
}

#[derive(Serialize)]
struct PeopleMergeOutput {
    canonical: String,
    aliases: Vec<String>,
    unresolved: Vec<String>,
    saved: bool,
    rebuilt: bool,
    people_before: Option<usize>,
    people_after: Option<usize>,
}

/// Resolve a slug-or-name token to a graph person's display name. Exact match on
/// slug or (case-insensitively) on name; never fuzzy. Returns `(name, resolved)`;
/// an unresolved token is passed through literally so a person not yet in the
/// graph can still be named, but the caller warns about it.
fn resolve_person_token(
    token: &str,
    people: &[minutes_core::graph::PersonSummary],
) -> (String, bool) {
    let t = token.trim();
    if let Some(p) = people.iter().find(|p| p.slug.eq_ignore_ascii_case(t)) {
        return (p.name.clone(), true);
    }
    let name_matches: Vec<&minutes_core::graph::PersonSummary> = people
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(t))
        .collect();
    if name_matches.len() == 1 {
        return (name_matches[0].name.clone(), true);
    }
    (t.to_string(), false)
}

fn cmd_people_merge(
    canonical: &str,
    aliases: &[String],
    no_rebuild: bool,
    json: bool,
    config: &Config,
) -> Result<()> {
    use minutes_core::graph;
    use minutes_core::vocabulary;

    // Best-effort: resolve slugs/names against the current graph. If the graph
    // isn't built yet, fall back to treating every token as a literal name.
    let people = graph::relationship_map(config).unwrap_or_default();

    let (canonical_name, canonical_resolved) = resolve_person_token(canonical, &people);
    let mut unresolved: Vec<String> = Vec::new();
    if !canonical_resolved {
        unresolved.push(canonical.trim().to_string());
    }

    // Resolve aliases; drop any that equal the canonical after resolution.
    let mut alias_names: Vec<String> = Vec::new();
    for a in aliases {
        let (name, resolved) = resolve_person_token(a, &people);
        if !resolved {
            unresolved.push(a.trim().to_string());
        }
        if name.eq_ignore_ascii_case(&canonical_name)
            || alias_names.iter().any(|n| n.eq_ignore_ascii_case(&name))
        {
            continue;
        }
        alias_names.push(name);
    }

    if alias_names.is_empty() {
        anyhow::bail!(
            "nothing to merge: no variant distinct from canonical \"{}\"",
            canonical_name
        );
    }

    if !unresolved.is_empty() {
        eprintln!(
            "  Note: not currently in the graph, treated as literal name(s): {}",
            unresolved.join(", ")
        );
    }

    // Persist the confirmed merge as a Person vocabulary entry (canonical +
    // variant aliases). On rebuild the canonicalizer routes every variant to the
    // canonical slug; the vocabulary survives the graph.db wipe.
    let path = vocabulary::default_path();
    let mut store = vocabulary::load().map_err(|e| anyhow::anyhow!("{}", e))?;
    let now = Local::now().to_rfc3339();
    store.entries.push(vocabulary::VocabularyEntry {
        kind: vocabulary::VocabularyKind::Person,
        canonical: canonical_name.clone(),
        aliases: alias_names.clone(),
        priority: vocabulary::VocabularyPriority::Normal,
        source: vocabulary::VocabularySource::Manual,
        created_at: Some(now.clone()),
        updated_at: Some(now),
        ..vocabulary::VocabularyEntry::default()
    });

    let normalized = match store.normalized() {
        Ok(s) => s,
        Err(e) => {
            // Fail closed on alias conflict; name the offending entry + recovery.
            if let vocabulary::VocabularyError::AliasConflict {
                alias, existing, ..
            } = &e
            {
                let existing_id = vocabulary::load()
                    .ok()
                    .and_then(|s| {
                        s.entries
                            .into_iter()
                            .find(|entry| {
                                entry.kind == vocabulary::VocabularyKind::Person
                                    && entry.canonical.eq_ignore_ascii_case(existing)
                            })
                            .map(|entry| entry.id)
                    })
                    .unwrap_or_default();
                anyhow::bail!(
                    "\"{alias}\" is already a confirmed variant of \"{existing}\"{}. \
                     Merge into \"{existing}\" instead (make it the canonical), or remove that entry first{}.",
                    if existing_id.is_empty() { String::new() } else { format!(" (vocabulary entry {existing_id})") },
                    if existing_id.is_empty() { String::new() } else { format!(": minutes vocabulary remove {existing_id}") }
                );
            }
            return Err(anyhow::anyhow!("{}", e));
        }
    };
    vocabulary::save_at(&path, &normalized).map_err(|e| anyhow::anyhow!("{}", e))?;

    let people_before = Some(people.len());
    let mut rebuilt = false;
    let mut people_after = None;
    if !no_rebuild {
        match graph::rebuild_index(config) {
            Ok(stats) => {
                rebuilt = true;
                people_after = Some(stats.people_count);
            }
            Err(e) => {
                eprintln!(
                    "  Saved the merge, but the graph rebuild failed ({e}). \
                     Run `minutes people --rebuild` to apply it."
                );
            }
        }
    }

    if json {
        let out = PeopleMergeOutput {
            canonical: canonical_name,
            aliases: alias_names,
            unresolved,
            saved: true,
            rebuilt,
            people_before,
            people_after,
        };
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        eprintln!(
            "Merged {} variant(s) into \"{}\": {}",
            alias_names.len(),
            canonical_name,
            alias_names.join(", ")
        );
        if rebuilt {
            if let (Some(b), Some(a)) = (people_before, people_after) {
                eprintln!("  Graph rebuilt: {b} → {a} people.");
            }
        } else if no_rebuild {
            eprintln!("  Not rebuilt (--no-rebuild). Run `minutes people --rebuild` to apply.");
        }
    }
    Ok(())
}

fn cmd_people(rebuild: bool, json: bool, limit: usize, config: &Config) -> Result<()> {
    use minutes_core::graph;

    if rebuild || !graph::db_path().exists() {
        eprintln!("Building relationship index...");
        let stats = graph::rebuild_index(config).map_err(|e| anyhow::anyhow!("{}", e))?;
        eprintln!(
            "Index rebuilt: {} people, {} meetings, {} commitments in {}ms",
            stats.people_count, stats.meeting_count, stats.commitment_count, stats.rebuild_ms
        );
        if !stats.alias_clusters.is_empty() {
            eprintln!("\nPossible name variants (same person, different spellings?):");
            for cluster in &stats.alias_clusters {
                let shared = if cluster.max_shared_meetings > 0 {
                    format!(" ({} shared meetings)", cluster.max_shared_meetings)
                } else {
                    String::new()
                };
                eprintln!("  {}{}", cluster.members.join(" ↔ "), shared);
                // slugs[0] is the highest-evidence spelling (canonical); the rest
                // fold into it. This is a ready-to-run confirm-merge.
                eprintln!("    merge: {}", format_merge_command(&cluster.slugs));
            }
        }
        if !stats.alias_suggestions.is_empty() {
            eprintln!("\nPossible duplicates (shortened / last-name match):");
            for alias in &stats.alias_suggestions {
                eprintln!(
                    "  {} ↔ {} ({} shared meetings)",
                    alias.name_a, alias.name_b, alias.shared_meetings
                );
            }
        }
        eprintln!();
    }

    let all_people = graph::relationship_map(config).map_err(|e| anyhow::anyhow!("{}", e))?;
    // Apply limit to all output modes (JSON and formatted)
    let people: Vec<_> = all_people.into_iter().take(limit).collect();

    if json {
        println!("{}", serde_json::to_string_pretty(&people)?);
        return Ok(());
    }

    if people.is_empty() {
        eprintln!(
            "No people found. Record some meetings first, then run: minutes people --rebuild"
        );
        return Ok(());
    }

    // Top contacts
    eprintln!("TOP CONTACTS (by relationship score)");
    for person in people.iter().take(limit) {
        let status = if person.losing_touch {
            "\x1b[33m⚠ losing touch\x1b[0m"
        } else if person.open_commitments > 0 {
            &format!(
                "{} open commitment{}",
                person.open_commitments,
                if person.open_commitments != 1 {
                    "s"
                } else {
                    ""
                }
            )
        } else {
            "\x1b[32m✓ all clear\x1b[0m"
        };

        let last = if person.days_since < 1.0 {
            "today".to_string()
        } else if person.days_since < 2.0 {
            "yesterday".to_string()
        } else {
            format!("{}d ago", person.days_since as i64)
        };

        eprintln!(
            "  {:<20} {} meeting{}  last: {:<12} {}",
            person.name,
            person.meeting_count,
            if person.meeting_count != 1 { "s" } else { " " },
            last,
            status
        );
    }

    // Stale commitments
    let commitments =
        graph::query_commitments(config, None).map_err(|e| anyhow::anyhow!("{}", e))?;
    let stale: Vec<_> = commitments.iter().filter(|c| c.status == "stale").collect();
    if !stale.is_empty() {
        eprintln!("\nSTALE COMMITMENTS");
        for c in &stale {
            let who = c.person_name.as_deref().unwrap_or("unknown");
            eprintln!(
                "  • {} (assigned: {}, due: {})",
                c.text,
                who,
                c.due_date.as_deref().unwrap_or("no date")
            );
        }
    }

    // Losing touch
    let losing: Vec<_> = people.iter().filter(|p| p.losing_touch).collect();
    if !losing.is_empty() {
        eprintln!("\nLOSING TOUCH");
        for person in &losing {
            eprintln!(
                "  {} — {} meetings total, last seen {}d ago",
                person.name, person.meeting_count, person.days_since as i64
            );
        }
    }

    // Print JSON to stdout for programmatic consumption
    println!("{}", serde_json::to_string_pretty(&people)?);
    Ok(())
}

#[derive(Serialize)]
struct VocabularyMutationOutput {
    path: String,
    entries: Vec<minutes_core::vocabulary::VocabularyEntry>,
    note: String,
}

#[derive(Serialize)]
struct VocabularyRemoveOutput {
    path: String,
    removed: bool,
    entries: Vec<minutes_core::vocabulary::VocabularyEntry>,
}

#[derive(Serialize)]
struct VocabularySuggestion {
    canonical: String,
    kind: String,
    aliases: Vec<String>,
    reason: String,
    count: usize,
}

fn cmd_vocabulary(action: VocabularyAction, config: &Config) -> Result<()> {
    match action {
        VocabularyAction::List { json } => cmd_vocabulary_list(json),
        VocabularyAction::Add {
            kind,
            canonical,
            aliases,
            json,
        } => cmd_vocabulary_add(&kind, &canonical, aliases, json),
        VocabularyAction::Remove { id, json } => cmd_vocabulary_remove(&id, json),
        VocabularyAction::Suggest { meeting, json } => cmd_vocabulary_suggest(&meeting, json),
        VocabularyAction::Rebuild { json } => cmd_vocabulary_rebuild(json, config),
    }
}

fn cmd_vocabulary_list(json: bool) -> Result<()> {
    let path = minutes_core::vocabulary::default_path();
    let store = minutes_core::vocabulary::load().map_err(|e| anyhow::anyhow!("{}", e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&store)?);
        return Ok(());
    }

    if store.entries.is_empty() {
        eprintln!("No vocabulary entries yet.");
        eprintln!(
            "Add one with: minutes vocabulary add --kind person \"Elijah Potter\" --alias Elijah"
        );
        eprintln!("Vocabulary file: {}", path.display());
        return Ok(());
    }

    eprintln!("Vocabulary entries ({}):", store.entries.len());
    for entry in &store.entries {
        let aliases = if entry.aliases.is_empty() {
            String::new()
        } else {
            format!(" aliases: {}", entry.aliases.join(", "))
        };
        eprintln!(
            "  {} [{}] {}{}",
            entry.id,
            vocabulary_kind_label(entry.kind),
            entry.canonical,
            aliases
        );
    }
    eprintln!("Vocabulary file: {}", path.display());
    Ok(())
}

fn cmd_vocabulary_add(kind: &str, canonical: &str, aliases: Vec<String>, json: bool) -> Result<()> {
    let path = minutes_core::vocabulary::default_path();
    let mut store = minutes_core::vocabulary::load().map_err(|e| anyhow::anyhow!("{}", e))?;
    let now = Local::now().to_rfc3339();
    store
        .entries
        .push(minutes_core::vocabulary::VocabularyEntry {
            kind: parse_vocabulary_kind(kind)?,
            canonical: canonical.to_string(),
            aliases,
            priority: minutes_core::vocabulary::VocabularyPriority::Normal,
            source: minutes_core::vocabulary::VocabularySource::Manual,
            created_at: Some(now.clone()),
            updated_at: Some(now),
            ..minutes_core::vocabulary::VocabularyEntry::default()
        });
    let store = store.normalized().map_err(|e| anyhow::anyhow!("{}", e))?;
    minutes_core::vocabulary::save_at(&path, &store).map_err(|e| anyhow::anyhow!("{}", e))?;

    let output = VocabularyMutationOutput {
        path: path.display().to_string(),
        entries: store.entries,
        note: "Saved. Future transcripts, search, and graph rebuilds can use this vocabulary; existing raw transcripts stay unchanged.".into(),
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        eprintln!("Saved vocabulary entry for \"{}\".", canonical.trim());
        eprintln!("Future transcripts/search/graph rebuilds can use it.");
        eprintln!("Existing raw transcripts stay unchanged.");
    }
    Ok(())
}

fn cmd_vocabulary_remove(id: &str, json: bool) -> Result<()> {
    let path = minutes_core::vocabulary::default_path();
    let mut store = minutes_core::vocabulary::load().map_err(|e| anyhow::anyhow!("{}", e))?;
    let before = store.entries.len();
    // Match by id or by exact canonical, so non-Latin-script entries (whose id
    // is a `<kind>-<hash>` derived from an empty ASCII slug) can be removed by
    // the canonical the user typed, not just an opaque hash id. (#511)
    store
        .entries
        .retain(|entry| entry.id != id && entry.canonical != id);
    let removed = store.entries.len() != before;
    let store = store.normalized().map_err(|e| anyhow::anyhow!("{}", e))?;
    minutes_core::vocabulary::save_at(&path, &store).map_err(|e| anyhow::anyhow!("{}", e))?;

    let output = VocabularyRemoveOutput {
        path: path.display().to_string(),
        removed,
        entries: store.entries,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else if removed {
        eprintln!("Removed vocabulary entry: {}", id);
        eprintln!("Existing raw transcripts stay unchanged.");
    } else {
        eprintln!("No vocabulary entry found with id or canonical: {}", id);
    }
    Ok(())
}

fn cmd_vocabulary_suggest(meeting: &Path, json: bool) -> Result<()> {
    let content = std::fs::read_to_string(meeting)
        .map_err(|e| anyhow::anyhow!("could not read {}: {}", meeting.display(), e))?;
    let (frontmatter, body) = minutes_core::markdown::split_frontmatter(&content);
    let store = minutes_core::vocabulary::load().unwrap_or_default();
    let suggestions = vocabulary_suggestions_from_meeting(frontmatter, body, &store);

    if json {
        println!("{}", serde_json::to_string_pretty(&suggestions)?);
        return Ok(());
    }

    if suggestions.is_empty() {
        eprintln!("No vocabulary suggestions found for {}.", meeting.display());
        return Ok(());
    }

    eprintln!("Vocabulary suggestions for {}:", meeting.display());
    for suggestion in &suggestions {
        eprintln!(
            "  {} [{}] — {} (count: {})",
            suggestion.canonical, suggestion.kind, suggestion.reason, suggestion.count
        );
    }
    eprintln!("Suggestions are not applied automatically. Add one with:");
    eprintln!("  minutes vocabulary add --kind person \"Name\" --alias Alias");
    Ok(())
}

fn cmd_vocabulary_rebuild(json: bool, config: &Config) -> Result<()> {
    let stats = minutes_core::graph::rebuild_index(config).map_err(|e| anyhow::anyhow!("{}", e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        eprintln!(
            "Rebuilt graph with vocabulary context: {} people, {} meetings, {} commitments in {}ms",
            stats.people_count, stats.meeting_count, stats.commitment_count, stats.rebuild_ms
        );
        eprintln!("Existing raw transcripts stay unchanged.");
    }
    Ok(())
}

fn vocabulary_suggestions_from_meeting(
    frontmatter: &str,
    body: &str,
    store: &minutes_core::vocabulary::VocabularyStore,
) -> Vec<VocabularySuggestion> {
    let mut known = std::collections::HashSet::new();
    for entry in &store.entries {
        for form in entry.surface_forms() {
            known.insert(vocabulary_key(&form));
        }
    }

    let mut suggestions = Vec::new();
    if let Ok(frontmatter) =
        serde_yaml::from_str::<minutes_core::markdown::Frontmatter>(frontmatter)
    {
        for attendee in frontmatter.normalized_attendees() {
            let canonical = clean_vocabulary_attendee_suggestion(&attendee);
            if canonical.is_empty() || !known.insert(vocabulary_key(&canonical)) {
                continue;
            }
            suggestions.push(VocabularySuggestion {
                canonical: canonical.clone(),
                kind: "person".into(),
                aliases: vec![],
                reason: "attendee not in vocabulary".into(),
                count: 1,
            });
            for token in canonical.split_whitespace() {
                known.insert(vocabulary_key(token));
            }
        }
    }

    let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for phrase in proper_noun_phrases(body) {
        if known.contains(&vocabulary_key(&phrase)) {
            continue;
        }
        *counts.entry(phrase).or_default() += 1;
    }

    let mut repeated = counts
        .into_iter()
        .filter(|(phrase, count)| *count >= 2 && phrase.len() >= 4)
        .collect::<Vec<_>>();
    repeated.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    for (phrase, count) in repeated.into_iter().take(10) {
        suggestions.push(VocabularySuggestion {
            canonical: phrase,
            kind: "term".into(),
            aliases: vec![],
            reason: "repeated capitalized phrase in transcript".into(),
            count,
        });
    }

    suggestions.truncate(20);
    suggestions
}

fn proper_noun_phrases(body: &str) -> Vec<String> {
    let mut phrases = Vec::new();

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
        {
            continue;
        }

        for sentence in trimmed.split(['.', '!', '?', ';', ':']) {
            let mut current = Vec::new();
            for (index, token) in sentence.split_whitespace().enumerate() {
                let cleaned = token.trim_matches(|c: char| !c.is_alphanumeric());
                let looks_like_nameish = cleaned.len() >= 2
                    && cleaned.chars().next().is_some_and(|ch| ch.is_uppercase())
                    && !cleaned.chars().all(|ch| ch.is_uppercase())
                    && !is_vocabulary_noise_token(cleaned)
                    && !is_vocabulary_sentence_starter(cleaned, index);

                if looks_like_nameish {
                    current.push(cleaned.to_string());
                    continue;
                }

                if !current.is_empty() {
                    phrases.push(current.join(" "));
                    current.clear();
                }
            }
            if !current.is_empty() {
                phrases.push(current.join(" "));
            }
        }
    }
    phrases
}

fn clean_vocabulary_attendee_suggestion(value: &str) -> String {
    let without_parenthetical = value
        .split_once(" (")
        .map(|(name, _)| name)
        .unwrap_or(value)
        .trim();
    without_parenthetical
        .trim_matches(|c: char| c == '-' || c == ',' || c == ';')
        .trim()
        .to_string()
}

fn is_vocabulary_noise_token(token: &str) -> bool {
    token.starts_with("SPEAKER_")
        || matches!(
            token,
            "Actually"
                | "Absolutely"
                | "Awesome"
                | "Basically"
                | "Because"
                | "Cool"
                | "Does"
                | "Exactly"
                | "Friday"
                | "Good"
                | "Great"
                | "Have"
                | "Hello"
                | "He's"
                | "Hey"
                | "I"
                | "I'll"
                | "I'm"
                | "I've"
                | "It's"
                | "Let's"
                | "Like"
                | "Make"
                | "Monday"
                | "No"
                | "Okay"
                | "Right"
                | "Saturday"
                | "She's"
                | "Sunday"
                | "Sure"
                | "They"
                | "Thanks"
                | "Thank"
                | "That"
                | "That's"
                | "There"
                | "There's"
                | "Then"
                | "Thursday"
                | "Tuesday"
                | "This"
                | "Totally"
                | "Wednesday"
                | "Well"
                | "We're"
                | "What"
                | "Why"
                | "Your"
                | "Yeah"
                | "Yep"
                | "You're"
        )
}

fn is_vocabulary_sentence_starter(token: &str, index: usize) -> bool {
    index == 0
        && matches!(
            token,
            "A" | "An"
                | "And"
                | "But"
                | "I"
                | "It"
                | "Later"
                | "Meeting"
                | "Next"
                | "So"
                | "That"
                | "The"
                | "Then"
                | "These"
                | "This"
                | "Today"
                | "Tomorrow"
                | "Transcript"
                | "We"
                | "Yesterday"
        )
}

fn parse_vocabulary_kind(kind: &str) -> Result<minutes_core::vocabulary::VocabularyKind> {
    match kind {
        "person" => Ok(minutes_core::vocabulary::VocabularyKind::Person),
        "organization" => Ok(minutes_core::vocabulary::VocabularyKind::Organization),
        "project" => Ok(minutes_core::vocabulary::VocabularyKind::Project),
        "term" => Ok(minutes_core::vocabulary::VocabularyKind::Term),
        "acronym" => Ok(minutes_core::vocabulary::VocabularyKind::Acronym),
        other => Err(anyhow::anyhow!("unknown vocabulary kind: {}", other)),
    }
}

fn vocabulary_kind_label(kind: minutes_core::vocabulary::VocabularyKind) -> &'static str {
    match kind {
        minutes_core::vocabulary::VocabularyKind::Person => "person",
        minutes_core::vocabulary::VocabularyKind::Organization => "organization",
        minutes_core::vocabulary::VocabularyKind::Project => "project",
        minutes_core::vocabulary::VocabularyKind::Term => "term",
        minutes_core::vocabulary::VocabularyKind::Acronym => "acronym",
    }
}

fn vocabulary_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase()
}

fn cmd_commitments(person: Option<&str>, json: bool, config: &Config) -> Result<()> {
    use minutes_core::graph;

    // Auto-rebuild if index doesn't exist
    if !graph::db_path().exists() {
        eprintln!("Building relationship index...");
        graph::rebuild_index(config).map_err(|e| anyhow::anyhow!("{}", e))?;
    }

    let commitments =
        graph::query_commitments(config, person).map_err(|e| anyhow::anyhow!("{}", e))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&commitments)?);
        return Ok(());
    }

    if commitments.is_empty() {
        let scope = person.map(|p| format!(" for {}", p)).unwrap_or_default();
        eprintln!("No open commitments found{}.", scope);
        return Ok(());
    }

    // Group by status for clear output
    let stale: Vec<_> = commitments.iter().filter(|c| c.status == "stale").collect();
    let open: Vec<_> = commitments.iter().filter(|c| c.status == "open").collect();

    if !stale.is_empty() {
        eprintln!("STALE ({} overdue)", stale.len());
        for c in &stale {
            let who = c.person_name.as_deref().unwrap_or("unassigned");
            eprintln!(
                "  \x1b[33m⚠\x1b[0m {} \x1b[2m({}; due: {}; from: {})\x1b[0m",
                c.text,
                who,
                c.due_date.as_deref().unwrap_or("no date"),
                c.meeting_title,
            );
        }
    }

    if !open.is_empty() {
        if !stale.is_empty() {
            eprintln!();
        }
        eprintln!("OPEN ({})", open.len());
        for c in &open {
            let who = c.person_name.as_deref().unwrap_or("unassigned");
            eprintln!(
                "  · {} \x1b[2m({}; from: {})\x1b[0m",
                c.text, who, c.meeting_title
            );
        }
    }

    Ok(())
}

fn cmd_research(
    query: &str,
    content_type: Option<String>,
    since: Option<String>,
    attendee: Option<String>,
    include_restricted: bool,
    config: &Config,
) -> Result<()> {
    if include_restricted {
        record_sensitivity_override("cli.research", Some(query));
    }
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee,
        intent_kind: None,
        owner: None,
        recorded_by: None,
        include_restricted,
    };

    let report = minutes_core::search::cross_meeting_research(query, config, &filters)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    if report.related_decisions.is_empty()
        && report.related_open_intents.is_empty()
        && report.recent_meetings.is_empty()
    {
        eprintln!("No cross-meeting results found for {}.", query);
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    eprintln!("Cross-meeting research for {}:", query);
    if !report.related_topics.is_empty() {
        eprintln!(
            "  Related topics: {}",
            report
                .related_topics
                .iter()
                .map(|topic| format!("{} ({})", topic.topic, topic.count))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !report.related_decisions.is_empty() {
        eprintln!("  Recent decisions:");
        for decision in &report.related_decisions {
            eprintln!("    {} — {}", decision.date, decision.what);
        }
    }
    if !report.related_open_intents.is_empty() {
        eprintln!("  Open follow-ups:");
        for intent in &report.related_open_intents {
            let owner = owner_display(
                intent.who.as_deref(),
                intent.who_original.as_deref(),
                intent.who_provenance.as_deref(),
            );
            let due = intent.by_date.as_deref().unwrap_or("no due date");
            eprintln!(
                "    {:?}: {} (@{}, {})",
                intent.kind, intent.what, owner, due
            );
        }
    }
    if !report.recent_meetings.is_empty() {
        eprintln!("  Matching meetings:");
        for meeting in &report.recent_meetings {
            eprintln!("    {} — {}", meeting.date, meeting.title);
        }
    }

    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn parse_intent_kind(kind: &str) -> Result<minutes_core::markdown::IntentKind> {
    match kind {
        "action-item" => Ok(minutes_core::markdown::IntentKind::ActionItem),
        "decision" => Ok(minutes_core::markdown::IntentKind::Decision),
        "open-question" => Ok(minutes_core::markdown::IntentKind::OpenQuestion),
        "commitment" => Ok(minutes_core::markdown::IntentKind::Commitment),
        other => anyhow::bail!(
            "unknown intent kind: {}. Use action-item, decision, open-question, or commitment.",
            other
        ),
    }
}

/// True when the meeting file at `path` carries `sensitivity: restricted`
/// frontmatter (consent layer Wave 2). Unreadable files are treated as not
/// restricted; downstream ingest surfaces its own read errors.
fn meeting_is_restricted(path: &Path) -> bool {
    let Ok(content) = std::fs::read_to_string(path) else {
        return false;
    };
    let (fm_str, _) = minutes_core::markdown::split_frontmatter(&content);
    minutes_core::markdown::extract_field(fm_str, "sensitivity").as_deref() == Some("restricted")
}

fn cmd_ingest(path: Option<PathBuf>, all: bool, dry_run: bool, config: &Config) -> Result<()> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        eprintln!("Knowledge base is not configured.");
        eprintln!("Add this to ~/.config/minutes/config.toml:\n");
        eprintln!("[knowledge]");
        eprintln!("enabled = true");
        eprintln!("path = \"/path/to/your/knowledge/base\"");
        eprintln!("adapter = \"wiki\"  # or \"para\", \"obsidian\"");
        return Ok(());
    }

    let files: Vec<PathBuf> = if all {
        let mut found = Vec::new();
        for entry_result in walkdir::WalkDir::new(&config.output_dir)
            .max_depth(2)
            .into_iter()
        {
            let entry = match entry_result {
                Ok(e) => e,
                Err(e) => {
                    eprintln!("  WARN: {}", e);
                    continue;
                }
            };
            let p = entry.path();
            if p.extension().is_some_and(|ext| ext == "md")
                && !p.starts_with(config.output_dir.join("memos"))
            {
                found.push(p.to_path_buf());
            }
        }
        found.sort();
        // Sensitivity enforcement (consent layer Wave 2): restricted meetings
        // never enter the knowledge base via batch ingest. There is no
        // override for knowledge ingest in this wave.
        let before = found.len();
        found.retain(|p| !meeting_is_restricted(p));
        let skipped_restricted = before - found.len();
        if skipped_restricted > 0 {
            eprintln!(
                "  skipping {} meeting(s) designated `sensitivity: restricted` (excluded from knowledge ingest)",
                skipped_restricted
            );
        }
        found
    } else if let Some(ref p) = path {
        if !p.exists() {
            anyhow::bail!("File not found: {}", p.display());
        }
        if meeting_is_restricted(p) {
            anyhow::bail!(
                "{} is designated `sensitivity: restricted`; it is excluded from knowledge ingest by default and was not written to the knowledge base",
                p.display()
            );
        }
        vec![p.clone()]
    } else {
        eprintln!("Usage: minutes ingest <path> or minutes ingest --all");
        return Ok(());
    };

    eprintln!(
        "Ingesting {} meeting(s) into knowledge base at {}",
        files.len(),
        config.knowledge.path.display()
    );
    if dry_run {
        eprintln!("(dry run — no files will be written)\n");
    }

    let mut total_written = 0usize;
    let mut total_skipped = 0usize;
    let mut total_people = std::collections::HashSet::new();
    let mut errors = 0usize;

    for file in &files {
        let filename = file.file_name().unwrap_or_default().to_string_lossy();

        if dry_run {
            // Read and extract but don't write
            let content = match std::fs::read_to_string(file) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("  SKIP {}: {}", filename, e);
                    errors += 1;
                    continue;
                }
            };
            let (fm_str, _body) = minutes_core::markdown::split_frontmatter(&content);
            if fm_str.is_empty() {
                eprintln!("  SKIP {}: no frontmatter", filename);
                continue;
            }
            let fm: minutes_core::markdown::Frontmatter = match serde_yaml::from_str(fm_str) {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("  SKIP {}: {}", filename, e);
                    errors += 1;
                    continue;
                }
            };
            let facts = minutes_core::knowledge_extract::extract_from_frontmatter(
                &fm,
                &file.display().to_string(),
            );
            let fact_count: usize = facts.iter().map(|pf| pf.facts.len()).sum();
            let people: Vec<&str> = facts.iter().map(|pf| pf.name.as_str()).collect();
            if fact_count > 0 {
                eprintln!(
                    "  {} — {} fact(s) for: {}",
                    filename,
                    fact_count,
                    people.join(", ")
                );
            }
            total_written += fact_count;
        } else {
            match minutes_core::knowledge::ingest_file(file, config) {
                Ok(result) => {
                    if result.facts_written > 0 {
                        eprintln!(
                            "  {} — {} written, {} skipped — {}",
                            filename,
                            result.facts_written,
                            result.facts_skipped,
                            result.people_updated.join(", ")
                        );
                    }
                    total_written += result.facts_written;
                    total_skipped += result.facts_skipped;
                    for p in result.people_updated {
                        total_people.insert(p);
                    }
                }
                Err(e) => {
                    eprintln!("  SKIP {}: {}", filename, e);
                    errors += 1;
                }
            }
        }
    }

    eprintln!(
        "\nDone. {} fact(s) written, {} skipped, {} error(s), {} people updated.",
        total_written,
        total_skipped,
        errors,
        total_people.len()
    );

    Ok(())
}

/// Resolve a single meeting `.md` file from a path or search term.
///
/// Both branches canonicalize the result and require it to live under the
/// meetings directory, so neither a crafted path nor a symlinked search hit can
/// point a mutating command at a file outside the user's meetings.
fn resolve_single_meeting(meeting: &str, config: &Config) -> Result<std::path::PathBuf> {
    let meetings_dir = &config.output_dir;
    let meetings_canonical = meetings_dir
        .canonicalize()
        .unwrap_or_else(|_| meetings_dir.clone());
    let ensure_contained = |canonical: std::path::PathBuf| -> Result<std::path::PathBuf> {
        if !canonical.starts_with(&meetings_canonical) {
            anyhow::bail!(
                "path {} is outside the meetings directory ({})",
                canonical.display(),
                meetings_dir.display()
            );
        }
        Ok(canonical)
    };

    let path = std::path::PathBuf::from(meeting);
    if path.exists() {
        return ensure_contained(path.canonicalize()?);
    }

    // Restrict the search to meetings so a memo/dictation never resolves here.
    let filters = minutes_core::search::SearchFilters {
        content_type: Some("meeting".into()),
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
        include_restricted: true,
    };
    let results = minutes_core::search::search(meeting, config, &filters)?;
    if results.is_empty() {
        anyhow::bail!("no meeting found matching: {}", meeting);
    }
    let canonical = ensure_contained(results[0].path.canonicalize()?)?;
    eprintln!("  Matched: {}", canonical.display());
    Ok(canonical)
}

/// Extract the body of the `## Transcript` section. Scans line-by-line so a
/// fenced code block or a heading like `## Transcript cleanup notes` cannot be
/// mistaken for the real transcript: only an H2 whose text is exactly
/// `Transcript` (outside any code fence) opens the section, and the next real H2
/// closes it. Returns `None` when there is no such section.
fn transcript_section(body: &str) -> Option<String> {
    // Track which marker opened the current fence so a `~~~` line inside a
    // ``` block (or vice versa) is treated as content, not as a fence toggle.
    let mut fence: Option<char> = None;
    let mut found = false;
    let mut collected: Vec<&str> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some('`')
        } else if trimmed.starts_with("~~~") {
            Some('~')
        } else {
            None
        };
        if let Some(m) = marker {
            match fence {
                None => fence = Some(m),                 // open a fence
                Some(open) if open == m => fence = None, // close only on a matching marker
                Some(_) => {}                            // other marker inside a fence: content
            }
            if found {
                collected.push(line);
            }
            continue;
        }
        if fence.is_none() {
            if let Some(rest) = line.strip_prefix("## ") {
                if found {
                    break; // next section ends the transcript
                }
                if rest.trim() == "Transcript" {
                    found = true;
                    continue;
                }
            }
        }
        if found {
            collected.push(line);
        }
    }
    if !found {
        return None;
    }
    Some(collected.join("\n").trim_start_matches('\n').to_string())
}

/// Merge a freshly-computed speaker map into the existing one without ever
/// downgrading an existing High-confidence attribution. Fresh entries (Medium at
/// most, from the LLM) fill in or refresh anything not already High-confidence.
fn merge_speaker_map(
    existing: &[minutes_core::diarize::SpeakerAttribution],
    fresh: &[minutes_core::diarize::SpeakerAttribution],
) -> Vec<minutes_core::diarize::SpeakerAttribution> {
    use minutes_core::diarize::Confidence;
    use std::collections::BTreeMap;

    fn rank(c: Confidence) -> u8 {
        match c {
            Confidence::High => 2,
            Confidence::Medium => 1,
            Confidence::Low => 0,
        }
    }

    let mut by_label: BTreeMap<String, minutes_core::diarize::SpeakerAttribution> = BTreeMap::new();
    // Fold existing entries, keeping the highest-confidence one per label. This
    // guards against a malformed frontmatter that lists the same label twice
    // (e.g. High then Medium) silently dropping the High before merge.
    for a in existing {
        match by_label.get(&a.speaker_label) {
            Some(cur) if rank(cur.confidence) >= rank(a.confidence) => {}
            _ => {
                by_label.insert(a.speaker_label.clone(), a.clone());
            }
        }
    }
    // Apply fresh results only where the retained existing entry is not High.
    for a in fresh {
        match by_label.get(&a.speaker_label) {
            Some(cur) if cur.confidence == Confidence::High => {} // never downgrade
            _ => {
                by_label.insert(a.speaker_label.clone(), a.clone());
            }
        }
    }
    by_label.into_values().collect()
}

/// Re-run Level-1 speaker mapping on an existing meeting and (with `--apply`)
/// write the merged map plus a `speaker_mapping` health block. Recovery path for
/// meetings whose mapping failed, timed out, or never ran.
fn cmd_redo_speaker_mapping(
    meeting: &str,
    apply: bool,
    engine: Option<String>,
    json: bool,
    config: &Config,
) -> Result<()> {
    let path = resolve_single_meeting(meeting, config)?;
    let content = std::fs::read_to_string(&path)?;
    let (fm_str, body) = minutes_core::markdown::split_frontmatter(&content);
    if fm_str.is_empty() {
        anyhow::bail!(
            "{} is not a meeting file (no YAML frontmatter)",
            path.display()
        );
    }
    let fm: minutes_core::markdown::Frontmatter = serde_yaml::from_str(fm_str)
        .map_err(|e| anyhow::anyhow!("could not parse frontmatter in {}: {e}", path.display()))?;

    // Only meetings carry diarized speaker labels. Refuse memos/dictation so we
    // never write a speaker map (or call the LLM) on a non-meeting file.
    if fm.r#type != minutes_core::markdown::ContentType::Meeting {
        anyhow::bail!(
            "{} is type {:?}, not a meeting; redo-speaker-mapping only operates on meetings",
            path.display(),
            fm.r#type
        );
    }

    // Source-of-truth contract: we map the diarized transcript against the
    // recorded attendee pool. If either is missing there is nothing to do, and
    // we say exactly which side is missing rather than silently producing junk.
    let transcript = transcript_section(body);
    let labels: Vec<String> = transcript
        .as_deref()
        .map(minutes_core::summarize::extract_speaker_labels_pub)
        .unwrap_or_default();
    let attendees = fm.normalized_attendees();

    // Engine override (validated lazily by the summarizer).
    let mut cfg = config.clone();
    if let Some(e) = &engine {
        cfg.summarization.engine = e.clone();
    }
    let model = minutes_core::summarize::speaker_mapping_model_hint(&cfg);

    // Contract failures short-circuit to a `skipped` health block.
    let skip_reason: Option<&str> = if transcript.is_none() {
        Some("no ## Transcript section in this meeting")
    } else if labels.is_empty() {
        Some("no anonymous SPEAKER_n labels remain to remap")
    } else if attendees.is_empty() {
        Some("no attendees recorded to map against; add attendees: to the frontmatter and retry")
    } else {
        None
    };

    if let Some(reason) = skip_reason {
        let health = minutes_core::markdown::SpeakerMappingHealth {
            status: "skipped".into(),
            model,
            diarized_speakers: labels.len(),
            mapped_speakers: 0,
            attendees: attendees.len(),
            duration_ms: None,
            reason: Some(reason.to_string()),
            last_run: Some(Local::now().to_rfc3339()),
        };
        report_redo_result(
            &path,
            apply,
            json,
            &fm.speaker_map,
            &fm.speaker_map,
            &health,
            config,
        )?;
        return Ok(());
    }

    let transcript = transcript.expect("transcript present past contract check");
    let started = std::time::Instant::now();
    let fresh = minutes_core::summarize::map_speakers(&transcript, &attendees, &cfg, None);
    let duration_ms = started.elapsed().as_millis() as u64;
    let merged = merge_speaker_map(&fm.speaker_map, &fresh);

    let health = minutes_core::markdown::SpeakerMappingHealth {
        status: if fresh.is_empty() {
            "empty".into()
        } else {
            "ok".into()
        },
        model,
        diarized_speakers: labels.len(),
        mapped_speakers: fresh.len(),
        attendees: attendees.len(),
        duration_ms: Some(duration_ms),
        reason: if fresh.is_empty() {
            Some("mapper ran but produced no confident matches".into())
        } else {
            None
        },
        last_run: Some(Local::now().to_rfc3339()),
    };

    report_redo_result(
        &path,
        apply,
        json,
        &fm.speaker_map,
        &merged,
        &health,
        config,
    )?;
    Ok(())
}

/// Apply (when `--apply`) then render the outcome of a redo run.
///
/// The write happens BEFORE any output so a failed write returns an error
/// instead of first printing a success line/JSON the caller would trust. On
/// `--apply` the `speaker_mapping` health block is always recorded (so a re-run
/// is visible in the frontmatter) even when the map itself did not change.
fn report_redo_result(
    path: &std::path::Path,
    apply: bool,
    json: bool,
    before: &[minutes_core::diarize::SpeakerAttribution],
    after: &[minutes_core::diarize::SpeakerAttribution],
    health: &minutes_core::markdown::SpeakerMappingHealth,
    _config: &Config,
) -> Result<()> {
    let map_changed = before != after;
    let mut frontmatter_written = false;

    // Write first: if this fails we propagate the error and emit no success output.
    if apply {
        let health_owned = health.clone();
        if map_changed {
            let after_owned = after.to_vec();
            minutes_core::markdown::update_frontmatter(path, move |f| {
                f.speaker_map = after_owned;
                f.speaker_mapping = Some(health_owned);
            })?;
        } else {
            minutes_core::markdown::update_frontmatter(path, move |f| {
                f.speaker_mapping = Some(health_owned);
            })?;
        }
        frontmatter_written = true;
    }

    if json {
        let payload = serde_json::json!({
            "path": path.display().to_string(),
            "map_changed": map_changed,
            "written": frontmatter_written,
            "health": health,
            "speaker_map": after,
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&json_envelope("redo-speaker-mapping", payload))?
        );
        return Ok(());
    }

    println!("Meeting: {}", path.display());
    println!(
        "  status: {} ({} of {} labels named, {} attendees)",
        health.status, health.mapped_speakers, health.diarized_speakers, health.attendees
    );
    if let Some(reason) = &health.reason {
        println!("  reason: {reason}");
    }
    if after.is_empty() {
        println!("  map: (none)");
    } else {
        for a in after {
            println!(
                "  map: {} -> {} [{:?}, {:?}]",
                a.speaker_label, a.name, a.confidence, a.source
            );
        }
    }
    if frontmatter_written {
        if map_changed {
            println!("  written: speaker_map + speaker_mapping health updated");
        } else {
            println!("  written: speaker_mapping health updated (map unchanged)");
        }
    } else {
        println!("  (dry run: re-run with --apply to write)");
    }

    Ok(())
}

fn cmd_clean(meeting: &str, apply: bool, config: &Config) -> Result<()> {
    let meetings_dir = &config.output_dir;

    // Resolve which files to clean
    let files: Vec<std::path::PathBuf> = if meeting == "all" {
        // Find all .md files in meetings dir
        let mut found = Vec::new();
        if meetings_dir.exists() {
            for entry in std::fs::read_dir(meetings_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    found.push(path);
                }
            }
        }
        // Also check memos subdir
        let memos_dir = meetings_dir.join("memos");
        if memos_dir.exists() {
            for entry in std::fs::read_dir(&memos_dir)? {
                let entry = entry?;
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "md") {
                    found.push(path);
                }
            }
        }
        found.sort();
        found
    } else {
        let path = std::path::PathBuf::from(meeting);
        if path.exists() {
            // Containment check: only allow files under the meetings directory
            let canonical = path.canonicalize()?;
            let meetings_canonical = meetings_dir
                .canonicalize()
                .unwrap_or_else(|_| meetings_dir.clone());
            if !canonical.starts_with(&meetings_canonical) {
                anyhow::bail!(
                    "path {} is outside the meetings directory ({})",
                    path.display(),
                    meetings_dir.display()
                );
            }
            vec![canonical]
        } else {
            // Try as a search term
            let filters = minutes_core::search::SearchFilters {
                content_type: None,
                since: None,
                attendee: None,
                intent_kind: None,
                owner: None,
                recorded_by: None,
                include_restricted: true,
            };
            let results = minutes_core::search::search(meeting, config, &filters)?;
            if results.is_empty() {
                anyhow::bail!("no meeting found matching: {}", meeting);
            }
            eprintln!("  Matched: {}", results[0].path.display());
            vec![results[0].path.clone()]
        }
    };

    if files.is_empty() {
        eprintln!("No meeting files found.");
        return Ok(());
    }

    let mut total_cleaned = 0;
    let mut total_lines_removed = 0;

    for path in &files {
        let content = std::fs::read_to_string(path)?;

        // Split into frontmatter + body, find the transcript section
        let (fm, body) = minutes_core::markdown::split_frontmatter(&content);

        // Find the "## Transcript" section — must be at start of line to avoid
        // matching "## Transcript" appearing in body text or notes
        let transcript_marker = "\n## Transcript";
        if let Some(transcript_start) = body.find(transcript_marker) {
            let heading_start = transcript_start + 1; // skip the \n
            let transcript_offset = heading_start + "## Transcript".len();
            let before_transcript = &body[..heading_start];

            // Get everything after "## Transcript\n"
            let transcript_text = body[transcript_offset..].trim_start_matches('\n');

            // Check if there's another section after transcript
            let (transcript_part, after_transcript) =
                if let Some(next_section) = transcript_text.find("\n## ") {
                    (
                        &transcript_text[..next_section],
                        Some(&transcript_text[next_section..]),
                    )
                } else {
                    (transcript_text, None)
                };

            // Clean the transcript
            let (cleaned, stats) = minutes_core::transcribe::clean_transcript(transcript_part);

            if stats.lines_removed > 0 {
                let filename = path.file_name().unwrap_or_default().to_string_lossy();
                if apply {
                    // Rebuild the file
                    // Reconstruct the file preserving original formatting.
                    // split_frontmatter returns fm with a leading \n — strip it
                    // to avoid inserting a blank line after the opening ---.
                    let mut new_content = String::new();
                    if !fm.is_empty() {
                        new_content.push_str("---\n");
                        new_content.push_str(fm.trim_start_matches('\n'));
                        if !fm.ends_with('\n') {
                            new_content.push('\n');
                        }
                        new_content.push_str("---\n");
                    }
                    // body also starts with \n after the closing --- line
                    new_content.push_str(before_transcript.trim_start_matches('\n'));
                    if !new_content.is_empty() && !new_content.ends_with('\n') {
                        new_content.push('\n');
                    }
                    new_content.push_str("\n## Transcript\n\n");
                    new_content.push_str(&cleaned);
                    if let Some(after) = after_transcript {
                        new_content.push_str(after);
                    }
                    new_content.push('\n');

                    // Backup original before overwriting
                    let backup = path.with_extension("md.bak");
                    std::fs::copy(path, &backup)?;

                    // Atomic write: temp file + rename to avoid corruption
                    // on interrupted writes
                    let tmp_path = path.with_extension("md.tmp");
                    std::fs::write(&tmp_path, &new_content)?;
                    std::fs::rename(&tmp_path, path)?;
                    eprintln!(
                        "  Cleaned {} — removed {} lines ({} → {})",
                        filename,
                        stats.lines_removed,
                        stats.original_lines,
                        stats.after_trailing_trim
                    );
                } else {
                    eprintln!(
                        "  {} — would remove {} lines ({} → {})",
                        filename,
                        stats.lines_removed,
                        stats.original_lines,
                        stats.after_trailing_trim
                    );
                }
                total_cleaned += 1;
                total_lines_removed += stats.lines_removed;
            }
        }
    }

    eprintln!();
    if total_cleaned == 0 {
        eprintln!(
            "All {} meetings are clean — no hallucination loops detected.",
            files.len()
        );
    } else if apply {
        eprintln!(
            "Cleaned {} meeting(s), removed {} total lines of hallucinated repetition.",
            total_cleaned, total_lines_removed
        );
    } else {
        eprintln!(
            "Found {} meeting(s) with hallucinated repetition ({} lines to remove).",
            total_cleaned, total_lines_removed
        );
        eprintln!("Run with --apply to fix them.");
    }

    Ok(())
}

/// Transcribe an audio file to text and optionally JSON-encode the result.
/// No meeting files are written, no summarization is performed.
fn cmd_transcribe(path: &Path, output_json: bool, do_diarize: bool, config: &Config) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }

    // Transcribe with whisper-guard cleanup applied.
    // result.segments carries real centisecond-precision start/end from the engine.
    // result.detected_language carries the language the ASR engine identified.
    let result = minutes_core::transcribe::transcribe(path, config)
        .map_err(|e| anyhow::anyhow!("transcription failed: {}", e))?;

    if !output_json {
        // Plain-text mode: emit the cleaned transcript unchanged
        print!("{}", result.text.trim_end());
        println!();
        return Ok(());
    }

    let duration_ms = (result.stats.audio_duration_secs * 1000.0) as u64;

    // Language: CLI override wins; fall back to what the engine detected.
    let language = config
        .transcription
        .language
        .as_deref()
        .map(str::to_owned)
        .or_else(|| result.detected_language.clone())
        .unwrap_or_else(|| "unknown".to_string());

    // Build output segments from the core result (real timestamps, no scraping).
    let segments: Vec<TranscribeSegmentOutput> = if do_diarize {
        let diarize_result = minutes_core::diarize::diarize(path, config);
        if let Some(dr) = diarize_result {
            result
                .segments
                .iter()
                .map(|seg| {
                    // Attribute this segment to the diarization speaker whose
                    // window best overlaps [seg.start, seg.end].
                    let speaker = dr
                        .segments
                        .iter()
                        .find(|s| s.start < seg.end && s.end > seg.start)
                        .map(|s| s.speaker.clone());
                    TranscribeSegmentOutput {
                        start: seg.start,
                        end: seg.end,
                        text: seg.text.clone(),
                        speaker,
                    }
                })
                .collect()
        } else {
            result
                .segments
                .iter()
                .map(|seg| TranscribeSegmentOutput {
                    start: seg.start,
                    end: seg.end,
                    text: seg.text.clone(),
                    speaker: None,
                })
                .collect()
        }
    } else {
        result
            .segments
            .iter()
            .map(|seg| TranscribeSegmentOutput {
                start: seg.start,
                end: seg.end,
                text: seg.text.clone(),
                speaker: None,
            })
            .collect()
    };

    // data.text: segment texts joined with newlines (clean, no [m:ss] prefixes).
    // Falls back to result.text when segments are empty (e.g. parakeet engine).
    let clean_text = if segments.is_empty() {
        // Strip [m:ss] prefixes from the existing timestamped text as a fallback
        result
            .text
            .lines()
            .map(|line| {
                if let Some(rest) = line.strip_prefix('[') {
                    if let Some(end) = rest.find(']') {
                        return rest[end + 1..].trim().to_string();
                    }
                }
                line.trim().to_string()
            })
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    };

    let data = TranscribeOutput {
        text: clean_text,
        language,
        segments,
        duration_ms,
    };
    let envelope = json_envelope("transcribe", data);
    println!("{}", serde_json::to_string_pretty(&envelope)?);

    Ok(())
}

fn cmd_process(
    path: &Path,
    content_type: &str,
    title: Option<&str>,
    template: Option<&minutes_core::Template>,
    config: &Config,
) -> Result<()> {
    let ct = match content_type {
        "meeting" => ContentType::Meeting,
        "memo" => ContentType::Memo,
        other => anyhow::bail!("unknown content type: {}. Use 'meeting' or 'memo'.", other),
    };

    let _trace_guard = if minutes_core::process_trace::is_active() {
        None
    } else {
        minutes_core::process_trace::start_process_trace(path, ct, config)
    };
    let result = (|| -> Result<()> {
        if !path.exists() {
            anyhow::bail!("file not found: {}", path.display());
        }

        config.ensure_dirs()?;
        let result = minutes_core::pipeline::process_with_template(
            path,
            ct,
            title,
            config,
            None,
            template,
            |_| {},
        )?;
        eprintln!("Saved: {}", result.path.display());

        // Update relationship graph index
        if let Err(e) = minutes_core::graph::rebuild_index(config) {
            tracing::warn!(error = %e, "graph index rebuild failed (non-fatal)");
        }

        let json = serde_json::to_string_pretty(&serde_json::json!({
            "status": "done",
            "file": result.path.display().to_string(),
            "title": result.title,
            "words": result.word_count,
        }))?;
        println!("{}", json);
        Ok(())
    })();
    match &result {
        Ok(_) => minutes_core::process_trace::stage("process.done"),
        Err(error) => minutes_core::process_trace::stage_with_extra(
            "process.error",
            serde_json::json!({"error": error.to_string()}),
        ),
    }
    result
}

fn cmd_template(cmd: TemplateCmd) -> Result<()> {
    let resolver = minutes_core::TemplateResolver::new();
    match cmd {
        TemplateCmd::List => {
            let listings = resolver.list();
            if listings.is_empty() {
                eprintln!("No templates installed.");
                return Ok(());
            }
            let slug_width = listings
                .iter()
                .map(|l| l.slug.len())
                .max()
                .unwrap_or(8)
                .max(8);
            let source_width = 8; // "bundled" / "project" / "user"
            println!(
                "{slug:slug_w$}  {src:src_w$}  DESCRIPTION",
                slug = "SLUG",
                src = "SOURCE",
                slug_w = slug_width,
                src_w = source_width,
            );
            for listing in listings {
                println!(
                    "{:slug_w$}  {:src_w$}  {}",
                    listing.slug,
                    listing.source.as_str(),
                    listing.description,
                    slug_w = slug_width,
                    src_w = source_width,
                );
            }
            Ok(())
        }
        TemplateCmd::Show { slug } => {
            let template = resolver
                .resolve(&slug)
                .map_err(|e| anyhow::anyhow!("{}", e))?;
            match template.path.as_ref() {
                Some(path) => {
                    let body = std::fs::read_to_string(path)
                        .map_err(|e| anyhow::anyhow!("could not read {}: {}", path.display(), e))?;
                    print!("{}", body);
                }
                None => {
                    let yaml = serde_yaml::to_string(&template.frontmatter)
                        .map_err(|e| anyhow::anyhow!("could not render template: {}", e))?;
                    println!("---\n{}---\n", yaml);
                    print!("{}", template.body);
                }
            }
            Ok(())
        }
        TemplateCmd::Validate { path } => {
            if !path.exists() {
                anyhow::bail!("file not found: {}", path.display());
            }
            let template =
                minutes_core::Template::load_file(&path, minutes_core::TemplateSource::Project)
                    .map_err(|e| anyhow::anyhow!("{}", e))?;
            println!(
                "OK: template '{}' ({} v{})",
                template.frontmatter.slug, template.frontmatter.name, template.frontmatter.version
            );
            Ok(())
        }
    }
}

/// Process an existing WAV file as a mock recording with full diagnostic output.
/// Bypasses live mic capture — runs diarization, voice matching, and the full
/// pipeline on the provided file so results can be reproduced deterministically.
fn cmd_diagnose(path: &Path, title: Option<&str>, config: &Config) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }
    config.ensure_dirs()?;

    eprintln!("=== Diagnose: {} ===", path.display());
    eprintln!();

    // Step 1: Diarization
    eprintln!("--- Diarization ---");
    let diarize_outcome = minutes_core::diarize::diarize_with_context(
        path,
        config,
        minutes_core::diarize::DiarizationContext {
            purpose: minutes_core::diarize::DiarizationPurpose::Auxiliary,
            transcript_windows: None,
        },
    );
    let diarization_embeddings = match &diarize_outcome {
        minutes_core::diarize::DiarizationOutcome::Result(result) => {
            eprintln!("  Speakers: {}", result.num_speakers);
            for seg in &result.segments {
                eprintln!("  [{} {:.1}s–{:.1}s]", seg.speaker, seg.start, seg.end);
            }
            for (label, emb) in &result.speaker_embeddings {
                let rms = (emb.iter().map(|v| v * v).sum::<f32>() / emb.len() as f32).sqrt();
                eprintln!("    {}: {} dims, rms={:.2}", label, emb.len(), rms);
            }
            result.speaker_embeddings.clone()
        }
        minutes_core::diarize::DiarizationOutcome::Skipped { reason } => {
            eprintln!(
                "  Diarization skipped because capture health was degraded: {:?}",
                reason.failure_kind
            );
            eprintln!(
                "  Source: {:?}, confidence: {:?}",
                reason.capture_source, reason.diagnostic_confidence
            );
            std::collections::HashMap::new()
        }
        minutes_core::diarize::DiarizationOutcome::NotConfigured => {
            eprintln!("  No diarization result (disabled or failed).");
            std::collections::HashMap::new()
        }
    };

    // Step 2: Voice matching
    eprintln!();
    eprintln!("--- Voice Matching ---");
    if config.voice.enabled && !diarization_embeddings.is_empty() {
        let profiles = minutes_core::voice::open_db()
            .ok()
            .and_then(|conn| minutes_core::voice::load_all_with_embeddings(&conn).ok())
            .unwrap_or_default();

        if profiles.is_empty() {
            eprintln!("  No enrolled voice profiles. Run `minutes enroll` first.");
        } else {
            eprintln!("  Enrolled profiles: {}", profiles.len());
            for p in &profiles {
                eprintln!("    {} ({})", p.name, p.person_slug);
            }

            let threshold = config.voice.match_threshold;
            eprintln!("  Threshold: {:.2}", threshold);
            eprintln!();

            for (label, emb) in &diarization_embeddings {
                eprintln!("  {} vs enrolled profiles:", label);
                for p in &profiles {
                    let sim = minutes_core::voice::cosine_similarity(emb, &p.embedding);
                    let marker = if sim > threshold { " ✓ MATCH" } else { "" };
                    eprintln!("    → {} : sim={:.4}{}", p.name, sim, marker);
                }
            }
        }
    } else if !config.voice.enabled {
        eprintln!("  Voice matching disabled.");
    } else {
        eprintln!("  No speaker embeddings to match against.");
    }

    // Step 3: Full pipeline
    eprintln!();
    eprintln!("--- Pipeline ---");
    let result = minutes_core::process(path, ContentType::Meeting, title, config)?;
    eprintln!("  Output: {}", result.path.display());
    eprintln!("  Title:  {}", result.title);
    eprintln!("  Words:  {}", result.word_count);
    eprintln!();

    let content = std::fs::read_to_string(&result.path)?;
    println!("{}", content);

    Ok(())
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
fn cmd_parakeet_helper(
    binary: &str,
    model_path: &Path,
    audio_path: &Path,
    vocab_path: &Path,
    model_id: &str,
    gpu: bool,
    fp16: bool,
    vad_path: Option<&Path>,
    vad_threshold: f32,
    config: &Config,
) -> Result<()> {
    let resolved_binary = minutes_core::parakeet::resolve_parakeet_binary(
        binary,
        minutes_core::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    )
    .map_err(anyhow::Error::msg)?;
    // `transcribe::transcribe_with_parakeet` (the only programmatic caller of
    // this hidden subcommand) only appends `--fp16` when it has decided
    // fp16=true for this invocation. So the flag is monotonically additive:
    // present means "force fp16 on for this run"; absent means "inherit
    // whatever the user's TOML says." Only override the cloned config in
    // the present-and-true case so manual `minutes parakeet-helper`
    // invocations keep honoring `transcription.parakeet_fp16` from disk.
    let config = if fp16 {
        let mut overridden = config.clone();
        overridden.transcription.parakeet_fp16 = true;
        std::borrow::Cow::Owned(overridden)
    } else {
        std::borrow::Cow::Borrowed(config)
    };
    let parsed = minutes_core::transcribe::run_parakeet_cli_structured(
        resolved_binary
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("resolved parakeet binary path is not valid UTF-8"))?,
        model_path,
        audio_path,
        vocab_path,
        model_id,
        gpu,
        vad_path,
        vad_threshold,
        &config,
        &minutes_core::transcribe::DecodeHints::default(),
    )?;
    let envelope = parakeet_helper_envelope("minutes parakeet-helper", parsed);
    println!("{}", serde_json::to_string(&envelope)?);
    Ok(())
}

#[cfg(not(feature = "parakeet"))]
#[allow(clippy::too_many_arguments)]
fn cmd_parakeet_helper(
    _binary: &str,
    _model_path: &Path,
    _audio_path: &Path,
    _vocab_path: &Path,
    _model_id: &str,
    _gpu: bool,
    _fp16: bool,
    _vad_path: Option<&Path>,
    _vad_threshold: f32,
    _config: &Config,
) -> Result<()> {
    anyhow::bail!(
        "Parakeet helper is not compiled in. Rebuild with `cargo build --features parakeet`."
    );
}

#[cfg(feature = "parakeet")]
#[allow(clippy::too_many_arguments)]
fn cmd_parakeet_benchmark(
    binary: &str,
    model_path: &Path,
    audio_path: &Path,
    vocab_path: &Path,
    model_id: &str,
    gpu: bool,
    vad_path: Option<&Path>,
    vad_threshold: f32,
    config: &Config,
) -> Result<()> {
    let helper_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("minutes"));
    let report = minutes_core::transcription_coordinator::benchmark_parakeet(
        &helper_bin,
        binary,
        model_path,
        audio_path,
        vocab_path,
        model_id,
        gpu,
        vad_path,
        vad_threshold,
        config,
    )
    .map_err(anyhow::Error::msg)?;
    let envelope = json_envelope("minutes parakeet-benchmark", report);
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    Ok(())
}

#[cfg(not(feature = "parakeet"))]
#[allow(clippy::too_many_arguments)]
fn cmd_parakeet_benchmark(
    _binary: &str,
    _model_path: &Path,
    _audio_path: &Path,
    _vocab_path: &Path,
    _model_id: &str,
    _gpu: bool,
    _vad_path: Option<&Path>,
    _vad_threshold: f32,
    _config: &Config,
) -> Result<()> {
    anyhow::bail!(
        "Parakeet benchmark is not compiled in. Rebuild with `cargo build --features parakeet`."
    );
}

fn cmd_autoresearch_decode_hints(
    corpus: &Path,
    output_root: Option<&Path>,
    engine: Option<&str>,
    json: bool,
) -> Result<()> {
    let options = DecodeHintEvalOptions {
        engine_override: engine.map(|value| value.to_string()),
    };
    let report = autoresearch::run_decode_hint_eval_corpus(corpus, &options)?;

    let request = DecodeHintEvalRequest {
        command: "minutes autoresearch decode-hints".into(),
        generated_at: Local::now().to_rfc3339(),
        corpus_path: corpus.to_path_buf(),
        output_root: output_root
            .map(Path::to_path_buf)
            .unwrap_or_else(autoresearch::default_research_root),
        git_commit: current_git_commit(),
        options,
    };
    let artifacts = autoresearch::write_decode_hint_eval_artifacts(&request, &report)?;
    let failed = !report.failure_messages.is_empty();

    if json {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AutoresearchDecodeHintsOutput {
            report: minutes_core::autoresearch::DecodeHintEvalReport,
            artifacts: DecodeHintEvalArtifactPaths,
        }

        let envelope = json_envelope(
            "minutes autoresearch decode-hints",
            AutoresearchDecodeHintsOutput {
                report,
                artifacts: artifacts.clone(),
            },
        );
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        println!(
            "{}",
            render_decode_hints_plaintext_summary(&report, &artifacts.run_dir, failed)
        );
    }

    if failed {
        anyhow::bail!(
            "decode hint eval failed; see {}",
            artifacts.summary_md.display()
        );
    }

    Ok(())
}

fn render_decode_hints_plaintext_summary(
    report: &minutes_core::autoresearch::DecodeHintEvalReport,
    artifact_dir: &Path,
    failed: bool,
) -> String {
    let verdict = if failed {
        "FAIL"
    } else if report.totals.cases_allowed_failures > 0 {
        "PASS WITH ALLOWED FAILURES"
    } else {
        "PASS"
    };

    let mut lines = vec![
        format!("Decode hint eval: {verdict}"),
        format!("Cases: {}", report.totals.cases_total),
        format!("Passed: {}", report.totals.cases_passed),
        format!("Failed: {}", report.totals.cases_failed),
        format!("Allowed failures: {}", report.totals.cases_allowed_failures),
        format!("Artifacts: {}", artifact_dir.display()),
    ];

    if failed {
        lines.push(String::new());
        lines.push("Failure messages:".into());
        for failure in &report.failure_messages {
            lines.push(format!("- {failure}"));
        }
    } else if report.totals.cases_allowed_failures > 0 {
        lines.push(String::new());
        lines.push("Allowed-failure cases:".into());
        for case in report
            .cases
            .iter()
            .filter(|case| !case.allowed_failure_reasons.is_empty())
        {
            lines.push(format!(
                "- {}: {}",
                case.id,
                case.allowed_failure_reasons.join("; ")
            ));
        }
    }

    lines.join("\n")
}

fn cmd_autoresearch_compare_decode_hints(
    left: &Path,
    right: &Path,
    output_root: Option<&Path>,
    json: bool,
) -> Result<()> {
    let report = autoresearch::compare_decode_hint_eval_reports(left, right)?;
    let request = DecodeHintEvalComparisonRequest {
        command: "minutes autoresearch compare-decode-hints".into(),
        generated_at: Local::now().to_rfc3339(),
        left_path: left.to_path_buf(),
        right_path: right.to_path_buf(),
        output_root: output_root
            .map(Path::to_path_buf)
            .unwrap_or_else(autoresearch::default_comparison_root),
    };
    let artifacts = autoresearch::write_decode_hint_eval_comparison_artifacts(&request, &report)?;

    if json {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AutoresearchDecodeHintsComparisonOutput {
            report: minutes_core::autoresearch::DecodeHintEvalComparisonReport,
            artifacts: DecodeHintEvalComparisonArtifactPaths,
        }

        let envelope = json_envelope(
            "minutes autoresearch compare-decode-hints",
            AutoresearchDecodeHintsComparisonOutput {
                report,
                artifacts: artifacts.clone(),
            },
        );
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        println!("Decode hint eval comparison");
        println!("Shared cases: {}", report.totals.shared_cases);
        println!("Added cases: {}", report.totals.added_cases);
        println!("Removed cases: {}", report.totals.removed_cases);
        println!("Improved cases: {}", report.totals.improved_cases);
        println!("Regressed cases: {}", report.totals.regressed_cases);
        println!("Newly passing: {}", report.totals.newly_passing_cases);
        println!("Newly failing: {}", report.totals.newly_failing_cases);
        println!("Artifacts: {}", artifacts.run_dir.display());
    }

    Ok(())
}

fn cmd_autoresearch_list_decode_hints(limit: usize, json: bool) -> Result<()> {
    let runs = autoresearch::list_decode_hint_runs(limit)?;

    if json {
        let envelope = json_envelope("minutes autoresearch list-decode-hints", runs);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else if runs.is_empty() {
        println!("No decode-hint research runs found.");
    } else {
        println!("Recent decode-hint research runs");
        for run in runs {
            println!(
                "- {} [{}] {} cases, {} failed, {} improved, {} regressed, {} newly passing, {} newly failing",
                run.generated_at,
                run.kind,
                run.cases_total,
                run.cases_failed,
                run.improved_cases,
                run.regressed_cases,
                run.newly_passing_cases,
                run.newly_failing_cases
            );
            println!("  status: {}", run.status);
            println!("  source: {}", run.source_path.display());
            println!("  dir: {}", run.run_dir.display());
            println!("  summary: {}", run.summary_path.display());
        }
    }

    Ok(())
}

/// Stable JSON schema describing what this CLI build supports.
///
/// The MCP server probes `minutes capabilities --json` at boot and uses
/// the returned feature flags to decide which tools to register. This is
/// the canonical surface for feature detection (see #183 phase 2); it
/// replaces the earlier strict-equality version check.
///
/// Schema stability: `api_version` bumps only when the wire contract
/// (keys removed, semantics of existing keys changed) breaks in a
/// non-additive way. Adding new feature keys does NOT bump api_version;
/// callers must treat missing keys as `false` so they cope with older
/// CLIs that predate a given feature.
#[derive(Serialize)]
struct CapabilityReport {
    /// Semver version string, e.g. "0.14.0".
    version: String,
    /// Wire-contract version. Currently 1. Only bumps on breaking changes.
    api_version: u32,
    /// Map of feature name to whether this CLI build supports it.
    ///
    /// Alphabetical via `BTreeMap` so JSON output is deterministic and
    /// diffable across versions.
    features: std::collections::BTreeMap<String, bool>,
}

fn build_capability_report() -> CapabilityReport {
    // Seed the map with every feature this CLI build supports. The MCP
    // server reads missing keys as "not supported", so adding a key here
    // is additive and safe.
    //
    // Policy: when adding a new MCP-visible surface backed by a CLI
    // subcommand, add its stable feature name here in the same commit.
    // That is the contract the MCP server uses to decide whether to
    // register the corresponding tool.
    let mut features = std::collections::BTreeMap::new();

    // Desktop context surface (new in 0.14.0). Backed by
    // `minutes context activity-summary|search|get-moment`.
    features.insert("activity_summary".into(), true);
    features.insert("search_context".into(), true);
    features.insert("get_moment".into(), true);
    features.insert("screen_context".into(), true);

    // Stable surfaces. Listed explicitly so consumers can probe for
    // them without relying on version-string inference.
    features.insert("add_note".into(), true);
    features.insert("confirm_speaker".into(), true);
    features.insert("consistency_report".into(), true);
    features.insert("copilot_realtime".into(), true);
    features.insert("events_since_seq".into(), true);
    features.insert("get_meeting".into(), true);
    features.insert("get_meeting_insights".into(), true);
    features.insert("get_person_profile".into(), true);
    features.insert("get_status".into(), true);
    features.insert("ingest_meeting".into(), true);
    features.insert("knowledge_status".into(), true);
    features.insert("list_meetings".into(), true);
    features.insert("list_processing_jobs".into(), true);
    features.insert("list_voices".into(), true);
    features.insert("open_dashboard".into(), true);
    features.insert("process_audio".into(), true);
    features.insert("qmd_collection_status".into(), true);
    features.insert("read_live_transcript".into(), true);
    features.insert("register_qmd_collection".into(), true);
    features.insert("relationship_map".into(), true);
    features.insert("research_topic".into(), true);
    features.insert("search_meetings".into(), true);
    features.insert("start_dictation".into(), true);
    features.insert("start_live_transcript".into(), true);
    features.insert("start_recording".into(), true);
    features.insert("stop_dictation".into(), true);
    features.insert("stop_recording".into(), true);
    features.insert("track_commitments".into(), true);

    // Cargo-feature-gated capabilities. Some are surfaced through the
    // feature flags so consumers know the build's runtime support.
    features.insert("parakeet".into(), cfg!(feature = "parakeet"));
    features.insert("diarize".into(), cfg!(feature = "diarize"));

    // Setup demo fixtures (new in 0.13.3).
    features.insert("setup_demo".into(), true);

    CapabilityReport {
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: 1,
        features,
    }
}

fn cmd_capabilities(json: bool) -> Result<()> {
    let report = build_capability_report();

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!("Minutes CLI capabilities");
    println!("  version: {}", report.version);
    println!("  api_version: {}", report.api_version);
    println!("  features:");
    for (name, supported) in &report.features {
        let marker = if *supported { "yes" } else { "no" };
        println!("    {}: {}", name, marker);
    }
    Ok(())
}

fn cmd_apple_speech_capabilities(json: bool) -> Result<()> {
    let report = apple_speech::probe_capabilities()?;

    if json {
        let envelope = json_envelope("minutes apple-speech capabilities", report);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }

    println!("Apple speech capability probe");
    println!("OS: {}", report.os_version);
    println!("Runtime supported: {}", report.runtime_supported);
    println!(
        "SpeechTranscriber available: {}",
        report
            .speech_transcriber
            .is_available
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".into())
    );
    println!(
        "SpeechTranscriber asset status: {}",
        report.speech_transcriber.asset_status
    );
    println!(
        "DictationTranscriber asset status: {}",
        report.dictation_transcriber.asset_status
    );
    if !report.speech_transcriber.installed_locales.is_empty() {
        println!(
            "SpeechTranscriber installed locales: {}",
            report.speech_transcriber.installed_locales.join(", ")
        );
    }
    if !report.dictation_transcriber.installed_locales.is_empty() {
        println!(
            "DictationTranscriber installed locales: {}",
            report.dictation_transcriber.installed_locales.join(", ")
        );
    }
    if !report.notes.is_empty() {
        println!("Notes:");
        for note in &report.notes {
            println!("- {}", note);
        }
    }

    Ok(())
}

fn cmd_apple_speech_benchmark(
    corpus: &Path,
    output_root: Option<&Path>,
    json: bool,
    config: &Config,
) -> Result<()> {
    let report = apple_speech::run_benchmark_corpus(corpus, config)?;
    let request = AppleSpeechBenchmarkRequest {
        command: "minutes apple-speech benchmark".into(),
        generated_at: Local::now().to_rfc3339(),
        corpus_path: corpus.to_path_buf(),
        output_root: output_root
            .map(Path::to_path_buf)
            .unwrap_or_else(apple_speech::default_research_root),
        configured_engine: config.transcription.engine.clone(),
    };
    let artifacts = apple_speech::write_benchmark_artifacts(&request, &report)?;

    if json {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct AppleSpeechBenchmarkOutput {
            report: minutes_core::apple_speech::AppleSpeechBenchmarkReport,
            artifacts: AppleSpeechBenchmarkArtifactPaths,
        }

        let envelope = json_envelope(
            "minutes apple-speech benchmark",
            AppleSpeechBenchmarkOutput {
                report,
                artifacts: artifacts.clone(),
            },
        );
        println!("{}", serde_json::to_string_pretty(&envelope)?);
        return Ok(());
    }

    println!("Apple speech benchmark complete");
    println!("Cases: {}", report.cases.len());
    println!("Artifacts: {}", artifacts.run_dir.display());
    println!(
        "SpeechTranscriber avg elapsed: {} ms",
        report
            .totals
            .speech_transcriber
            .average_elapsed_ms
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".into())
    );
    println!(
        "DictationTranscriber avg elapsed: {} ms",
        report
            .totals
            .dictation_transcriber
            .average_elapsed_ms
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".into())
    );
    println!(
        "Whisper avg elapsed: {} ms",
        report
            .totals
            .whisper
            .average_elapsed_ms
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".into())
    );
    println!(
        "Parakeet avg elapsed: {} ms",
        report
            .totals
            .parakeet
            .average_elapsed_ms
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "n/a".into())
    );

    Ok(())
}

fn current_git_commit() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn cmd_watch(dir: Option<&Path>, config: &Config) -> Result<()> {
    config.ensure_dirs()?;

    // Set up Ctrl-C to release the lock and exit cleanly
    ctrlc::set_handler(move || {
        eprintln!("\nStopping watcher...");
        minutes_core::parakeet_sidecar::shutdown_global_parakeet_sidecar();
        // Release the watch lock before exiting
        let lock_path = minutes_core::watch::lock_path();
        std::fs::remove_file(&lock_path).ok();
        std::process::exit(0);
    })?;

    // Run watcher directly (blocks until interrupted)
    minutes_core::watch::run(dir, config).map_err(|e| anyhow::anyhow!("{}", e))
}

fn cmd_devices() -> Result<()> {
    let devices = minutes_core::capture::list_input_devices();
    if devices.is_empty() {
        eprintln!("No audio input devices found.");
    } else {
        // Human-readable to stderr, JSON to stdout (consistent with other commands)
        eprintln!("Audio input devices:");
        for d in &devices {
            eprintln!("  {}", d);
        }
        let json = serde_json::to_string_pretty(&devices)?;
        println!("{}", json);
    }

    // Platform-specific virtual audio hints
    #[cfg(target_os = "macos")]
    eprintln!("\nTip: Install BlackHole for system audio capture: brew install blackhole-2ch");
    #[cfg(target_os = "windows")]
    eprintln!("\nTip: Install VB-CABLE for system audio capture: https://vb-audio.com/Cable/");
    #[cfg(target_os = "linux")]
    eprintln!(
        "\nTip: System audio capture works automatically when PipeWire or PulseAudio is running. \
         Run `minutes sources` for the categorized view."
    );

    Ok(())
}

fn cmd_sources() -> Result<()> {
    use minutes_core::capture::{list_devices_categorized, DeviceCategory};

    let devices = list_devices_categorized();
    if devices.is_empty() {
        eprintln!("No audio input devices found.");
        return Ok(());
    }

    let mics: Vec<_> = devices
        .iter()
        .filter(|d| d.category == DeviceCategory::Microphone)
        .collect();
    let system: Vec<_> = devices
        .iter()
        .filter(|d| d.category == DeviceCategory::SystemAudio)
        .collect();
    let virtual_devs: Vec<_> = devices
        .iter()
        .filter(|d| d.category == DeviceCategory::Virtual)
        .collect();

    eprintln!("Microphones:");
    for d in &mics {
        let marker = if d.is_default { "* " } else { "  " };
        eprintln!(
            "  {}{} ({}Hz, {} ch)",
            marker, d.name, d.sample_rate, d.channels
        );
    }

    if !system.is_empty() {
        eprintln!("\nSystem Audio:");
        for d in &system {
            eprintln!("    {} ({}Hz, {} ch)", d.name, d.sample_rate, d.channels);
        }
    } else {
        eprintln!("\nSystem Audio:");
        eprintln!("    (none detected)");
        #[cfg(target_os = "macos")]
        eprintln!("    Install a loopback driver: brew install blackhole-2ch");
        #[cfg(target_os = "linux")]
        eprintln!(
            "    On PipeWire, your speakers/headphones are the system-audio sources.\n    \
             On PulseAudio, look for source names ending in `.monitor`.\n    \
             If neither shows up here, check `wpctl status` or `pactl list sinks`."
        );
        eprintln!("    Or use the Minutes desktop app for native call capture (no driver needed).");
    }

    if !virtual_devs.is_empty() {
        eprintln!("\nVirtual Devices:");
        for d in &virtual_devs {
            eprintln!("    {} ({}Hz, {} ch)", d.name, d.sample_rate, d.channels);
        }
    }

    // JSON output to stdout
    let json_devices: Vec<serde_json::Value> = devices
        .iter()
        .map(|d| {
            serde_json::json!({
                "name": d.name,
                "category": format!("{:?}", d.category),
                "sample_rate": d.sample_rate,
                "channels": d.channels,
                "is_default": d.is_default,
            })
        })
        .collect();
    println!("{}", serde_json::to_string_pretty(&json_devices)?);

    Ok(())
}

fn cmd_setup(model: &str, list: bool, diarization: bool) -> Result<()> {
    if list {
        eprintln!("Available whisper models:");
        eprintln!("  tiny      75 MB   (fastest, lowest quality)");
        eprintln!("  base     142 MB");
        eprintln!("  small    466 MB   (recommended default)");
        eprintln!("  medium   1.5 GB");
        eprintln!("  large-v3 3.1 GB   (best quality, slower)");
        eprintln!();
        eprintln!("Speaker diarization:");
        eprintln!("  --diarization   34 MB   (pyannote-rs: segmentation + speaker embedding)");
        eprintln!();
        eprintln!("Sherpa engine (recommended for multilingual / SOTA, --sherpa):");
        eprintln!(
            "  parakeet-tdt-0.6b-v3-int8  ~670 MB   (in-process, no Python; `minutes setup --sherpa` downloads + enables it)"
        );
        eprintln!();
        eprintln!("Parakeet.cpp models (alternative subprocess engine, --parakeet):");
        eprintln!("  tdt-ctc-110m  ~220 MB   (English, fast)");
        eprintln!("  tdt-600m      ~1.2 GB   (multilingual v3, 25 EU languages, best quality)");
        return Ok(());
    }

    if diarization {
        return cmd_setup_diarization();
    }

    let valid_models = ["tiny", "base", "small", "medium", "large-v3"];
    if !valid_models.contains(&model) {
        anyhow::bail!(
            "unknown model: {}. Available: {}",
            model,
            valid_models.join(", ")
        );
    }

    let config = Config::default();
    let model_dir = &config.transcription.model_path;
    std::fs::create_dir_all(model_dir)?;

    let dest = model_dir.join(format!("ggml-{}.bin", model));
    let expected_min_bytes = minutes_core::transcribe::expected_whisper_model_size_bytes(model);
    let mb = |bytes: u64| bytes as f64 / 1_048_576.0;

    // Helper: treat an existing file as truncated if it's smaller than the
    // expected minimum for this model (issue #229 root cause: a partial
    // download was reported as "already downloaded" and whisper-rs aborted
    // parsing the truncated GGML header). Returns Ok(true) when the
    // existing file should be kept, Ok(false) when it was removed and a
    // re-download is needed.
    let keep_existing = if dest.exists() {
        let actual = std::fs::metadata(&dest)?.len();
        match expected_min_bytes {
            Some(min_bytes) if actual < min_bytes => {
                eprintln!(
                    "Model file at {} is {:.0} MB but the {} model should be at least {:.0} MB.",
                    dest.display(),
                    mb(actual),
                    model,
                    mb(min_bytes),
                );
                eprintln!(
                    "Looks truncated, probably an interrupted download. Removing and refetching."
                );
                std::fs::remove_file(&dest)?;
                false
            }
            _ => {
                eprintln!(
                    "Model already downloaded: {} ({:.0} MB)",
                    dest.display(),
                    mb(actual),
                );
                true
            }
        }
    } else {
        false
    };

    if !keep_existing {
        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
            model
        );

        eprintln!("Downloading whisper model: {} ...", model);
        download_file(&url, &dest)?;

        // Validate the freshly downloaded file too. A partial download
        // that ends in a successful HTTP close (rare but possible) would
        // otherwise slip through silently.
        if let Some(min_bytes) = expected_min_bytes {
            let actual = std::fs::metadata(&dest)?.len();
            if actual < min_bytes {
                let _ = std::fs::remove_file(&dest);
                anyhow::bail!(
                    "downloaded model is {:.0} MB but expected at least {:.0} MB for {}; the file was truncated and has been removed. Try running `minutes setup --model {}` again on a stable connection.",
                    mb(actual),
                    mb(min_bytes),
                    model,
                    model,
                );
            }
        }

        // Update config hint
        eprintln!("\nTo use this model, add to ~/.config/minutes/config.toml:");
        eprintln!("  [transcription]");
        eprintln!("  model = \"{}\"", model);
    }

    // Auto-download Silero VAD model (prevents transcription loops on non-English audio)
    let vad_dest = model_dir.join("ggml-silero-v6.2.0.bin");
    if !vad_dest.exists() {
        let vad_url =
            "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v6.2.0.bin";
        eprintln!("Downloading Silero VAD model (~885 KB) ...");
        if let Err(e) = download_file(vad_url, &vad_dest) {
            eprintln!(
                "Warning: VAD model download failed ({}). Transcription will still work \
                 but may produce loops on non-English audio.",
                e
            );
        }
    }

    // Streaming Silero ONNX (used by the OrtSileroVad engine). Only
    // downloaded when this build was compiled with the `vad-ort`
    // feature, since builds without it cannot use the file.
    //
    // Naming note: the file is `silero-vad-v6.2.0.onnx` to mirror the
    // existing ggml file naming convention used by ggml-org's whisper
    // mirror. The actual upstream artifact is from the snakers4
    // `v6.0` git tag — that's the most recent upstream release that
    // ships the ONNX. Keep the URL pinned to a tag, never to a
    // mutable branch, so the schema this code was written against
    // (input [1,576], state [2,1,128], sr i64 scalar, output [1,1])
    // stays stable.
    #[cfg(feature = "vad-ort")]
    {
        let onnx_dest = model_dir.join("silero-vad-v6.2.0.onnx");
        if !onnx_dest.exists() {
            let onnx_url = "https://github.com/snakers4/silero-vad/raw/v6.0/src/silero_vad/data/silero_vad.onnx";
            eprintln!("Downloading Silero VAD ONNX from snakers4 v6.0 tag (~2.3 MB) ...");
            if let Err(e) = download_file(onnx_url, &onnx_dest) {
                eprintln!(
                    "Warning: Silero ONNX download failed ({}). The streaming VAD engine \
                     will not be available; recordings will continue using whisper-rs's \
                     bundled Silero.",
                    e
                );
            }
        }
    }

    // Also list available input devices
    let devices = minutes_core::capture::list_input_devices();
    if !devices.is_empty() {
        eprintln!("\nAvailable audio input devices:");
        for d in &devices {
            eprintln!("  {}", d);
        }
    }

    Ok(())
}

fn cmd_setup_demo() -> Result<()> {
    let demo_dir = Config::minutes_dir().join("demo");
    let install = demo_data::install_mcp_demo_fixtures(&demo_dir)?;

    if install.updated_fixtures == 0 {
        eprintln!(
            "Demo corpus already ready at: {}",
            install.demo_dir.display()
        );
    } else {
        eprintln!(
            "Demo corpus ready at: {} ({} fixture meetings)",
            install.demo_dir.display(),
            install.total_fixtures
        );
    }

    eprintln!("Use it with MCP or any agent client by pointing MEETINGS_DIR at that folder:");
    eprintln!();
    eprintln!("  {{");
    eprintln!("    \"mcpServers\": {{");
    eprintln!("      \"minutes-demo\": {{");
    eprintln!("        \"command\": \"npx\",");
    eprintln!("        \"args\": [\"minutes-mcp\"],");
    eprintln!(
        "        \"env\": {{ \"MEETINGS_DIR\": \"{}\" }}",
        install.demo_dir.display()
    );
    eprintln!("      }}");
    eprintln!("    }}");
    eprintln!("  }}");
    eprintln!();
    eprintln!("Try asking your agent:");
    eprintln!("  - List the meetings in this corpus.");
    eprintln!("  - What did we decide about pricing? Which decision is current?");
    eprintln!("  - What got killed in the last product prioritization meeting?");
    eprintln!("  - What action items are still open, and who owns each?");
    eprintln!("  - Summarize the Northwind customer thread.");

    Ok(())
}

fn cmd_setup_diarization() -> Result<()> {
    use minutes_core::diarize;

    let config = Config::load();
    let emb_info = diarize::embedding_model_for_config(&config);
    let model_dir = &config.diarization.model_path;
    std::fs::create_dir_all(model_dir)?;

    eprintln!(
        "Embedding model: {} ({})",
        config.diarization.embedding_model, emb_info.filename
    );

    let models: [(&str, &str, &str); 2] = [
        (
            diarize::SEGMENTATION_MODEL,
            diarize::SEGMENTATION_MODEL_URL,
            "segmentation",
        ),
        (emb_info.filename, emb_info.url, "speaker embedding"),
    ];

    let mut all_exist = true;
    for (filename, url, label) in &models {
        let dest = model_dir.join(filename);
        if dest.exists() {
            let size = std::fs::metadata(&dest)?.len();
            eprintln!(
                "Already downloaded: {} ({:.1} MB)",
                filename,
                size as f64 / 1_048_576.0
            );
        } else {
            all_exist = false;
            eprintln!("Downloading {} model: {} ...", label, filename);
            download_file(url, &dest)?;
        }
    }

    if all_exist {
        eprintln!("\nAll diarization models are installed.");
    } else {
        eprintln!("\nDiarization models installed.");
    }

    eprintln!("\nTo enable speaker diarization, add to ~/.config/minutes/config.toml:");
    eprintln!("  [diarization]");
    eprintln!("  engine = \"pyannote-rs\"");
    eprintln!("  # embedding_model = \"cam++-lm\"  # or \"cam++\" for the lighter original");

    Ok(())
}

/// Set up a parakeet.cpp model for alternative transcription.
///
/// Parakeet models are distributed as .nemo files on HuggingFace and must be
/// converted to safetensors format using parakeet.cpp's convert_nemo.py script.
/// This command prints the steps needed and checks for existing files.
fn cmd_setup_parakeet(model: &str) -> Result<()> {
    let valid_models = VALID_PARAKEET_MODELS;
    if !valid_models.contains(&model) {
        anyhow::bail!(
            "unknown parakeet model: {}. Available: {}",
            model,
            valid_models.join(", ")
        );
    }

    let config = Config::default();
    let model_dir = parakeet::install_dir(&config, model);
    std::fs::create_dir_all(&model_dir)?;

    let dest_model = model_dir.join(parakeet::default_model_filename(model));
    let dest_vocab_name = parakeet::default_tokenizer_filename(model);
    let dest_vocab = model_dir.join(&dest_vocab_name);
    let native_vad_dest = parakeet::installs_root(&config).join("silero_vad_v5.safetensors");

    // Map model name to HuggingFace repo
    let hf_repo = match model {
        "tdt-ctc-110m" => "nvidia/parakeet-tdt_ctc-110m",
        "tdt-600m" => "nvidia/parakeet-tdt-0.6b-v3",
        _ => unreachable!(),
    };

    // Check if model already exists
    let model_exists = dest_model.exists();
    let vocab_exists = dest_vocab.exists();

    if model_exists && vocab_exists {
        let size = std::fs::metadata(&dest_model)?.len();
        eprintln!(
            "Model already set up: {} ({:.0} MB)",
            dest_model.display(),
            size as f64 / 1_048_576.0
        );
        eprintln!("Vocab file: {}", dest_vocab.display());
        if let Ok(metadata_path) =
            parakeet::write_install_metadata(&config, model, &dest_model, &dest_vocab)
        {
            eprintln!("Metadata file: {}", metadata_path.display());
        }
    } else {
        eprintln!("Parakeet model setup: {}", model);
        eprintln!();
        eprintln!("Parakeet models require a one-time conversion from NVIDIA's .nemo format.");
        eprintln!("Follow these steps:");
        eprintln!();
        eprintln!("  Install directory:");
        eprintln!("    {}", model_dir.display());
        eprintln!();
        eprintln!("  Step 1: Clone parakeet.cpp");
        eprintln!("    git clone https://github.com/Frikallo/parakeet.cpp");
        eprintln!("    cd parakeet.cpp");
        eprintln!();
        eprintln!("  Step 2: Download the .nemo model from HuggingFace");
        eprintln!(
            "    hf download {} --include '*.nemo' --local-dir .",
            hf_repo
        );
        eprintln!();
        eprintln!("  Step 3: Convert to safetensors");
        let convert_model_arg = match model {
            "tdt-ctc-110m" => "110m-tdt-ctc",
            "tdt-600m" => "600m-tdt",
            _ => unreachable!(),
        };
        eprintln!(
            "    python scripts/convert_nemo.py *.nemo -o {} --model {}",
            dest_model.display(),
            convert_model_arg
        );
        eprintln!();
        eprintln!("  Step 4: Extract the SentencePiece tokenizer vocab");
        eprintln!("    tar xf *.nemo --wildcards --no-anchored '*tokenizer.vocab'");
        eprintln!("    cp *_tokenizer.vocab {}", dest_vocab.display());
        eprintln!();
        eprintln!("  Step 5: Build and install the parakeet binary");
        eprintln!("    mkdir build && cd build && cmake .. && make -j");
        eprintln!("    cp parakeet /usr/local/bin/");

        if model_exists {
            eprintln!();
            eprintln!(
                "Note: model file already present at {}",
                dest_model.display()
            );
        }
        if vocab_exists {
            eprintln!(
                "Note: vocab file already present at {}",
                dest_vocab.display()
            );
        }
    }

    if !native_vad_dest.exists() {
        if let Some(parent) = native_vad_dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&native_vad_dest, PARAKEET_NATIVE_VAD_WEIGHTS)?;
        let size = std::fs::metadata(&native_vad_dest)?.len();
        eprintln!(
            "Installed native Parakeet VAD weights: {} ({:.1} MB)",
            native_vad_dest.display(),
            size as f64 / 1_048_576.0
        );
    } else {
        let size = std::fs::metadata(&native_vad_dest)?.len();
        eprintln!(
            "Native Parakeet VAD weights already installed: {} ({:.1} MB)",
            native_vad_dest.display(),
            size as f64 / 1_048_576.0
        );
    }

    // Verify parakeet binary is available
    eprintln!();
    match minutes_core::parakeet::resolve_parakeet_binary(
        "parakeet",
        minutes_core::parakeet::ResolveParakeetBinaryMode::WarnAndFallback,
    ) {
        Ok(path) => {
            eprintln!("Resolved parakeet binary: {}", path.display());
        }
        Err(_) => {
            eprintln!("Warning: no working `parakeet` binary was found.");
            eprintln!("See: https://github.com/Frikallo/parakeet.cpp");
        }
    }

    // Verify the warm-sidecar binary (#295). Without it, live transcription
    // pays a full model load per utterance and will lag badly on real meetings.
    match minutes_core::parakeet_sidecar::resolve_server_binary("parakeet") {
        Some(path) => {
            eprintln!(
                "Resolved example-server (warm live sidecar): {}",
                path.display()
            );
            eprintln!("Live transcription will auto-use the warm sidecar.");
        }
        None => {
            eprintln!("Note: `example-server` (warm live sidecar) was not found.");
            eprintln!("Live transcription will fall back to slow cold per-utterance runs.");
            eprintln!(
                "To build it, configure parakeet.cpp with -DPARAKEET_BUILD_SERVER_EXAMPLE=ON"
            );
            eprintln!("and copy build/**/example-server next to your parakeet binary.");
            eprintln!("Details: https://github.com/silverstein/minutes/blob/main/docs/architecture/parakeet.md");
        }
    }

    eprintln!();
    eprintln!("To use parakeet, add to ~/.config/minutes/config.toml:");
    eprintln!("  [transcription]");
    eprintln!("  engine = \"parakeet\"");
    eprintln!("  parakeet_model = \"{}\"", model);
    eprintln!("  parakeet_vocab = \"{}\"", dest_vocab_name);
    // Print parakeet_binary too. Users who installed parakeet.cpp from
    // source typically land at ~/.local/bin/parakeet, which is reachable
    // from a Terminal-launched CLI but NOT from a Finder/Spotlight/Dock-
    // launched desktop app (different PATH). Spelling the binary out in
    // the config footer prevents the "minutes works in terminal, app
    // says binary-not-found" class of issue.
    eprintln!(
        "  parakeet_binary = \"<absolute path to parakeet, e.g. /Users/you/.local/bin/parakeet>\""
    );

    // Feature-flag visibility check. If this binary was compiled without
    // `--features parakeet`, every config key above is silently inert at
    // runtime and the engine falls back to whisper. The setup command
    // itself runs to completion because download + binary resolution don't
    // require the feature, so without this warning a user can follow every
    // step successfully and still wonder why parakeet isn't transcribing.
    //
    // Tagged release artifacts (the DMG and the per-platform CLI binaries
    // built by .github/workflows/release-{macos,cli}.yml) ship WITH the
    // feature. The paths that don't are the Homebrew Formula CLI
    // (`brew install silverstein/tap/minutes`, which runs bare
    // `cargo install --path crates/cli`) and any source `cargo install`
    // without `--features parakeet`.
    //
    // Confirmed reachable: this function is not feature-gated, so the
    // warning fires correctly on a whisper-only binary.
    if !cfg!(feature = "parakeet") {
        eprintln!();
        eprintln!("WARNING: this minutes binary was compiled WITHOUT the parakeet feature.");
        eprintln!("The model and helper binary above are installed, but the runtime will fall");
        eprintln!("back to whisper regardless of the config keys you just set. To actually use");
        eprintln!("parakeet, rebuild the CLI with the feature enabled, e.g.:");
        eprintln!();
        eprintln!("  cargo install --path crates/cli --features parakeet --root ~/.cargo --force");
        eprintln!();
        eprintln!("The downloadable DMG and tagged CLI release binaries do include parakeet.");
        eprintln!("The Homebrew Formula CLI (`brew install silverstein/tap/minutes`) and bare");
        eprintln!("`cargo install minutes-cli` runs are the install paths that omit it.");
    }

    Ok(())
}

/// Download a file from a URL to a destination path, with progress reporting.
/// Download the sherpa-onnx parakeet-tdt-0.6b-v3 (int8) model for the opt-in
/// `engine-sherpa` transcription engine into the resolved model directory.
fn cmd_setup_sherpa(config: &Config) -> Result<()> {
    let dir = minutes_core::sherpa_engine::model_dir(config);
    eprintln!("Installing sherpa-onnx parakeet-tdt-0.6b-v3 (int8) model");
    eprintln!("  Dir: {}", dir.display());
    std::fs::create_dir_all(&dir)
        .map_err(|e| anyhow::anyhow!("failed to create model dir {}: {}", dir.display(), e))?;
    let base =
        "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main";
    for (file, min) in minutes_core::sherpa_engine::MODEL_FILES {
        let dest = dir.join(file);
        // Skip only if the file already meets its size floor; re-download
        // zero-byte / truncated leftovers instead of trusting existence alone.
        if dest.metadata().map(|m| m.len() >= min).unwrap_or(false) {
            eprintln!("  {file} already present, skipping");
            continue;
        }
        eprintln!("Downloading {file} ...");
        download_file(&format!("{base}/{file}"), &dest)?;
    }
    if !minutes_core::sherpa_engine::model_files_present(&dir) {
        anyhow::bail!(
            "sherpa model install incomplete; some files are missing in {}",
            dir.display()
        );
    }
    eprintln!("\nSherpa model ready.");

    // One-command UX: make sherpa the active engine. Safe to set unconditionally
    // -- if this binary wasn't built with `--features engine-sherpa`, or the model
    // ever goes missing, transcription auto-falls-back to whisper (with a warning),
    // so the recording never breaks.
    let mut cfg = Config::load();
    if cfg.transcription.engine != "sherpa" {
        cfg.transcription.engine = "sherpa".to_string();
        match cfg.save() {
            Ok(()) => eprintln!(
                "Set transcription.engine = \"sherpa\" in {}",
                Config::config_path().display()
            ),
            Err(e) => eprintln!(
                "(could not write config: {e}). Set transcription.engine = \"sherpa\" manually in {}",
                Config::config_path().display()
            ),
        }
    }
    if !cfg!(feature = "engine-sherpa") {
        eprintln!(
            "Note: this build lacks the sherpa engine, so transcription falls back to whisper \
             until you build with `--features engine-sherpa`."
        );
    }
    Ok(())
}

fn download_file(url: &str, dest: &std::path::Path) -> Result<()> {
    eprintln!("  From: {}", url);
    eprintln!("  To:   {}", dest.display());

    let response = ureq::get(url)
        .call()
        .map_err(|e| anyhow::anyhow!("download failed: {}. Check your internet connection.", e))?;

    let content_length = response
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let mut reader = response.into_body().into_reader();
    let tmp_dest = dest.with_extension("partial");
    let mut file = std::fs::File::create(&tmp_dest)?;
    let mut downloaded: u64 = 0;
    let mut buf = vec![0u8; 64 * 1024];
    let mut last_report = std::time::Instant::now();

    loop {
        let n = std::io::Read::read(&mut reader, &mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n])?;
        downloaded += n as u64;

        if last_report.elapsed().as_millis() > 500 {
            if let Some(total) = content_length {
                eprint!(
                    "\r  {:.0} / {:.0} MB ({:.0}%)",
                    downloaded as f64 / 1_048_576.0,
                    total as f64 / 1_048_576.0,
                    downloaded as f64 / total as f64 * 100.0
                );
            } else {
                eprint!("\r  {:.0} MB downloaded", downloaded as f64 / 1_048_576.0);
            }
            last_report = std::time::Instant::now();
        }
    }
    eprintln!();
    drop(file);

    // Rename from partial to final (atomic on most filesystems)
    std::fs::rename(&tmp_dest, dest).map_err(|e| {
        std::fs::remove_file(&tmp_dest).ok();
        anyhow::anyhow!("failed to save model: {}", e)
    })?;

    let size = std::fs::metadata(dest)?.len();
    eprintln!("  Done! Saved ({:.1} MB)", size as f64 / 1_048_576.0);

    Ok(())
}

#[derive(Debug, Clone, Serialize)]
struct QmdCollectionInfo {
    name: String,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
struct QmdStatusReport {
    qmd_available: bool,
    output_dir: PathBuf,
    target_collection: String,
    registered: bool,
    matching_collections: Vec<QmdCollectionInfo>,
    config_engine: String,
    config_collection: Option<String>,
}

fn parse_qmd_collection_names(stdout: &str) -> Vec<String> {
    let mut collections = Vec::new();

    for line in stdout.lines() {
        if let Some((name, _)) = line.split_once(" (qmd://") {
            collections.push(name.trim().to_string());
        }
    }

    collections
}

fn parse_qmd_collection_path(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .find_map(|line| line.trim_start().strip_prefix("Path:"))
        .map(|path| PathBuf::from(path.trim()))
}

fn normalize_path_for_compare(path: &Path) -> PathBuf {
    if path.exists() {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    } else {
        path.to_path_buf()
    }
}

fn content_type_path_matches(output_dir: &Path, candidate: &Path) -> bool {
    normalize_path_for_compare(output_dir) == normalize_path_for_compare(candidate)
}

fn qmd_status_report(collection: &str, config: &Config) -> Result<QmdStatusReport> {
    let output_dir = normalize_path_for_compare(&config.output_dir);
    let output = match std::process::Command::new("qmd")
        .args(["collection", "list"])
        .output()
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(QmdStatusReport {
                qmd_available: false,
                output_dir,
                target_collection: collection.to_string(),
                registered: false,
                matching_collections: Vec::new(),
                config_engine: config.search.engine.clone(),
                config_collection: config.search.qmd_collection.clone(),
            });
        }
        Err(error) => return Err(error.into()),
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        anyhow::bail!("{}", if !stderr.is_empty() { stderr } else { stdout });
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut matching_collections = Vec::new();
    for candidate_name in parse_qmd_collection_names(&stdout) {
        let show_output = std::process::Command::new("qmd")
            .args(["collection", "show", &candidate_name])
            .output()?;
        if !show_output.status.success() {
            continue;
        }

        let show_stdout = String::from_utf8_lossy(&show_output.stdout);
        if let Some(path) = parse_qmd_collection_path(&show_stdout) {
            let candidate = QmdCollectionInfo {
                name: candidate_name,
                path,
            };
            if content_type_path_matches(&output_dir, &candidate.path) {
                matching_collections.push(candidate);
            }
        }
    }
    let registered = matching_collections
        .iter()
        .any(|candidate| candidate.name == collection);

    Ok(QmdStatusReport {
        qmd_available: true,
        output_dir,
        target_collection: collection.to_string(),
        registered,
        matching_collections,
        config_engine: config.search.engine.clone(),
        config_collection: config.search.qmd_collection.clone(),
    })
}

fn cmd_qmd(action: &str, collection: &str, config: &Config) -> Result<()> {
    match action {
        "status" => {
            let report = qmd_status_report(collection, config)?;

            if !report.qmd_available {
                eprintln!("QMD is not installed or not on PATH.");
                eprintln!(
                    "Install qmd, then run: minutes qmd register --collection {}",
                    collection
                );
            } else if report.registered {
                eprintln!(
                    "QMD collection '{}' already indexes {}",
                    collection,
                    report.output_dir.display()
                );
            } else if report.matching_collections.is_empty() {
                eprintln!("{} is not indexed in QMD yet.", report.output_dir.display());
                eprintln!("Run: minutes qmd register --collection {}", collection);
            } else {
                eprintln!(
                    "{} is already indexed in QMD under: {}",
                    report.output_dir.display(),
                    report
                        .matching_collections
                        .iter()
                        .map(|candidate| candidate.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                eprintln!("Run: minutes qmd register --collection {}", collection);
            }

            if report.config_engine != "qmd"
                || report.config_collection.as_deref() != Some(collection)
            {
                eprintln!("\nTo opt into QMD search, add to ~/.config/minutes/config.toml:");
                eprintln!("  [search]");
                eprintln!("  engine = \"qmd\"");
                eprintln!("  qmd_collection = \"{}\"", collection);
            }

            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        "register" => {
            config.ensure_dirs()?;
            let initial = qmd_status_report(collection, config)?;

            if !initial.qmd_available {
                anyhow::bail!(
                    "qmd is not installed or not on PATH. Install qmd, then rerun this command."
                );
            }

            if initial.registered {
                eprintln!(
                    "QMD collection '{}' already indexes {}",
                    collection,
                    initial.output_dir.display()
                );
                println!("{}", serde_json::to_string_pretty(&initial)?);
                return Ok(());
            }

            let output = std::process::Command::new("qmd")
                .arg("collection")
                .arg("add")
                .arg(&config.output_dir)
                .arg("--name")
                .arg(collection)
                .output()?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                anyhow::bail!("{}", if !stderr.is_empty() { stderr } else { stdout });
            }

            let report = qmd_status_report(collection, config)?;
            eprintln!(
                "Registered {} as QMD collection '{}'.",
                report.output_dir.display(),
                collection
            );
            eprintln!(
                "Run `qmd update -c {}` or `qmd embed` as needed to refresh the collection.",
                collection
            );

            if report.config_engine != "qmd"
                || report.config_collection.as_deref() != Some(collection)
            {
                eprintln!("\nTo opt into QMD search, add to ~/.config/minutes/config.toml:");
                eprintln!("  [search]");
                eprintln!("  engine = \"qmd\"");
                eprintln!("  qmd_collection = \"{}\"", collection);
            }

            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => anyhow::bail!("Unknown qmd action: {}. Use status or register.", action),
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn cmd_service(action: &str) -> Result<()> {
    let minutes_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("minutes"));
    let home = dirs::home_dir().unwrap_or_default();
    let log_dir = Config::minutes_dir().join("logs");
    let agents_dir = home.join("Library/LaunchAgents");
    let bin_str = minutes_bin.display().to_string();
    let home_str = home.display().to_string();
    let log_dir_str = log_dir.display().to_string();
    let path_env = format!(
        "{h}/.local/bin:{h}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin",
        h = home_str
    );

    // (label, plist_xml)
    let agents: Vec<(&str, String)> = vec![
        (
            "dev.getminutes.watcher",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.getminutes.watcher</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>watch</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{home}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
        <key>PATH</key>
        <string>{path}</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{logs}/watcher.log</string>
    <key>StandardErrorPath</key>
    <string>{logs}/watcher.log</string>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
    <key>Nice</key>
    <integer>5</integer>
    <key>ThrottleInterval</key>
    <integer>10</integer>
</dict>
</plist>"#,
                bin = bin_str,
                home = home_str,
                path = path_env,
                logs = log_dir_str,
            ),
        ),
        (
            "dev.getminutes.weekly-summary",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.getminutes.weekly-summary</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>automate</string>
        <string>weekly-summary</string>
        <string>--json</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{home}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
        <key>PATH</key>
        <string>{path}</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{logs}/weekly-summary.log</string>
    <key>StandardErrorPath</key>
    <string>{logs}/weekly-summary.log</string>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Weekday</key>
        <integer>0</integer>
        <key>Hour</key>
        <integer>19</integer>
        <key>Minute</key>
        <integer>0</integer>
    </dict>
    <key>Nice</key>
    <integer>10</integer>
</dict>
</plist>"#,
                bin = bin_str,
                home = home_str,
                path = path_env,
                logs = log_dir_str,
            ),
        ),
        (
            "dev.getminutes.proactive-context",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>dev.getminutes.proactive-context</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
        <string>automate</string>
        <string>proactive-context</string>
        <string>--json</string>
    </array>
    <key>WorkingDirectory</key>
    <string>{home}</string>
    <key>EnvironmentVariables</key>
    <dict>
        <key>HOME</key>
        <string>{home}</string>
        <key>PATH</key>
        <string>{path}</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{logs}/proactive-context.log</string>
    <key>StandardErrorPath</key>
    <string>{logs}/proactive-context.log</string>
    <key>StartCalendarInterval</key>
    <dict>
        <key>Hour</key>
        <integer>8</integer>
        <key>Minute</key>
        <integer>0</integer>
    </dict>
    <key>Nice</key>
    <integer>10</integer>
</dict>
</plist>"#,
                bin = bin_str,
                home = home_str,
                path = path_env,
                logs = log_dir_str,
            ),
        ),
    ];

    match action {
        "install" => {
            std::fs::create_dir_all(&log_dir)?;
            std::fs::create_dir_all(&agents_dir)?;

            // Remove legacy weekly-lint if present (replaced by weekly-summary)
            let legacy = agents_dir.join("dev.getminutes.weekly-lint.plist");
            if legacy.exists() {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", &legacy.to_string_lossy()])
                    .status();
                let _ = std::fs::remove_file(&legacy);
                eprintln!(
                    "Removed legacy dev.getminutes.weekly-lint (replaced by weekly-summary)."
                );
            }

            for (label, plist) in &agents {
                let dest = agents_dir.join(format!("{}.plist", label));
                let was_loaded = dest.exists()
                    && std::process::Command::new("launchctl")
                        .args(["list", label])
                        .output()
                        .map(|o| o.status.success())
                        .unwrap_or(false);

                if was_loaded {
                    let _ = std::process::Command::new("launchctl")
                        .args(["unload", &dest.to_string_lossy()])
                        .status();
                }

                std::fs::write(&dest, plist)?;

                let status = std::process::Command::new("launchctl")
                    .args(["load", "-w", &dest.to_string_lossy()])
                    .status()?;

                let verb = if was_loaded { "reloaded" } else { "installed" };
                if status.success() {
                    eprintln!("  {} {}", verb, label);
                } else {
                    eprintln!("  FAILED {}", label);
                }
            }

            eprintln!();
            eprintln!("All services installed.");
            eprintln!("  Binary: {}", minutes_bin.display());
            eprintln!("  Logs:   {}", log_dir.display());
            eprintln!("  Watcher auto-starts on login; weekly-summary runs Sundays 7pm;");
            eprintln!("  proactive-context runs daily at 8am.");
        }
        "uninstall" => {
            let mut removed = 0;
            for (label, _) in &agents {
                let dest = agents_dir.join(format!("{}.plist", label));
                if dest.exists() {
                    let _ = std::process::Command::new("launchctl")
                        .args(["unload", &dest.to_string_lossy()])
                        .status();
                    std::fs::remove_file(&dest)?;
                    eprintln!("  removed {}", label);
                    removed += 1;
                }
            }
            if removed == 0 {
                eprintln!("No services installed.");
            } else {
                eprintln!("Uninstalled {} service(s).", removed);
            }
        }
        "restart" => {
            let uid = unsafe { libc::getuid() };
            for (label, _) in &agents {
                let dest = agents_dir.join(format!("{}.plist", label));
                if !dest.exists() {
                    continue;
                }
                let target = format!("gui/{}/{}", uid, label);
                let kicked = std::process::Command::new("launchctl")
                    .args(["kickstart", "-k", &target])
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);
                if !kicked {
                    let _ = std::process::Command::new("launchctl")
                        .args(["unload", &dest.to_string_lossy()])
                        .status();
                    let _ = std::process::Command::new("launchctl")
                        .args(["load", "-w", &dest.to_string_lossy()])
                        .status();
                }
                eprintln!("  restarted {}", label);
            }
        }
        "status" => {
            for (label, _) in &agents {
                let output = std::process::Command::new("launchctl")
                    .args(["list", label])
                    .output();
                match output {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        let pid = stdout
                            .lines()
                            .find(|l| l.contains("PID"))
                            .map(|l| l.trim().to_string())
                            .unwrap_or_default();
                        eprintln!("  running  {}  {}", label, pid);
                    }
                    _ => {
                        let dest = agents_dir.join(format!("{}.plist", label));
                        if dest.exists() {
                            eprintln!(
                                "  stopped  {}  (plist exists, try: minutes service install)",
                                label
                            );
                        } else {
                            eprintln!("  missing  {}", label);
                        }
                    }
                }
            }
        }
        _ => anyhow::bail!(
            "Unknown action: {}. Use install, uninstall, restart, or status.",
            action
        ),
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn cmd_service_linux(action: &str) -> Result<()> {
    let minutes_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("minutes"));
    let home = dirs::home_dir().unwrap_or_default();
    let bin_str = minutes_bin.display().to_string();
    let home_str = home.display().to_string();
    let systemd_dir = home.join(".config/systemd/user");
    let path_env = format!(
        "{h}/.local/bin:{h}/.cargo/bin:/usr/local/bin:/usr/bin:/bin",
        h = home_str
    );

    // (unit_name, unit_content, optional timer_content)
    let units: Vec<(&str, String, Option<String>)> = vec![
        (
            "minutes-watcher",
            format!(
                "[Unit]\nDescription=Minutes voice memo watcher\n\n[Service]\nType=simple\nExecStart={bin} watch\nRestart=on-failure\nRestartSec=10\nNice=5\nEnvironment=PATH={path}\n\n[Install]\nWantedBy=default.target\n",
                bin = bin_str, path = path_env
            ),
            None,
        ),
        (
            "minutes-weekly-summary",
            format!(
                "[Unit]\nDescription=Minutes weekly summary\n\n[Service]\nType=oneshot\nExecStart={bin} automate weekly-summary --json\nEnvironment=PATH={path}\n",
                bin = bin_str, path = path_env
            ),
            Some("[Unit]\nDescription=Minutes weekly summary timer\n\n[Timer]\nOnCalendar=Sun 19:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n".to_string()),
        ),
        (
            "minutes-proactive-context",
            format!(
                "[Unit]\nDescription=Minutes proactive context\n\n[Service]\nType=oneshot\nExecStart={bin} automate proactive-context --json\nEnvironment=PATH={path}\n",
                bin = bin_str, path = path_env
            ),
            Some("[Unit]\nDescription=Minutes proactive context timer\n\n[Timer]\nOnCalendar=*-*-* 08:00\nPersistent=true\n\n[Install]\nWantedBy=timers.target\n".to_string()),
        ),
    ];

    match action {
        "install" => {
            std::fs::create_dir_all(&systemd_dir)?;

            for (name, service, timer) in &units {
                let svc_path = systemd_dir.join(format!("{}.service", name));
                std::fs::write(&svc_path, service)?;
                eprintln!("  wrote {}.service", name);

                if let Some(timer_content) = timer {
                    let timer_path = systemd_dir.join(format!("{}.timer", name));
                    std::fs::write(&timer_path, timer_content)?;
                    eprintln!("  wrote {}.timer", name);
                }
            }

            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();

            for (name, _, timer) in &units {
                let target = if timer.is_some() {
                    format!("{}.timer", name)
                } else {
                    format!("{}.service", name)
                };
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "enable", "--now", &target])
                    .status();
                eprintln!("  enabled {}", target);
            }

            eprintln!();
            eprintln!("All services installed.");
            eprintln!("  Binary: {}", minutes_bin.display());
            eprintln!("  Units:  {}", systemd_dir.display());
        }
        "uninstall" => {
            for (name, _, timer) in &units {
                let targets: Vec<String> = if timer.is_some() {
                    vec![format!("{}.timer", name), format!("{}.service", name)]
                } else {
                    vec![format!("{}.service", name)]
                };
                for t in &targets {
                    let path = systemd_dir.join(t);
                    if path.exists() {
                        let _ = std::process::Command::new("systemctl")
                            .args(["--user", "disable", "--now", t])
                            .status();
                        std::fs::remove_file(&path)?;
                        eprintln!("  removed {}", t);
                    }
                }
            }
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
        }
        "restart" => {
            for (name, _, timer) in &units {
                let target = if timer.is_some() {
                    format!("{}.timer", name)
                } else {
                    format!("{}.service", name)
                };
                let _ = std::process::Command::new("systemctl")
                    .args(["--user", "restart", &target])
                    .status();
                eprintln!("  restarted {}", target);
            }
        }
        "status" => {
            for (name, _, timer) in &units {
                let target = if timer.is_some() {
                    format!("{}.timer", name)
                } else {
                    format!("{}.service", name)
                };
                let output = std::process::Command::new("systemctl")
                    .args(["--user", "is-active", &target])
                    .output();
                let state = match output {
                    Ok(o) => String::from_utf8_lossy(&o.stdout).trim().to_string(),
                    Err(_) => "unknown".to_string(),
                };
                eprintln!("  {}  {}", state, target);
            }
        }
        _ => anyhow::bail!(
            "Unknown action: {}. Use install, uninstall, restart, or status.",
            action
        ),
    }
    Ok(())
}

fn cmd_logs(errors: bool, lines: usize) -> Result<()> {
    let log_path = Config::minutes_dir().join("logs").join("minutes.log");
    if !log_path.exists() {
        eprintln!("No log file found at {}", log_path.display());
        return Ok(());
    }

    let content = std::fs::read_to_string(&log_path)?;
    let all_lines: Vec<&str> = content.lines().collect();

    let filtered: Vec<&&str> = if errors {
        all_lines
            .iter()
            .filter(|line| line.contains("\"level\":\"error\"") || line.contains("ERROR"))
            .collect()
    } else {
        all_lines.iter().collect()
    };

    let start = if filtered.len() > lines {
        filtered.len() - lines
    } else {
        0
    };

    for line in &filtered[start..] {
        println!("{}", line);
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use minutes_core::autoresearch::{
        DecodeHintEvalCaseResult, DecodeHintEvalHintDebug, DecodeHintEvalOptions,
        DecodeHintEvalReport, DecodeHintEvalTotals, DecodeHintEvalTranscriptMetrics,
    };
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

    #[cfg(feature = "parakeet")]
    #[derive(Serialize)]
    struct DummyTranscript {
        transcript: String,
        segments: Vec<String>,
    }

    fn test_guard() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn with_temp_home<T>(f: impl FnOnce(&Path) -> T) -> T {
        let _guard = test_guard();
        let dir = std::env::temp_dir().join(format!(
            "minutes-cli-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let original_home = std::env::var_os("HOME");
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", &dir);
        std::env::set_var("USERPROFILE", &dir);
        let result = f(&dir);
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(userprofile) = original_userprofile {
            std::env::set_var("USERPROFILE", userprofile);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        std::fs::remove_dir_all(&dir).ok();
        result
    }

    fn attr(
        label: &str,
        name: &str,
        confidence: minutes_core::diarize::Confidence,
    ) -> minutes_core::diarize::SpeakerAttribution {
        minutes_core::diarize::SpeakerAttribution {
            speaker_label: label.into(),
            name: name.into(),
            confidence,
            source: minutes_core::diarize::AttributionSource::Llm,
        }
    }

    #[test]
    fn shell_quote_arg_leaves_slugs_bare_and_quotes_spaces() {
        assert_eq!(shell_quote_arg("jun-rei"), "jun-rei");
        assert_eq!(shell_quote_arg("junrei_v2"), "junrei_v2");
        assert_eq!(shell_quote_arg("Jun Rei"), "\"Jun Rei\"");
        assert_eq!(shell_quote_arg("a\"b"), "\"a\\\"b\"");
    }

    #[test]
    fn format_merge_command_uses_first_slug_as_canonical() {
        let slugs = vec![
            "junrei".to_string(),
            "junlei".to_string(),
            "jun-rei".to_string(),
        ];
        assert_eq!(
            format_merge_command(&slugs),
            "minutes people merge junrei junlei jun-rei"
        );
        // A name with a space gets quoted.
        let slugs = vec!["jun rei".to_string(), "junlei".to_string()];
        assert_eq!(
            format_merge_command(&slugs),
            "minutes people merge \"jun rei\" junlei"
        );
    }

    #[test]
    fn resolve_person_token_matches_slug_then_name_else_literal() {
        let people = vec![minutes_core::graph::PersonSummary {
            slug: "jun-rei".into(),
            name: "Jun Rei".into(),
            meeting_count: 3,
            last_seen: String::new(),
            days_since: 0.0,
            open_commitments: 0,
            top_topics: vec![],
            score: 0.0,
            losing_touch: false,
        }];
        // slug match (case-insensitive) -> display name, resolved
        assert_eq!(
            resolve_person_token("JUN-REI", &people),
            ("Jun Rei".into(), true)
        );
        // exact name match -> resolved
        assert_eq!(
            resolve_person_token("jun rei", &people),
            ("Jun Rei".into(), true)
        );
        // no match -> literal, unresolved
        assert_eq!(
            resolve_person_token("Bobby", &people),
            ("Bobby".into(), false)
        );
    }

    #[test]
    fn merge_speaker_map_never_downgrades_high() {
        use minutes_core::diarize::Confidence;
        let existing = vec![attr("SPEAKER_00", "Sarah", Confidence::High)];
        let fresh = vec![
            attr("SPEAKER_00", "Someone Else", Confidence::Medium),
            attr("SPEAKER_01", "Dan", Confidence::Medium),
        ];
        let merged = merge_speaker_map(&existing, &fresh);
        let s0 = merged
            .iter()
            .find(|a| a.speaker_label == "SPEAKER_00")
            .unwrap();
        assert_eq!(s0.name, "Sarah", "existing High must survive");
        assert_eq!(s0.confidence, Confidence::High);
        let s1 = merged
            .iter()
            .find(|a| a.speaker_label == "SPEAKER_01")
            .unwrap();
        assert_eq!(s1.name, "Dan", "fresh fills in unmapped labels");
    }

    #[test]
    fn merge_speaker_map_preserves_high_on_duplicate_existing_labels() {
        use minutes_core::diarize::Confidence;
        // Malformed frontmatter: same label listed twice, High then Medium.
        let existing = vec![
            attr("SPEAKER_00", "Sarah", Confidence::High),
            attr("SPEAKER_00", "sarah lower", Confidence::Medium),
        ];
        // Even with an empty fresh map, the High entry must not be lost.
        let merged = merge_speaker_map(&existing, &[]);
        let s0 = merged
            .iter()
            .find(|a| a.speaker_label == "SPEAKER_00")
            .unwrap();
        assert_eq!(s0.name, "Sarah");
        assert_eq!(s0.confidence, Confidence::High);
        assert_eq!(merged.len(), 1, "duplicate labels collapse to one");
    }

    #[test]
    fn transcript_section_requires_exact_heading() {
        // A look-alike heading must not be treated as the transcript.
        let body = "## Transcript cleanup notes\n\n[SPEAKER_00 0:00] not the transcript\n";
        assert!(transcript_section(body).is_none());
    }

    #[test]
    fn transcript_section_ignores_fenced_code_block() {
        // A `## Transcript` line inside a code fence must be ignored; the real
        // section after the fence is the one that wins.
        let body = "## Summary\n\n```\n## Transcript\n[SPEAKER_99 0:00] fake\n```\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = transcript_section(body).expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn transcript_section_handles_mixed_fence_markers() {
        // A backtick fence whose body contains a `~~~` line must NOT be treated
        // as closed by that tilde line; the fake `## Transcript` inside stays
        // ignored and the real section after the fence wins.
        let body = "## Summary\n\n```\n~~~\n## Transcript\n[SPEAKER_99 0:00] fake\n```\n\n## Transcript\n\n[SPEAKER_00 0:00] real line\n";
        let t = transcript_section(body).expect("real transcript section found");
        assert!(t.contains("real line"));
        assert!(!t.contains("fake"));
    }

    #[test]
    fn transcript_section_extracts_first_and_stops_at_next_h2() {
        let body = "## Transcript\n\n[SPEAKER_00 0:00] hello\n\n## Action Items\n\n- do thing\n";
        let t = transcript_section(body).unwrap();
        assert!(t.contains("hello"));
        assert!(!t.contains("Action Items"));
    }

    #[test]
    fn recording_consent_explicit_basis_still_reminds_in_remind_mode() {
        let config = Config::default();

        let resolved = prepare_recording_consent(
            &config,
            Some("verbal_all_parties"),
            Some("Read aloud."),
            false,
            || panic!("prompt should not run for explicit basis"),
        )
        .unwrap();

        assert_eq!(resolved.basis, ConsentBasis::VerbalAllParties);
        assert_eq!(resolved.notice.as_deref(), Some("Read aloud."));
        assert_eq!(
            resolved.reminder.as_deref(),
            Some(config.consent.disclosure_script.as_str())
        );
        assert_eq!(resolved.warning, None);
    }

    #[test]
    fn recording_consent_does_not_fabricate_notice_from_disclosure_script() {
        let config = Config::default();

        let resolved = prepare_recording_consent(&config, Some("na"), None, false, || {
            panic!("prompt should not run for explicit basis")
        })
        .unwrap();

        assert_eq!(resolved.basis, ConsentBasis::NotApplicable);
        assert_eq!(resolved.notice, None);
        assert!(resolved.reminder.is_some());
        assert_eq!(resolved.warning, None);
    }

    #[test]
    fn recording_consent_require_non_tty_does_not_prompt() {
        let mut config = Config::default();
        config.consent.mode = ConsentMode::Require;
        let prompt_calls = AtomicUsize::new(0);

        let resolved = prepare_recording_consent(&config, None, None, false, || {
            prompt_calls.fetch_add(1, Ordering::SeqCst);
            Ok(true)
        })
        .unwrap();

        assert_eq!(resolved.basis, ConsentBasis::Unattested);
        assert!(resolved.warning.is_some());
        assert!(resolved.reminder.is_some());
        assert_eq!(prompt_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recording_consent_rejects_unknown_basis() {
        let err =
            parse_recording_consent_basis("made_up_basis").expect_err("unknown basis should error");

        assert!(err.to_string().contains("unknown consent basis"));
    }

    #[test]
    fn recording_consent_sidecar_round_trip_from_cli_resolution() {
        with_temp_home(|_| {
            let config = Config::default();
            let resolved = prepare_recording_consent(
                &config,
                Some("notice_in_invite"),
                Some("Included in the invite."),
                false,
                || panic!("prompt should not run for explicit basis"),
            )
            .unwrap();

            minutes_core::notes::save_consent(Some(resolved.basis), resolved.notice.as_deref())
                .unwrap();
            let (basis, notice) = minutes_core::notes::load_consent();

            assert_eq!(basis, Some(ConsentBasis::NoticeInInvite));
            assert_eq!(notice.as_deref(), Some("Included in the invite."));
        });
    }

    #[test]
    fn sensitive_cli_start_stop_non_tty_marks_debrief_pending() {
        with_temp_home(|home| {
            let config = Config {
                output_dir: home.join("meetings"),
                ..Config::default()
            };
            cmd_sensitive(
                SensitiveAction::Start {
                    title: Some("Board sync".into()),
                },
                &config,
            )
            .unwrap();
            cmd_sensitive(SensitiveAction::Stop, &config).unwrap();

            let meeting = std::fs::read_dir(&config.output_dir)
                .unwrap()
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("md"))
                .expect("sensitive meeting markdown");
            let content = std::fs::read_to_string(meeting).unwrap();
            assert!(content.contains("capture: none"));
            assert!(content.contains("sensitivity: restricted"));
            assert!(content.contains("debrief: pending"));
        });
    }

    fn sample_decode_hint_eval_report_with_allowed_failures() -> DecodeHintEvalReport {
        DecodeHintEvalReport {
            generated_at: "2026-04-23T12:00:00Z".into(),
            corpus_path: PathBuf::from("/tmp/corpus.json"),
            options: DecodeHintEvalOptions::default(),
            totals: DecodeHintEvalTotals {
                cases_total: 2,
                cases_passed: 2,
                cases_failed: 0,
                cases_allowed_failures: 1,
                improved_cases: 1,
                regressed_cases: 0,
                average_delta_wer: -0.01,
            },
            cases: vec![
                DecodeHintEvalCaseResult {
                    id: "self-intro-whisper".into(),
                    engine: "whisper".into(),
                    hint_debug: DecodeHintEvalHintDebug::default(),
                    baseline: DecodeHintEvalTranscriptMetrics {
                        wer: 0.12,
                        focus_hits: vec!["mat".into()],
                        forbidden_hits: vec![],
                        text: String::new(),
                    },
                    candidate: DecodeHintEvalTranscriptMetrics {
                        wer: 0.09,
                        focus_hits: vec!["mat".into(), "leadernet".into()],
                        forbidden_hits: vec![],
                        text: String::new(),
                    },
                    delta_wer: -0.03,
                    max_wer_regression: Some(0.03),
                    required_terms: vec!["mat".into(), "leadernet".into()],
                    forbidden_terms: vec![],
                    passed: true,
                    status: "pass".into(),
                    failure_reasons: vec![],
                    allowed_failure_reasons: vec![],
                },
                DecodeHintEvalCaseResult {
                    id: "external-proper-noun-research".into(),
                    engine: "parakeet".into(),
                    hint_debug: DecodeHintEvalHintDebug::default(),
                    baseline: DecodeHintEvalTranscriptMetrics {
                        wer: 0.10,
                        focus_hits: vec!["pdf toolkit".into()],
                        forbidden_hits: vec![],
                        text: String::new(),
                    },
                    candidate: DecodeHintEvalTranscriptMetrics {
                        wer: 0.10,
                        focus_hits: vec!["pdf toolkit".into()],
                        forbidden_hits: vec![],
                        text: String::new(),
                    },
                    delta_wer: 0.0,
                    max_wer_regression: Some(0.02),
                    required_terms: vec!["casey rowan".into()],
                    forbidden_terms: vec![],
                    passed: true,
                    status: "allowed-failure".into(),
                    failure_reasons: vec![],
                    allowed_failure_reasons: vec![
                        "missing required hinted term 'casey rowan'".into()
                    ],
                },
            ],
            failure_messages: vec![],
        }
    }

    fn sample_decode_hint_eval_report_with_failures() -> DecodeHintEvalReport {
        DecodeHintEvalReport {
            generated_at: "2026-04-23T12:00:00Z".into(),
            corpus_path: PathBuf::from("/tmp/corpus.json"),
            options: DecodeHintEvalOptions::default(),
            totals: DecodeHintEvalTotals {
                cases_total: 1,
                cases_passed: 0,
                cases_failed: 1,
                cases_allowed_failures: 0,
                improved_cases: 0,
                regressed_cases: 1,
                average_delta_wer: 0.03,
            },
            cases: vec![DecodeHintEvalCaseResult {
                id: "case-1".into(),
                engine: "parakeet".into(),
                hint_debug: DecodeHintEvalHintDebug::default(),
                baseline: DecodeHintEvalTranscriptMetrics {
                    wer: 0.12,
                    focus_hits: vec![],
                    forbidden_hits: vec![],
                    text: String::new(),
                },
                candidate: DecodeHintEvalTranscriptMetrics {
                    wer: 0.15,
                    focus_hits: vec![],
                    forbidden_hits: vec!["matt mullenweg".into()],
                    text: String::new(),
                },
                delta_wer: 0.03,
                max_wer_regression: Some(0.02),
                required_terms: vec!["alex chen".into()],
                forbidden_terms: vec!["matt mullenweg".into()],
                passed: false,
                status: "fail".into(),
                failure_reasons: vec!["contains forbidden hinted term 'matt mullenweg'".into()],
                allowed_failure_reasons: vec![],
            }],
            failure_messages: vec!["case-1 contains forbidden hinted term 'matt mullenweg'".into()],
        }
    }

    #[test]
    fn parse_qmd_collection_names_extracts_collection_headers() {
        let output = r#"Collections (2):

minutes (qmd://minutes/)
  Pattern:  **/*.md
  Files:    12
  Updated:  1h ago

life (qmd://life/)
  Pattern:  **/*.md
  Files:    100
  Updated:  2d ago
"#;

        let collections = parse_qmd_collection_names(output);
        assert_eq!(collections, vec!["minutes".to_string(), "life".to_string()]);
    }

    #[test]
    fn json_envelope_includes_schema_metadata() {
        let envelope = json_envelope("minutes health", json!({ "engine": "parakeet" }));
        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["command"], "minutes health");
        assert_eq!(value["meta"]["schemaVersion"], 1);
        assert_eq!(value["data"]["engine"], "parakeet");
        assert!(value["meta"]["generatedAt"].is_string());
    }

    #[test]
    #[cfg(feature = "parakeet")]
    fn parakeet_helper_envelope_flattens_transcript_fields() {
        let envelope = parakeet_helper_envelope(
            "minutes parakeet-helper",
            DummyTranscript {
                transcript: "[0:00] hello".into(),
                segments: vec!["hello".into()],
            },
        );
        let value = serde_json::to_value(envelope).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["command"], "minutes parakeet-helper");
        assert_eq!(value["transcript"], "[0:00] hello");
        assert_eq!(value["segments"][0], "hello");
        assert_eq!(value["meta"]["schemaVersion"], 1);
    }

    /// Regression guard for issue #163: the helper subcommand must accept
    /// `--fp16` when forwarded by `transcribe::transcribe_with_parakeet`,
    /// AND must continue to parse without it for manual invocations and for
    /// the `use_fp16=false` programmatic path. Pre-fix, clap rejected the
    /// flag on every utterance and silently fell back to spawning parakeet
    /// directly, ending in a confusing error on Ctrl+C and a session-level
    /// fallback to whisper.
    #[test]
    fn parakeet_helper_clap_accepts_fp16_flag_present_or_absent() {
        let common = [
            "minutes",
            "parakeet-helper",
            "--binary",
            "/usr/local/bin/parakeet",
            "--model-path",
            "/tmp/model.bin",
            "--audio-path",
            "/tmp/audio.wav",
            "--vocab-path",
            "/tmp/vocab.txt",
            "--model-id",
            "tdt-600m",
        ];

        // Without --fp16: must parse, fp16 must be false.
        let parsed_without =
            Cli::try_parse_from(common).expect("parakeet-helper without --fp16 must parse");
        match parsed_without.command {
            Commands::ParakeetHelper { fp16, .. } => assert!(!fp16),
            _ => panic!("expected ParakeetHelper variant"),
        }

        // With --fp16: must parse, fp16 must be true.
        let mut with_fp16: Vec<&str> = common.to_vec();
        with_fp16.push("--fp16");
        let parsed_with =
            Cli::try_parse_from(with_fp16).expect("parakeet-helper --fp16 must parse");
        match parsed_with.command {
            Commands::ParakeetHelper { fp16, .. } => assert!(fp16),
            _ => panic!("expected ParakeetHelper variant"),
        }
    }

    #[test]
    fn import_accepts_audio_path_for_recovery_alias() {
        let parsed = Cli::try_parse_from([
            "minutes",
            "import",
            "/Users/test/.minutes/native-captures/2026-05-19-120148-call.voice.wav",
        ])
        .expect("import must accept audio paths so it can route to process");

        match parsed.command {
            Commands::Import { from, dir, dry_run } => {
                assert_eq!(
                    from,
                    "/Users/test/.minutes/native-captures/2026-05-19-120148-call.voice.wav"
                );
                assert!(dir.is_none());
                assert!(!dry_run);
            }
            _ => panic!("expected Import variant"),
        }
    }

    #[test]
    fn looks_like_audio_path_matches_supported_process_formats() {
        assert!(looks_like_audio_path("call.voice.wav"));
        assert!(looks_like_audio_path("meeting.MOV"));
        assert!(looks_like_audio_path("/tmp/memo.m4a"));
        assert!(!looks_like_audio_path("granola"));
        assert!(!looks_like_audio_path("notes.md"));
    }

    #[test]
    fn render_decode_hints_plaintext_summary_surfaces_allowed_failures() {
        let output = render_decode_hints_plaintext_summary(
            &sample_decode_hint_eval_report_with_allowed_failures(),
            Path::new("/tmp/decode-hints/2026-04-23T12-00-00Z"),
            false,
        );

        assert!(output.contains("Decode hint eval: PASS WITH ALLOWED FAILURES"));
        assert!(output.contains("Allowed failures: 1"));
        assert!(output.contains("Allowed-failure cases:"));
        assert!(output.contains("external-proper-noun-research"));
        assert!(output.contains("missing required hinted term 'casey rowan'"));
    }

    #[test]
    fn render_decode_hints_plaintext_summary_surfaces_blocking_failures() {
        let output = render_decode_hints_plaintext_summary(
            &sample_decode_hint_eval_report_with_failures(),
            Path::new("/tmp/decode-hints/2026-04-23T12-00-00Z"),
            true,
        );

        assert!(output.contains("Decode hint eval: FAIL"));
        assert!(output.contains("Allowed failures: 0"));
        assert!(output.contains("Failure messages:"));
        assert!(output.contains("case-1 contains forbidden hinted term 'matt mullenweg'"));
        assert!(!output.contains("Allowed-failure cases:"));
    }

    #[test]
    fn parse_qmd_collection_path_reads_show_output() {
        let output = r#"Collection: minutes
  Path:     /Users/you/meetings
  Pattern:  **/*.md
  Include:  yes (default)
"#;

        assert_eq!(
            parse_qmd_collection_path(output),
            Some(PathBuf::from("/Users/you/meetings"))
        );
    }

    #[test]
    fn cleanup_live_capture_state_clears_pid_metadata_and_notes() {
        with_temp_home(|_| {
            minutes_core::pid::create().unwrap();
            minutes_core::pid::write_recording_metadata(CaptureMode::Meeting).unwrap();
            minutes_core::notes::save_context("pricing review").unwrap();
            minutes_core::notes::add_note("remember to ask about budget").unwrap();

            cleanup_live_capture_state();

            assert!(!minutes_core::pid::pid_path().exists());
            assert!(!minutes_core::pid::recording_meta_path().exists());
            assert!(!minutes_core::notes::recording_start_path().exists());
            assert!(minutes_core::notes::read_context().is_none());
            assert!(minutes_core::notes::read_notes().is_none());
        });
    }

    #[test]
    fn resolve_recording_device_overrides_uses_single_cli_source() {
        let mut config = Config::default();
        resolve_recording_device_overrides(&mut config, &[String::from("Yeti Nano")], None, false)
            .expect("single source should map to recording.device");
        assert_eq!(config.recording.device.as_deref(), Some("Yeti Nano"));
        assert!(config.recording.sources.is_none());
    }

    #[test]
    fn resolve_recording_device_overrides_maps_dual_cli_sources() {
        let mut config = Config::default();
        resolve_recording_device_overrides(
            &mut config,
            &[String::from("Mic"), String::from("BlackHole 2ch")],
            None,
            false,
        )
        .expect("dual CLI sources should map to recording.sources");
        let sources = config
            .recording
            .sources
            .expect("dual sources should remain configured");
        assert_eq!(sources.voice.as_deref(), Some("Mic"));
        assert_eq!(sources.call.as_deref(), Some("BlackHole 2ch"));
        assert!(config.recording.device.is_none());
    }

    #[test]
    fn resolve_recording_device_overrides_preserves_dual_config_sources() {
        let mut config = Config::default();
        config.recording.sources = Some(minutes_core::config::SourcesConfig {
            voice: Some("Mic".into()),
            call: Some("BlackHole 2ch".into()),
        });

        resolve_recording_device_overrides(&mut config, &[], None, false)
            .expect("dual config sources should remain intact");
        let sources = config
            .recording
            .sources
            .expect("dual config should remain configured");
        assert_eq!(sources.voice.as_deref(), Some("Mic"));
        assert_eq!(sources.call.as_deref(), Some("BlackHole 2ch"));
    }

    #[test]
    fn resolve_recording_device_overrides_allows_explicit_device_to_win() {
        let mut config = Config::default();
        config.recording.sources = Some(minutes_core::config::SourcesConfig {
            voice: Some("Mic".into()),
            call: Some("BlackHole 2ch".into()),
        });

        resolve_recording_device_overrides(&mut config, &[], Some("USB Mic".into()), false)
            .expect("explicit --device should override config sources");
        assert_eq!(config.recording.device.as_deref(), Some("USB Mic"));
        assert!(config.recording.sources.is_none());
    }

    #[test]
    fn resolve_recording_device_overrides_normalizes_decorated_explicit_device() {
        let mut config = Config::default();
        resolve_recording_device_overrides(
            &mut config,
            &[],
            Some("Ground Control (16000Hz, 1 ch)".into()),
            false,
        )
        .expect("decorated --device value should normalize");
        assert_eq!(config.recording.device.as_deref(), Some("Ground Control"));
    }

    #[test]
    fn resolve_recording_device_overrides_uses_single_voice_config_source() {
        let mut config = Config::default();
        config.recording.sources = Some(minutes_core::config::SourcesConfig {
            voice: Some("Built-in Microphone".into()),
            call: None,
        });

        resolve_recording_device_overrides(&mut config, &[], None, false)
            .expect("single voice source should map to recording.device");
        assert_eq!(
            config.recording.device.as_deref(),
            Some("Built-in Microphone")
        );
        assert!(config.recording.sources.is_none());
    }

    #[test]
    fn cmd_delete_archives_meeting_to_archive_dir() {
        with_temp_home(|dir| {
            let meetings = dir.join("meetings");
            std::fs::create_dir_all(&meetings).unwrap();
            let md = meetings.join("2026-04-01-test.md");
            std::fs::write(&md, "---\ntitle: Test\n---\nContent").unwrap();
            let wav = meetings.join("2026-04-01-test.wav");
            std::fs::write(&wav, b"fake audio").unwrap();

            let config = Config {
                output_dir: meetings.clone(),
                ..Config::default()
            };

            // Archive (soft delete)
            cmd_delete("2026-04-01-test", false, false, &config).unwrap();
            assert!(!md.exists(), "md should be moved");
            assert!(
                meetings.join("archive/2026-04-01-test.md").exists(),
                "md should be in archive"
            );
            assert!(wav.exists(), "wav should remain without --with-audio");
        });
    }

    #[test]
    fn cmd_delete_archives_all_audio_artifacts_with_with_audio() {
        with_temp_home(|dir| {
            let meetings = dir.join("meetings");
            std::fs::create_dir_all(&meetings).unwrap();
            let md = meetings.join("2026-04-01-artifacts.md");
            std::fs::write(&md, "---\ntitle: Artifacts\n---\nContent").unwrap();
            let wav = meetings.join("2026-04-01-artifacts.wav");
            std::fs::write(&wav, b"fake audio").unwrap();
            let voice = meetings.join("2026-04-01-artifacts.voice.wav");
            std::fs::write(&voice, b"fake voice stem").unwrap();
            let system = meetings.join("2026-04-01-artifacts.system.wav");
            std::fs::write(&system, b"fake system stem").unwrap();
            let embeddings = meetings.join(".2026-04-01-artifacts.embeddings");
            std::fs::write(&embeddings, b"{\"Speaker 1\":[0.1,0.2]}").unwrap();

            let config = Config {
                output_dir: meetings.clone(),
                ..Config::default()
            };

            cmd_delete("2026-04-01-artifacts", true, false, &config).unwrap();
            assert!(!md.exists(), "md should be moved");
            assert!(
                meetings.join("archive/2026-04-01-artifacts.md").exists(),
                "md should be in archive"
            );
            assert!(!wav.exists(), "merged wav should be moved");
            assert!(!voice.exists(), "voice stem should be moved");
            assert!(!system.exists(), "system stem should be moved");
            assert!(!embeddings.exists(), "embeddings sidecar should be moved");
            assert!(
                meetings.join("archive/2026-04-01-artifacts.wav").exists(),
                "merged wav should be archived"
            );
            assert!(
                meetings
                    .join("archive/2026-04-01-artifacts.voice.wav")
                    .exists(),
                "voice stem should be archived"
            );
            assert!(
                meetings
                    .join("archive/2026-04-01-artifacts.system.wav")
                    .exists(),
                "system stem should be archived"
            );
            assert!(
                meetings
                    .join("archive/.2026-04-01-artifacts.embeddings")
                    .exists(),
                "embeddings sidecar should be archived"
            );
        });
    }

    #[test]
    fn cmd_delete_force_permanently_removes() {
        with_temp_home(|dir| {
            let meetings = dir.join("meetings");
            std::fs::create_dir_all(&meetings).unwrap();
            let md = meetings.join("2026-04-01-force.md");
            std::fs::write(&md, "---\ntitle: Force\n---\nContent").unwrap();
            let wav = meetings.join("2026-04-01-force.wav");
            std::fs::write(&wav, b"fake audio").unwrap();
            let voice = meetings.join("2026-04-01-force.voice.wav");
            std::fs::write(&voice, b"fake voice stem").unwrap();
            let system = meetings.join("2026-04-01-force.system.wav");
            std::fs::write(&system, b"fake system stem").unwrap();
            let embeddings = meetings.join(".2026-04-01-force.embeddings");
            std::fs::write(&embeddings, b"{\"Speaker 1\":[0.1,0.2]}").unwrap();

            let config = Config {
                output_dir: meetings.clone(),
                ..Config::default()
            };

            cmd_delete("2026-04-01-force", true, true, &config).unwrap();
            assert!(!md.exists(), "md should be gone");
            assert!(
                !wav.exists(),
                "wav should be gone with --with-audio --force"
            );
            assert!(!voice.exists(), "voice stem should be gone");
            assert!(!system.exists(), "system stem should be gone");
            assert!(!embeddings.exists(), "embeddings sidecar should be gone");
            assert!(
                !meetings.join("archive/2026-04-01-force.md").exists(),
                "nothing in archive for force delete"
            );
        });
    }

    #[test]
    fn parse_retention_days_accepts_day_suffixes() {
        assert_eq!(parse_retention_days("30").unwrap(), 30);
        assert_eq!(parse_retention_days("14d").unwrap(), 14);
        assert_eq!(parse_retention_days("7 days").unwrap(), 7);
    }

    #[test]
    fn cmd_cleanup_apply_removes_only_expired_audio_candidates() {
        with_temp_home(|dir| {
            let meetings = dir.join("meetings");
            std::fs::create_dir_all(&meetings).unwrap();
            let old_md = meetings.join("old.md");
            std::fs::write(
                &old_md,
                "---\ntitle: Old\ntype: meeting\ndate: 2026-04-01T09:00:00-07:00\nduration: 5m\n---\n\nOld",
            )
            .unwrap();
            let old_wav = meetings.join("old.wav");
            std::fs::write(&old_wav, b"old audio").unwrap();

            let pinned_md = meetings.join("pinned.md");
            std::fs::write(
                &pinned_md,
                "---\ntitle: Pinned\ntype: meeting\ndate: 2026-04-01T09:00:00-07:00\nduration: 5m\naudio_retention: pinned\n---\n\nPinned",
            )
            .unwrap();
            let pinned_wav = meetings.join("pinned.wav");
            std::fs::write(&pinned_wav, b"pinned audio").unwrap();

            let config = Config {
                output_dir: meetings.clone(),
                ..Config::default()
            };

            cmd_cleanup(true, Some("0d"), true, &config).unwrap();

            assert!(old_md.exists(), "cleanup must not delete markdown");
            assert!(
                !old_wav.exists(),
                "expired unpinned audio should be removed"
            );
            assert!(pinned_md.exists(), "pinned markdown remains");
            assert!(pinned_wav.exists(), "pinned audio should be kept");
        });
    }

    #[test]
    fn setup_demo_installs_five_mcp_fixture_meetings_idempotently() {
        with_temp_home(|_| {
            let demo_dir = Config::minutes_dir().join("demo");

            let first = demo_data::install_mcp_demo_fixtures(&demo_dir).unwrap();
            assert_eq!(first.total_fixtures, 5);
            assert_eq!(first.updated_fixtures, 5);

            let files = std::fs::read_dir(&demo_dir)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("md"))
                .count();
            assert_eq!(files, 5);

            let one_fixture =
                std::fs::read_to_string(demo_dir.join("2026-02-28-pricing-strategy.md")).unwrap();
            assert!(one_fixture.contains("minutes_demo: true"));

            let second = demo_data::install_mcp_demo_fixtures(&demo_dir).unwrap();
            assert_eq!(second.total_fixtures, 5);
            assert_eq!(second.updated_fixtures, 0);
        });
    }

    #[test]
    fn graceful_interrupt_requests_shutdown_before_force_exit() {
        let stop = AtomicBool::new(false);
        let shutdowns = AtomicUsize::new(0);

        let first = handle_graceful_interrupt_with_shutdown(&stop, "Stopping...", || {
            shutdowns.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(first, InterruptAction::Continue);
        assert!(stop.load(Ordering::Relaxed));
        assert_eq!(shutdowns.load(Ordering::Relaxed), 1);

        let second = handle_graceful_interrupt_with_shutdown(&stop, "Stopping...", || {
            shutdowns.fetch_add(1, Ordering::Relaxed);
        });
        assert_eq!(second, InterruptAction::ForceExit(1));
        assert_eq!(shutdowns.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn transcribe_subcommand_parses_minimal_args() {
        let parsed = Cli::try_parse_from(["minutes", "transcribe", "/tmp/audio.wav"])
            .expect("transcribe must parse with only a file path");
        match parsed.command {
            Commands::Transcribe {
                path,
                json,
                language,
                diarize,
            } => {
                assert_eq!(path, PathBuf::from("/tmp/audio.wav"));
                assert!(!json);
                assert!(language.is_none());
                assert!(!diarize);
            }
            _ => panic!("expected Transcribe variant"),
        }
    }

    #[test]
    fn copilot_start_parses_goal_and_surface() {
        let parsed = Cli::try_parse_from([
            "minutes",
            "copilot",
            "start",
            "--goal",
            "land the decision",
            "--surface",
            "stdout",
            "--mode",
            "decision",
        ])
        .expect("copilot start must parse its portable surface");
        match parsed.command {
            Commands::Copilot {
                action:
                    CopilotAction::Start {
                        goal,
                        surface,
                        mode,
                        live,
                    },
            } => {
                assert_eq!(goal, "land the decision");
                assert_eq!(surface.as_deref(), Some("stdout"));
                assert_eq!(mode.as_deref(), Some("decision"));
                assert!(!live);
            }
            _ => panic!("expected Copilot Start variant"),
        }
    }

    #[test]
    fn copilot_start_requires_goal() {
        assert!(Cli::try_parse_from(["minutes", "copilot", "start"]).is_err());
    }

    #[test]
    fn copilot_feedback_parses_explicit_rating() {
        let parsed = Cli::try_parse_from([
            "minutes",
            "copilot",
            "feedback",
            "--nudge-id",
            "nudge-7-2",
            "--rating",
            "not-helpful",
        ])
        .expect("copilot feedback flags must parse");
        match parsed.command {
            Commands::Copilot {
                action: CopilotAction::Feedback { nudge_id, rating },
            } => {
                assert_eq!(nudge_id, "nudge-7-2");
                assert_eq!(rating, "not-helpful");
            }
            _ => panic!("expected Copilot Feedback variant"),
        }
    }

    #[test]
    fn copilot_eval_parses_deterministic_suite_options() {
        let parsed = Cli::try_parse_from([
            "minutes",
            "copilot",
            "eval",
            "--fixtures",
            "/tmp/copilot-fixtures",
            "--model",
            "qwen3.5:4b-mlx",
            "--accelerated",
            "--json",
        ])
        .expect("copilot eval flags must parse");
        match parsed.command {
            Commands::Copilot {
                action:
                    CopilotAction::Eval {
                        fixtures,
                        model,
                        accelerated,
                        json,
                    },
            } => {
                assert_eq!(fixtures, Some(PathBuf::from("/tmp/copilot-fixtures")));
                assert_eq!(model.as_deref(), Some("qwen3.5:4b-mlx"));
                assert!(accelerated);
                assert!(json);
            }
            _ => panic!("expected Copilot Eval variant"),
        }
    }

    #[test]
    fn coach_model_eval_scoring_helpers_are_deterministic() {
        use minutes_core::copilot::eval::OpportunityLabel;
        use minutes_core::copilot::{NudgeDraft, NudgeKind, OpportunityKind};

        assert_eq!(model_eval_percentile(&[40, 10, 30, 20], 0.50), Some(20));
        assert_eq!(model_eval_percentile(&[40, 10, 30, 20], 0.95), Some(40));
        assert_eq!(model_eval_percentile(&[], 0.50), None);
        assert_eq!(model_eval_rate(0, 0).rate, 1.0);

        let labels = vec![OpportunityLabel {
            id: "pricing-owner".into(),
            start_ms: 500,
            end_ms: 2_000,
            kind: Some(NudgeKind::Ask),
            match_any: vec!["finance approver".into(), "pricing owner".into()],
        }];
        let draft = NudgeDraft {
            kind: NudgeKind::Ask,
            text: "Who is the finance approver for this offer?".into(),
            source_chip: "transcript".into(),
            opportunity: OpportunityKind::Decision,
            confidence: 92,
        };
        assert_eq!(
            match_model_eval_opportunity(&labels, 1_200, &draft),
            Some("pricing-owner")
        );
        assert_eq!(match_model_eval_opportunity(&labels, 2_500, &draft), None);
    }

    #[test]
    fn copilot_external_attach_and_non_streaming_backends_are_final_only() {
        let mut config = Config::default();
        assert_eq!(
            copilot_evidence_mode(true, true, &config),
            minutes_core::copilot::CopilotEvidenceMode::FinalOnly
        );

        config.live_transcript.backend = "parakeet".into();
        assert_eq!(
            copilot_evidence_mode(true, false, &config),
            minutes_core::copilot::CopilotEvidenceMode::FinalOnly
        );

        config.live_transcript.backend = "whisper".into();
        assert_eq!(
            copilot_evidence_mode(true, false, &config),
            minutes_core::copilot::CopilotEvidenceMode::InProcessPartials
        );
    }

    #[test]
    fn coach_alias_exposes_one_command_setup() {
        let parsed = Cli::try_parse_from(["minutes", "coach", "setup"])
            .expect("the plain-language Coach setup command must parse");
        assert!(matches!(
            parsed.command,
            Commands::Copilot {
                action: CopilotAction::Setup {
                    model: None,
                    retune: false
                }
            }
        ));
    }

    #[test]
    fn copilot_alias_still_exposes_setup() {
        let parsed = Cli::try_parse_from(["minutes", "copilot", "setup"])
            .expect("the existing Copilot setup command must keep parsing");
        assert!(matches!(
            parsed.command,
            Commands::Copilot {
                action: CopilotAction::Setup {
                    model: None,
                    retune: false
                }
            }
        ));
    }

    #[test]
    fn coach_setup_parses_model_and_retune_controls() {
        let forced = Cli::try_parse_from([
            "minutes",
            "coach",
            "setup",
            "--model",
            "custom/coach:latest",
        ])
        .unwrap();
        assert!(matches!(
            forced.command,
            Commands::Copilot {
                action: CopilotAction::Setup {
                    model: Some(ref model),
                    retune: false
                }
            } if model == "custom/coach:latest"
        ));

        let retune = Cli::try_parse_from(["minutes", "coach", "setup", "--retune"]).unwrap();
        assert!(matches!(
            retune.command,
            Commands::Copilot {
                action: CopilotAction::Setup {
                    model: None,
                    retune: true
                }
            }
        ));
        assert!(Cli::try_parse_from([
            "minutes",
            "coach",
            "setup",
            "--retune",
            "--model",
            "qwen3.5:4b"
        ])
        .is_err());
    }

    #[test]
    fn coach_setup_override_detection_preserves_only_explicit_custom_models() {
        let mut config = Config::default();
        assert_eq!(
            configured_copilot_model_override(&config, None, false),
            None
        );

        config.copilot.fast_model = "llama3.2:latest".into();
        assert_eq!(
            configured_copilot_model_override(&config, None, false),
            None
        );

        config.copilot.fast_model = "gemma4:26b-mlx".into();
        assert_eq!(
            configured_copilot_model_override(&config, None, false),
            None
        );

        config.copilot.fast_model = "custom/coach:latest".into();
        assert_eq!(
            configured_copilot_model_override(&config, None, false).as_deref(),
            Some("custom/coach:latest")
        );
        assert_eq!(configured_copilot_model_override(&config, None, true), None);
        assert_eq!(
            configured_copilot_model_override(&config, Some("forced:tag"), false).as_deref(),
            Some("forced:tag")
        );
    }

    #[test]
    fn coach_hardware_parsers_are_deterministic() {
        assert_eq!(
            parse_linux_meminfo_bytes("MemTotal:       16384000 kB\nMemFree: 10 kB"),
            Some(16_777_216_000)
        );
        assert_eq!(parse_linux_meminfo_bytes("MemFree: 10 kB"), None);
        assert!(is_apple_silicon_brand("Apple M4 Max"));
        assert!(!is_apple_silicon_brand("Intel(R) Core(TM) i9"));
    }

    #[test]
    fn copilot_setup_decision_uses_mocked_probe_state_without_network() {
        let cases = [
            ((true, true, false, false), CopilotSetupAction::Ready),
            ((true, false, false, false), CopilotSetupAction::PullModel),
            ((false, false, false, true), CopilotSetupAction::StartOllama),
            (
                (false, false, true, false),
                CopilotSetupAction::InstallWithBrew,
            ),
            (
                (false, false, false, false),
                CopilotSetupAction::DownloadGuidance,
            ),
            ((false, true, true, true), CopilotSetupAction::StartOllama),
        ];

        for ((api_reachable, model_present, brew_present, cli_present), expected) in cases {
            assert_eq!(
                decide_copilot_setup(api_reachable, model_present, brew_present, cli_present,),
                expected
            );
        }
    }

    #[test]
    fn ollama_tags_detect_configured_model_and_latest_alias() {
        let models = parse_ollama_model_tags(
            r#"{
                "models": [
                    {"name": "llama3.2:latest", "model": "llama3.2:latest"},
                    {"name": "qwen3:4b", "model": "qwen3:4b"}
                ]
            }"#,
        )
        .unwrap();

        assert_eq!(models, ["llama3.2:latest", "qwen3:4b"]);
        assert!(ollama_model_is_present(&models, "llama3.2"));
        assert!(ollama_model_is_present(&models, "llama3.2:latest"));
        assert!(ollama_model_is_present(&models, "qwen3:4b"));
        assert!(!ollama_model_is_present(&models, "qwen3:8b"));
    }

    #[test]
    fn ollama_pull_progress_frames_parse_without_network() {
        let downloading: OllamaPullFrame =
            serde_json::from_str(r#"{"status":"pulling layers","completed":50,"total":100}"#)
                .unwrap();
        let success: OllamaPullFrame = serde_json::from_str(r#"{"status":"success"}"#).unwrap();

        assert_eq!(downloading.status, "pulling layers");
        assert_eq!(downloading.completed, Some(50));
        assert_eq!(downloading.total, Some(100));
        assert!(success.status.eq_ignore_ascii_case("success"));
        assert!(success.error.is_none());
    }

    #[test]
    fn ollama_download_guidance_is_plain_and_actionable() {
        let guidance = ollama_download_guidance();

        assert!(guidance.contains("free Ollama app"));
        assert!(guidance.contains("https://ollama.com/download"));
        assert!(guidance.contains("open it once"));
        assert!(guidance.contains("minutes coach setup"));
        assert!(!guidance.contains("Homebrew"));
        assert!(!guidance.contains("provider"));
        assert!(!guidance.contains("contract"));
    }

    #[test]
    fn copilot_start_with_no_local_model_returns_guided_setup_not_error() {
        let state = minutes_core::copilot::CopilotSetupNeeded::private_ai();
        let output = format_copilot_setup_needed(&state);

        assert!(output.contains("small on-device AI model"));
        assert!(output.contains("minutes coach setup"));
        assert!(output.contains("Then start Coach again"));
    }

    fn test_partial_nudge(
        session_epoch: u64,
        utterance_sequence: u64,
        revision: u64,
        text: &str,
    ) -> minutes_core::copilot::Nudge {
        use minutes_core::copilot::{
            BattleCard, CopilotRequest, CopilotUtterance, MeetingMode, NudgeDraft, NudgeKind,
            NudgePolicy, OpportunityKind, StrategyState, TranscriptUpdateKind,
        };

        let request = CopilotRequest {
            goal: "land the decision".into(),
            mode: MeetingMode::Generic,
            session_epoch,
            evidence_revision: revision,
            evidence_utterance_sequence: utterance_sequence,
            evidence_utterance_revision: revision,
            update_kind: TranscriptUpdateKind::Partial,
            utterances: vec![CopilotUtterance {
                utterance_sequence,
                revision,
                update_kind: TranscriptUpdateKind::Partial,
                source: "in-process-live".into(),
                text: text.into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 10,
            }],
            battle_card: BattleCard::empty(),
            strategy_state: StrategyState::empty(),
        };
        NudgePolicy::new(12_000)
            .accept(
                NudgeDraft {
                    kind: NudgeKind::Say,
                    text: text.into(),
                    source_chip: "decision".into(),
                    opportunity: OpportunityKind::Decision,
                    confidence: 100,
                },
                &request,
                chrono::Utc::now(),
            )
            .unwrap()
    }

    #[test]
    fn producer_update_between_poll_and_render_suppresses_partial_nudge() {
        use minutes_core::live_partials::{
            channel, LivePartialEvent, PartialPublishOutcome, SupersessionReason,
        };

        let (mut publisher, mut subscriber) = channel(91, 2);
        publisher.begin_utterance(std::time::Instant::now());
        assert_eq!(
            publisher.try_publish("Approve".into(), 0),
            PartialPublishOutcome::Published
        );
        let LivePartialEvent::Partial(approve) = subscriber.try_recv().unwrap() else {
            panic!("first partial");
        };
        let approve_nudge = test_partial_nudge(
            approve.session_epoch,
            approve.utterance_sequence,
            approve.revision,
            &approve.text,
        );
        assert!(copilot_nudge_is_fresh_for_render(
            &approve_nudge,
            Some(&subscriber)
        ));

        // This arrives after runner polling but before the output write.
        assert_eq!(
            publisher.try_publish("Reject".into(), 1),
            PartialPublishOutcome::Published
        );
        assert!(!copilot_nudge_is_fresh_for_render(
            &approve_nudge,
            Some(&subscriber)
        ));

        let LivePartialEvent::Partial(reject) = subscriber.try_recv().unwrap() else {
            panic!("corrected partial");
        };
        let reject_nudge = test_partial_nudge(
            reject.session_epoch,
            reject.utterance_sequence,
            reject.revision,
            &reject.text,
        );
        publisher.supersede_current(SupersessionReason::Finalized);
        assert!(!copilot_nudge_is_fresh_for_render(
            &reject_nudge,
            Some(&subscriber)
        ));
    }

    struct PanickingNudgeSink;

    impl std::io::Write for PanickingNudgeSink {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            panic!("simulated broken pipe panic");
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn capture_guard_finalizes_wav_when_nudge_sink_panics() {
        with_temp_home(|home| {
            let wav_path = home.join("copilot-live.wav");
            let stop = Arc::new(AtomicBool::new(false));
            let capture_stop = Arc::clone(&stop);
            let thread_path = wav_path.clone();
            let (finalized_tx, finalized_rx) = std::sync::mpsc::channel();
            let capture_thread = std::thread::spawn(move || {
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: 16_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&thread_path, spec).unwrap();
                writer.write_sample(1_i16).unwrap();
                while !capture_stop.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                writer.write_sample(2_i16).unwrap();
                writer.finalize().unwrap();
                finalized_tx.send(()).unwrap();
            });

            let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _capture_guard = LiveCaptureGuard::new(stop, Some(capture_thread));
                let nudge = test_partial_nudge(1, 1, 1, "Approve");
                let mut sink = PanickingNudgeSink;
                render_copilot_nudge_to(&nudge, "stdout", false, &mut sink).unwrap();
            }));
            assert!(panic.is_err());
            finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("capture guard must stop and join the WAV writer");
            let reader = hound::WavReader::open(&wav_path).expect("finalized WAV header");
            assert_eq!(reader.duration(), 2);
        });
    }

    #[test]
    fn restricted_history_context_is_not_rendered_by_copilot_hud() {
        use minutes_core::copilot::{
            BattleCard, CopilotRequest, CopilotUtterance, MeetingMode, NudgeDraft, NudgeKind,
            NudgePolicy, OpportunityKind, StrategyState, TranscriptUpdateKind,
        };

        const RESTRICTED_CANARY: &str = "RESTRICTED_BOARD_PRICING_CANARY";
        let request = CopilotRequest {
            goal: "Confirm public next steps".into(),
            mode: MeetingMode::Decision,
            session_epoch: 1,
            evidence_revision: 1,
            evidence_utterance_sequence: 1,
            evidence_utterance_revision: 1,
            update_kind: TranscriptUpdateKind::Final,
            utterances: vec![CopilotUtterance {
                utterance_sequence: 1,
                revision: 1,
                update_kind: TranscriptUpdateKind::Final,
                source: "live.utterance.final".into(),
                text: "Who owns the public follow-up?".into(),
                speaker: None,
                speaker_verified: false,
                offset_ms: 0,
                duration_ms: 100,
            }],
            // Even if an upstream caller accidentally retains hidden context,
            // the renderer receives only the policy-approved Nudge contract.
            battle_card: BattleCard {
                rendered: RESTRICTED_CANARY.into(),
                ..BattleCard::default()
            },
            strategy_state: StrategyState::empty(),
        };
        let nudge = NudgePolicy::new(12_000)
            .accept(
                NudgeDraft {
                    kind: NudgeKind::Ask,
                    text: "Ask who owns the public follow-up.".into(),
                    source_chip: "public owner".into(),
                    opportunity: OpportunityKind::NextStep,
                    confidence: 100,
                },
                &request,
                chrono::Utc::now(),
            )
            .unwrap();

        for (surface, terminal) in [("tui", true), ("tui", false), ("stdout", false)] {
            let mut output = Vec::new();
            render_copilot_nudge_to(&nudge, surface, terminal, &mut output).unwrap();
            let output = String::from_utf8(output).unwrap();
            assert!(output.contains("public"));
            assert!(!output.contains(RESTRICTED_CANARY));
        }
    }

    struct SaturatedCopilotModel {
        calls: AtomicUsize,
        started: std::sync::mpsc::Sender<u64>,
        release_first: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl minutes_core::copilot::CopilotModel for SaturatedCopilotModel {
        fn provider_name(&self) -> &str {
            "test"
        }

        fn model_name(&self) -> &str {
            "saturated"
        }

        fn prewarm(&self) -> std::result::Result<(), minutes_core::copilot::ModelError> {
            Ok(())
        }

        fn stream_structured(
            &self,
            request: &minutes_core::copilot::CopilotRequest,
            _cancel: &minutes_core::copilot::CancelToken,
            _sink: &dyn minutes_core::copilot::ModelEventSink,
        ) -> std::result::Result<minutes_core::copilot::NudgeDraft, minutes_core::copilot::ModelError>
        {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            self.started.send(request.evidence_revision).unwrap();
            if call == 0 {
                self.release_first
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .recv()
                    .unwrap();
            }
            Ok(minutes_core::copilot::NudgeDraft {
                kind: minutes_core::copilot::NudgeKind::Hold,
                text: String::new(),
                source_chip: String::new(),
                opportunity: minutes_core::copilot::OpportunityKind::General,
                confidence: 100,
            })
        }

        fn health(&self) -> minutes_core::copilot::ModelHealth {
            minutes_core::copilot::ModelHealth {
                provider: "test".into(),
                model: "saturated".into(),
                status: minutes_core::copilot::ModelHealthStatus::Available,
                detail: "ok".into(),
                checked_ts: chrono::Utc::now(),
            }
        }
    }

    fn contention_request(
        session_epoch: u64,
        revision: u64,
    ) -> minutes_core::copilot::CopilotRequest {
        use minutes_core::copilot::{
            BattleCard, CopilotRequest, CopilotUtterance, MeetingMode, StrategyState,
            TranscriptUpdateKind,
        };

        CopilotRequest {
            goal: "Keep capture healthy".into(),
            mode: MeetingMode::Generic,
            session_epoch,
            evidence_revision: revision,
            evidence_utterance_sequence: revision,
            evidence_utterance_revision: 1,
            update_kind: TranscriptUpdateKind::Final,
            utterances: vec![CopilotUtterance {
                utterance_sequence: revision,
                revision: 1,
                update_kind: TranscriptUpdateKind::Final,
                source: "live.utterance.final".into(),
                text: format!("revision {revision}"),
                speaker: None,
                speaker_verified: false,
                offset_ms: revision,
                duration_ms: 1,
            }],
            battle_card: BattleCard::empty(),
            strategy_state: StrategyState::empty(),
        }
    }

    #[test]
    fn saturated_copilot_sheds_work_without_delaying_capture_finalization() {
        use minutes_core::copilot::{CopilotRunner, NudgePolicy, SubmitOutcome};

        with_temp_home(|home| {
            let wav_path = home.join("contention.wav");
            let capture_stop = Arc::new(AtomicBool::new(false));
            let capture_stop_worker = Arc::clone(&capture_stop);
            let capture_path = wav_path.clone();
            let (finalized_tx, finalized_rx) = std::sync::mpsc::channel();
            let capture_worker = std::thread::spawn(move || {
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: 16_000,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let mut writer = hound::WavWriter::create(&capture_path, spec).unwrap();
                writer.write_sample(1_i16).unwrap();
                while !capture_stop_worker.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                writer.write_sample(2_i16).unwrap();
                writer.finalize().unwrap();
                finalized_tx.send(()).unwrap();
            });

            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let model = Arc::new(SaturatedCopilotModel {
                calls: AtomicUsize::new(0),
                started: started_tx,
                release_first: Mutex::new(release_rx),
            });
            let runner = CopilotRunner::start(model, NudgePolicy::new(12_000));
            let epoch = runner.session_epoch();
            assert_eq!(
                runner.submit(contention_request(epoch, 1)),
                SubmitOutcome::Queued
            );
            assert_eq!(started_rx.recv_timeout(Duration::from_secs(1)).unwrap(), 1);

            let submitted_at = std::time::Instant::now();
            let dropped = (2..=128)
                .filter(|revision| {
                    runner.submit(contention_request(epoch, *revision))
                        == SubmitOutcome::DroppedQueueFull
                })
                .count();
            assert!(dropped > 0, "copilot must shed work when its queue is full");
            assert!(
                submitted_at.elapsed() < Duration::from_secs(1),
                "nonblocking copilot submission must stay bounded under contention"
            );

            // The model is still blocked and its bounded queue is saturated.
            // Capture finalization owns no copilot lock or queue and completes
            // before the provider is released.
            capture_stop.store(true, Ordering::Release);
            finalized_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("capture must finalize while copilot remains saturated");
            capture_worker.join().unwrap();
            assert_eq!(hound::WavReader::open(&wav_path).unwrap().duration(), 2);

            release_tx.send(()).unwrap();
            let retry_deadline = std::time::Instant::now() + Duration::from_secs(1);
            let mut revision = 129;
            let queued_revision = loop {
                let outcome = runner.submit(contention_request(epoch, revision));
                if matches!(
                    outcome,
                    SubmitOutcome::Queued | SubmitOutcome::CancelledOlderRequest
                ) {
                    break revision;
                }
                assert!(
                    std::time::Instant::now() < retry_deadline,
                    "copilot worker did not free its bounded queue"
                );
                revision += 1;
                std::thread::yield_now();
            };
            assert_eq!(
                started_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
                queued_revision,
                "copilot worker should resume with fresh work after shedding saturation"
            );
            runner.stop();
        });
    }

    #[test]
    fn coach_user_surfaces_use_plain_language() {
        use minutes_core::copilot::{
            CopilotInputMode, CopilotSessionStatus, CopilotState, Nudge, NudgeKind,
        };

        let setup_output =
            format_copilot_setup_needed(&minutes_core::copilot::CopilotSetupNeeded::private_ai());
        let mut status = CopilotSessionStatus::default();
        status.active = true;
        status.goal = "Agree on next steps".into();
        status.capture_attachment = "Coach is listening to the current recording.".into();
        status.input_mode = CopilotInputMode::FinalOnly;
        status.health.state = CopilotState::Degraded;
        status.health.provider = "apple-fm".into();
        status.health.last_error = Some(
            "apple-fm provider failed at utterance epoch; ollama final_only fallback failed".into(),
        );
        let status_output = format_copilot_status(&status);
        let nudge = Nudge {
            v: 1,
            id: "nudge-1".into(),
            kind: NudgeKind::Ask,
            text: "Ask who owns the next step.".into(),
            source_chip: "next-step owner".into(),
            opportunity: minutes_core::copilot::OpportunityKind::General,
            confidence: 100,
            session_epoch: 1,
            evidence_revision: 42,
            evidence_utterance_sequence: 42,
            evidence_utterance_revision: 42,
            grounded_partial_utterance_sequence: None,
            grounded_partial_utterance_revision: None,
            update_kind: minutes_core::copilot::TranscriptUpdateKind::Final,
            created_ts: chrono::Utc::now(),
            ttl_ms: 12_000,
            supersedes: None,
        };
        let nudge_output = format_copilot_nudge(&nudge, false);
        let outputs = [setup_output, status_output.clone(), nudge_output];
        let forbidden = [
            "final_only",
            "auto-local",
            "apple-fm",
            "ollama",
            "contract v1",
            "provider",
            "utterance",
            "epoch",
        ];

        for output in outputs {
            let lower = output.to_ascii_lowercase();
            for term in forbidden {
                assert!(
                    !lower.contains(term),
                    "user-facing Coach output contains {term:?}: {output:?}"
                );
            }
        }
        assert!(status_output.contains("Coach status:"));
        assert!(status_output.contains("Coach will keep trying"));
        assert!(status_output.contains("Using your local AI model."));
        assert!(status_output.contains("Coaching on completed sentences (a bit slower)."));
    }

    #[test]
    fn transcribe_subcommand_parses_all_flags() {
        let parsed = Cli::try_parse_from([
            "minutes",
            "transcribe",
            "/tmp/audio.wav",
            "--json",
            "--language",
            "en",
            "--diarize",
        ])
        .expect("transcribe must parse all flags");
        match parsed.command {
            Commands::Transcribe {
                path,
                json,
                language,
                diarize,
            } => {
                assert_eq!(path, PathBuf::from("/tmp/audio.wav"));
                assert!(json);
                assert_eq!(language.as_deref(), Some("en"));
                assert!(diarize);
            }
            _ => panic!("expected Transcribe variant"),
        }
    }

    #[test]
    fn transcribe_output_uses_snake_case_duration() {
        let output = TranscribeOutput {
            text: "hello".to_string(),
            language: "en".to_string(),
            segments: vec![],
            duration_ms: 1234,
        };
        let json = serde_json::to_string(&output).unwrap();
        assert!(
            json.contains("\"duration_ms\""),
            "expected snake_case duration_ms, got: {}",
            json
        );
        assert!(
            !json.contains("durationMs"),
            "camelCase durationMs must not appear in output, got: {}",
            json
        );
    }

    #[test]
    fn transcribe_segment_omits_speaker_when_none() {
        let seg = TranscribeSegmentOutput {
            start: 0.0,
            end: 1.5,
            text: "hello".to_string(),
            speaker: None,
        };
        let json = serde_json::to_string(&seg).unwrap();
        assert!(
            !json.contains("speaker"),
            "speaker field must be omitted when None, got: {}",
            json
        );
    }
}

// Frontmatter parsing is in minutes_core::markdown::{split_frontmatter, extract_field}

fn cmd_delete(meeting: &str, with_audio: bool, force: bool, config: &Config) -> Result<()> {
    // Resolve the slug to a file path
    let md_path = if Path::new(meeting).exists() {
        PathBuf::from(meeting)
    } else {
        minutes_core::search::resolve_slug(meeting, config)
            .ok_or_else(|| anyhow::anyhow!("no meeting found matching: {}", meeting))?
    };

    let title = md_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let audio_artifacts = minutes_core::capture::meeting_audio_artifact_paths(&md_path);
    let has_audio = audio_artifacts.iter().any(|path| path.exists());

    if force {
        // Permanent delete
        std::fs::remove_file(&md_path)?;
        eprintln!("Deleted: {}", md_path.display());

        if with_audio && has_audio {
            for path in audio_artifacts.iter().filter(|path| path.exists()) {
                std::fs::remove_file(path)?;
                eprintln!("Deleted audio artifact: {}", path.display());
            }
        }
    } else {
        // Soft delete: move to archive directory
        let archive_dir = config.output_dir.join("archive");
        std::fs::create_dir_all(&archive_dir)?;

        let dest_md = archive_dir.join(md_path.file_name().unwrap());
        std::fs::rename(&md_path, &dest_md)?;
        eprintln!("Archived: {} → {}", title, dest_md.display());

        if with_audio && has_audio {
            for path in audio_artifacts.iter().filter(|path| path.exists()) {
                let dest_audio = archive_dir.join(path.file_name().unwrap());
                std::fs::rename(path, &dest_audio)?;
                eprintln!("Archived audio artifact: {}", dest_audio.display());
            }
        }
    }

    if has_audio && !with_audio {
        eprintln!(
            "Note: audio artifacts still exist alongside {}. Use --with-audio to remove them.",
            md_path.display()
        );
    }

    Ok(())
}

fn cmd_schema() -> Result<()> {
    let schema = schemars::schema_for!(minutes_core::markdown::Frontmatter);
    let json = serde_json::to_string_pretty(&schema)?;
    println!("{}", json);
    Ok(())
}

fn cmd_get(slug_or_path: &str, json: bool, compact_json: bool, config: &Config) -> Result<()> {
    // Accept either a slug ("2026-03-17-advisor-call") or a path to the
    // meeting markdown. MCP and Tauri pass paths; humans pass slugs. Paths —
    // whether absolute or relative to cwd — must resolve to a .md file
    // inside the configured meetings directory. The check happens via
    // `notes::validate_meeting_path`, which canonicalizes both sides and
    // rejects escapes (preventing `minutes get /etc/passwd.md` from
    // leaking arbitrary files).
    let path = if let Some(p) = minutes_core::search::resolve_slug(slug_or_path, config) {
        p
    } else {
        let candidate = std::path::PathBuf::from(slug_or_path);
        if !candidate.exists() || candidate.extension().and_then(|s| s.to_str()) != Some("md") {
            anyhow::bail!("no meeting found matching slug or path: {}", slug_or_path);
        }
        if let Err(msg) = minutes_core::notes::validate_meeting_path(&candidate, &config.output_dir)
        {
            anyhow::bail!("{}", msg);
        }
        candidate
    };

    let content = std::fs::read_to_string(&path)?;

    if !json {
        println!("{}", content);
        return Ok(());
    }

    // Structured JSON with overlays layered in. Raw body is preserved verbatim;
    // only speaker_map is rewritten to reflect sidecar confirmations. Agents
    // and UIs can apply the renaming to body lines themselves if they want to,
    // but the markdown on disk stays untouched.
    let (frontmatter_str, body) = minutes_core::markdown::split_frontmatter(&content);
    let mut frontmatter: minutes_core::markdown::Frontmatter = if frontmatter_str.is_empty() {
        anyhow::bail!("meeting has no frontmatter: {}", path.display());
    } else {
        serde_yaml::from_str(frontmatter_str.trim())?
    };

    let overlay_db = minutes_core::overlays::default_db_path();
    let confirmations =
        minutes_core::overlays::load_speaker_confirmations_for_meeting_at(&overlay_db, &path)
            .unwrap_or_default();
    let overlay_applied = !confirmations.is_empty();
    minutes_core::overlays::apply_speaker_confirmations(
        &mut frontmatter.speaker_map,
        &confirmations,
    );

    let payload = serde_json::json!({
        "path": path.to_string_lossy(),
        "frontmatter": frontmatter,
        "body": body,
        "overlay_applied": overlay_applied,
    });
    let payload = if compact_json {
        payload
    } else {
        let mut payload = payload;
        payload["raw_markdown"] = serde_json::Value::String(content);
        payload
    };
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}

fn cmd_copilot(action: CopilotAction, config: &mut Config) -> Result<()> {
    match action {
        CopilotAction::Start {
            goal,
            surface,
            mode,
            live,
        } => cmd_copilot_start(&goal, surface.as_deref(), mode.as_deref(), live, config),
        CopilotAction::Status => cmd_copilot_status(),
        CopilotAction::Pause => cmd_copilot_pause(),
        CopilotAction::Resume => cmd_copilot_resume(),
        CopilotAction::Stop => cmd_copilot_stop(),
        CopilotAction::Feedback { nudge_id, rating } => cmd_copilot_feedback(&nudge_id, &rating),
        CopilotAction::Eval {
            fixtures,
            model,
            accelerated,
            json,
        } => cmd_copilot_eval(fixtures.as_deref(), model.as_deref(), accelerated, json),
        CopilotAction::Setup { model, retune } => {
            cmd_copilot_setup(config, model.as_deref(), retune)
        }
    }
}

fn cmd_copilot_eval(
    fixtures: Option<&Path>,
    model: Option<&str>,
    accelerated: bool,
    json: bool,
) -> Result<()> {
    use minutes_core::copilot::eval::{
        builtin_fixtures, load_fixtures_dir, render_report_table, run_suite, EvalOptions,
        ReplayMode,
    };

    let fixtures = match fixtures {
        Some(path) => load_fixtures_dir(path)?,
        None => builtin_fixtures()?,
    };
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        let report = run_ollama_model_eval(&fixtures, model, accelerated)?;
        if json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print_ollama_model_eval(&report);
        }
        return Ok(());
    }
    let report = run_suite(
        &fixtures,
        EvalOptions {
            mode: if accelerated {
                ReplayMode::Accelerated
            } else {
                ReplayMode::RealTime
            },
        },
    )?;
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_report_table(&report));
        println!(
            "summary_json={}",
            serde_json::to_string(&serde_json::json!({
                "suite_version": report.suite_version,
                "fixed_seed": report.fixed_seed,
                "mode": report.mode,
                "summary": report.summary,
            }))?
        );
    }
    if !report.summary.baseline_passed {
        anyhow::bail!("copilot eval baseline failed");
    }
    Ok(())
}

#[derive(Debug, Serialize)]
struct OllamaModelEvalRate {
    numerator: usize,
    denominator: usize,
    rate: f64,
}

#[derive(Debug, Serialize)]
struct OllamaModelEvalLatency {
    samples: usize,
    p50_ms: Option<u64>,
    p95_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
struct OllamaModelEvalQuality {
    useful_nudge_precision: OllamaModelEvalRate,
    opportunity_recall: OllamaModelEvalRate,
    no_nudge_quality: OllamaModelEvalRate,
}

#[derive(Debug, Serialize)]
struct OllamaModelEvalSample {
    fixture_id: String,
    evidence_utterance_sequence: u64,
    evidence_time_ms: u64,
    ttft_ms: Option<u64>,
    total_ms: u64,
    schema_valid: bool,
    visible_after_policy: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    matched_opportunity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    draft: Option<minutes_core::copilot::NudgeDraft>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct OllamaModelEvalReport {
    suite_version: u32,
    mode: &'static str,
    provider: &'static str,
    model: String,
    fixtures: usize,
    requests: usize,
    schema_valid: OllamaModelEvalRate,
    visible_nudges: usize,
    quality: OllamaModelEvalQuality,
    ttft: OllamaModelEvalLatency,
    total: OllamaModelEvalLatency,
    samples: Vec<OllamaModelEvalSample>,
}

fn run_ollama_model_eval(
    fixtures: &[minutes_core::copilot::eval::EvalFixture],
    model_name: &str,
    accelerated: bool,
) -> Result<OllamaModelEvalReport> {
    use minutes_core::copilot::eval::EVAL_SUITE_VERSION;
    use minutes_core::copilot::{
        BattleCard, CancelToken, CopilotModel, CopilotRequest, CopilotUtterance, ModelStreamEvent,
        NudgePolicy, OllamaCopilotModel, StrategyState, StrategyStateDraft, TranscriptUpdateKind,
    };
    use std::collections::BTreeSet;
    use std::sync::{Arc, Mutex};

    let model = OllamaCopilotModel::new(
        copilot_ollama_base_url(),
        model_name,
        Duration::from_secs(600),
    );
    model
        .prewarm()
        .map_err(|error| anyhow::anyhow!("could not warm Ollama model {model_name}: {error}"))?;

    let mut samples = Vec::new();
    let mut schema_valid = 0usize;
    let mut visible_nudges = 0usize;
    let mut useful_nudges = 0usize;
    let mut matched_opportunities = BTreeSet::new();
    let mut visible_evidence = Vec::<(String, u64)>::new();
    let mut ttft_samples = Vec::new();
    let mut total_samples = Vec::new();

    for (fixture_index, fixture) in fixtures.iter().enumerate() {
        let mut policy = NudgePolicy::for_mode(12_000, fixture.mode);
        let mut previous_evidence_ms = 0u64;
        for (utterance_index, evidence) in fixture.transcript.iter().enumerate() {
            let evidence_time_ms = evidence.offset_ms.saturating_add(evidence.duration_ms);
            if !accelerated && previous_evidence_ms > 0 {
                std::thread::sleep(Duration::from_millis(
                    evidence_time_ms.saturating_sub(previous_evidence_ms),
                ));
            }
            previous_evidence_ms = evidence_time_ms;

            let evidence_revision = (utterance_index + 1) as u64;
            let utterances = fixture.transcript[..=utterance_index]
                .iter()
                .map(|utterance| CopilotUtterance {
                    utterance_sequence: utterance.utterance_sequence,
                    revision: utterance.utterance_sequence,
                    update_kind: TranscriptUpdateKind::Final,
                    source: utterance.source.clone(),
                    text: utterance.final_text.clone(),
                    speaker: None,
                    speaker_verified: false,
                    offset_ms: utterance.offset_ms,
                    duration_ms: utterance.duration_ms,
                })
                .collect();
            let strategy_state = fixture
                .labels
                .strategy
                .as_ref()
                .map(|expected| {
                    StrategyState::from_draft(
                        StrategyStateDraft {
                            open_threads: expected.open_threads.clone(),
                            unmet_goal_items: expected.unmet_goal_items.clone(),
                            unresolved_objections: expected.unresolved_objections.clone(),
                            steer_toward: expected.steer_toward.clone(),
                        },
                        evidence_revision,
                    )
                })
                .unwrap_or_else(StrategyState::empty);
            let request = CopilotRequest {
                goal: fixture.goal.clone(),
                mode: fixture.mode,
                session_epoch: (fixture_index + 1) as u64,
                evidence_revision,
                evidence_utterance_sequence: evidence.utterance_sequence,
                evidence_utterance_revision: evidence.utterance_sequence,
                update_kind: TranscriptUpdateKind::Final,
                utterances,
                battle_card: BattleCard::empty(),
                strategy_state,
            };

            let first_token = Arc::new(Mutex::new(None::<std::time::Instant>));
            let first_token_sink = Arc::clone(&first_token);
            let started = std::time::Instant::now();
            let sink = move |event: ModelStreamEvent| {
                if matches!(event, ModelStreamEvent::TextDelta(ref text) if !text.is_empty()) {
                    first_token_sink
                        .lock()
                        .unwrap()
                        .get_or_insert_with(std::time::Instant::now);
                }
            };
            let result = model.stream_structured(&request, &CancelToken::new(), &sink);
            let completed = std::time::Instant::now();
            let total_ms = duration_ms(completed.duration_since(started));
            let ttft_ms = first_token
                .lock()
                .unwrap()
                .map(|observed| duration_ms(observed.duration_since(started)));
            total_samples.push(total_ms);
            if let Some(ttft_ms) = ttft_ms {
                ttft_samples.push(ttft_ms);
            }

            let mut sample = OllamaModelEvalSample {
                fixture_id: fixture.id.clone(),
                evidence_utterance_sequence: evidence.utterance_sequence,
                evidence_time_ms,
                ttft_ms,
                total_ms,
                schema_valid: false,
                visible_after_policy: false,
                matched_opportunity: None,
                draft: None,
                error: None,
            };
            match result {
                Ok(draft) => {
                    schema_valid = schema_valid.saturating_add(1);
                    sample.schema_valid = true;
                    sample.draft = Some(draft.clone());
                    let now = chrono::Utc
                        .with_ymd_and_hms(2026, 1, 1, 0, 0, 0)
                        .single()
                        .expect("fixed eval date is valid")
                        + chrono::Duration::milliseconds(
                            evidence_time_ms.min(i64::MAX as u64) as i64
                        );
                    if policy.accept(draft.clone(), &request, now).is_some() {
                        sample.visible_after_policy = true;
                        visible_nudges = visible_nudges.saturating_add(1);
                        visible_evidence.push((fixture.id.clone(), evidence_time_ms));
                        if let Some(label_id) = match_model_eval_opportunity(
                            &fixture.labels.opportunities,
                            evidence_time_ms,
                            &draft,
                        ) {
                            useful_nudges = useful_nudges.saturating_add(1);
                            let qualified = format!("{}:{label_id}", fixture.id);
                            matched_opportunities.insert(qualified);
                            sample.matched_opportunity = Some(label_id.to_string());
                        }
                    }
                }
                Err(error) => sample.error = Some(error.to_string()),
            }
            samples.push(sample);
        }
    }

    let opportunity_count = fixtures
        .iter()
        .map(|fixture| fixture.labels.opportunities.len())
        .sum();
    let no_opportunity_count = fixtures
        .iter()
        .map(|fixture| fixture.labels.no_opportunity_ranges.len())
        .sum();
    let clean_no_opportunity_ranges = fixtures
        .iter()
        .flat_map(|fixture| {
            fixture.labels.no_opportunity_ranges.iter().map(|range| {
                !visible_evidence.iter().any(|(fixture_id, evidence_ms)| {
                    fixture_id == &fixture.id
                        && *evidence_ms >= range.start_ms
                        && *evidence_ms <= range.end_ms
                })
            })
        })
        .filter(|clean| *clean)
        .count();
    let requests = samples.len();

    Ok(OllamaModelEvalReport {
        suite_version: EVAL_SUITE_VERSION,
        mode: if accelerated {
            "accelerated"
        } else {
            "real_time"
        },
        provider: "ollama",
        model: model_name.into(),
        fixtures: fixtures.len(),
        requests,
        schema_valid: model_eval_rate(schema_valid, requests),
        visible_nudges,
        quality: OllamaModelEvalQuality {
            useful_nudge_precision: model_eval_rate(useful_nudges, visible_nudges),
            opportunity_recall: model_eval_rate(matched_opportunities.len(), opportunity_count),
            no_nudge_quality: model_eval_rate(clean_no_opportunity_ranges, no_opportunity_count),
        },
        ttft: model_eval_latency(&ttft_samples),
        total: model_eval_latency(&total_samples),
        samples,
    })
}

fn match_model_eval_opportunity<'a>(
    labels: &'a [minutes_core::copilot::eval::OpportunityLabel],
    evidence_time_ms: u64,
    draft: &minutes_core::copilot::NudgeDraft,
) -> Option<&'a str> {
    let text = draft.text.to_ascii_lowercase();
    labels
        .iter()
        .find(|label| {
            evidence_time_ms >= label.start_ms
                && evidence_time_ms <= label.end_ms
                && label.kind.is_none_or(|kind| kind == draft.kind)
                && label
                    .match_any
                    .iter()
                    .any(|needle| text.contains(&needle.to_ascii_lowercase()))
        })
        .map(|label| label.id.as_str())
}

fn model_eval_rate(numerator: usize, denominator: usize) -> OllamaModelEvalRate {
    OllamaModelEvalRate {
        numerator,
        denominator,
        rate: if denominator == 0 {
            1.0
        } else {
            numerator as f64 / denominator as f64
        },
    }
}

fn model_eval_latency(samples: &[u64]) -> OllamaModelEvalLatency {
    OllamaModelEvalLatency {
        samples: samples.len(),
        p50_ms: model_eval_percentile(samples, 0.50),
        p95_ms: model_eval_percentile(samples, 0.95),
    }
}

fn model_eval_percentile(samples: &[u64], percentile: f64) -> Option<u64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = (percentile.clamp(0.0, 1.0) * sorted.len() as f64).ceil() as usize;
    sorted.get(rank.saturating_sub(1)).copied()
}

fn print_ollama_model_eval(report: &OllamaModelEvalReport) {
    println!("Coach model eval | ollama/{}", report.model);
    println!(
        "schema valid: {}/{} ({:.1}%)",
        report.schema_valid.numerator,
        report.schema_valid.denominator,
        report.schema_valid.rate * 100.0
    );
    println!(
        "visible nudges: {} | useful precision: {:.1}% | opportunity recall: {:.1}% | no-nudge quality: {:.1}%",
        report.visible_nudges,
        report.quality.useful_nudge_precision.rate * 100.0,
        report.quality.opportunity_recall.rate * 100.0,
        report.quality.no_nudge_quality.rate * 100.0,
    );
    println!(
        "TTFT p50/p95: {:?}/{:?} ms | total p50/p95: {:?}/{:?} ms",
        report.ttft.p50_ms, report.ttft.p95_ms, report.total.p50_ms, report.total.p95_ms,
    );
}

struct LiveCaptureGuard {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl LiveCaptureGuard {
    fn new(
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        thread: Option<std::thread::JoinHandle<()>>,
    ) -> Self {
        Self { stop, thread }
    }

    fn stop_and_join(&mut self) -> std::thread::Result<()> {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        self.thread.take().map_or(Ok(()), |thread| thread.join())
    }
}

impl Drop for LiveCaptureGuard {
    fn drop(&mut self) {
        let _ = self.stop_and_join();
    }
}

fn cmd_copilot_start(
    goal: &str,
    surface: Option<&str>,
    mode: Option<&str>,
    own_live_capture: bool,
    config: &Config,
) -> Result<()> {
    use minutes_core::copilot::{
        BattleCard, CaptureAttachPlan, CaptureRelayClient, CopilotEvidenceMode, CopilotRequest,
        CopilotRunner, CopilotSessionStatus, CopilotState, CopilotUtterance, DepthLaneConfig,
        MeetingMode, NudgePolicy, OllamaCopilotModel, PartialLatencySeed, RelayCursor, RelayFrame,
        RelayTranscriptUpdate, RepositoryGrounding, RunnerEvent, StrategyState,
        TranscriptUpdateKind,
    };
    use minutes_core::live_partials::{self, LivePartialEvent, DEFAULT_PARTIAL_CHANNEL_CAPACITY};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    let goal = goal.trim();
    if goal.is_empty() {
        anyhow::bail!("Tell Coach what you want help with using --goal \"...\"");
    }
    let surface = surface.unwrap_or(config.copilot.surface.as_str());
    if !matches!(surface, "tui" | "stdout") {
        anyhow::bail!(
            "Coach does not recognize that display choice. Use tui or stdout in config.toml"
        );
    }
    let mode = mode
        .unwrap_or(config.copilot.mode.as_str())
        .parse::<MeetingMode>()
        .map_err(anyhow::Error::msg)?;

    let candidates: Vec<Arc<dyn minutes_core::copilot::CopilotModel>> = vec![
        Arc::new(OllamaCopilotModel::from_config(&config.copilot)),
        Arc::new(minutes_core::copilot::AppleFoundationCopilotModel::new(
            "apple-foundation-model",
        )),
        Arc::new(minutes_core::copilot::CloudCopilotModel::new(
            config.copilot.fast_model.clone(),
        )),
    ];
    let requested_provider = match config.copilot.resolved_fast_provider() {
        "auto-local" => None,
        provider => Some(provider),
    };
    let (model, provider_selection) = match minutes_core::copilot::route_fast_model(
        candidates,
        requested_provider,
        minutes_core::copilot::RoutingPolicy::local_first(
            config.copilot.allow_cloud,
            4_096,
            config.copilot.target_latency_ms,
        ),
    ) {
        minutes_core::copilot::FastModelRoute::Selected { model, detail, .. } => (model, detail),
        minutes_core::copilot::FastModelRoute::SetupRequired { message, probes } => {
            let setup = minutes_core::copilot::CopilotSetupNeeded::private_ai();
            let status = CopilotSessionStatus {
                goal: goal.into(),
                surface: surface.into(),
                capture_attachment: "Coach did not open a recording while setup is needed.".into(),
                provider_selection: message.clone(),
                setup_needed: Some(setup.clone()),
                input_mode: minutes_core::copilot::CopilotInputMode::FinalOnly,
                updated_ts: chrono::Utc::now(),
                ..CopilotSessionStatus::default()
            };
            minutes_core::copilot::write_session_status(&status)?;
            for probe in probes {
                tracing::debug!(
                    provider = %probe.provider,
                    model = %probe.model,
                    detail = %probe.detail,
                    measured_latency_ms = probe.measured_latency_ms,
                    context_window_tokens = probe.context_window_tokens,
                    "Coach model probe requires setup"
                );
            }
            eprintln!("{}", format_copilot_setup_needed(&setup));
            return Ok(());
        }
    };

    let session_guard = minutes_core::copilot::create_session_guard().map_err(|error| {
        tracing::debug!(%error, "Coach session lock failed");
        anyhow::anyhow!("Coach is already running. Stop it before starting another session")
    })?;
    minutes_core::copilot::clear_session_controls()?;

    let external_capture = copilot_external_capture_active();
    let in_process_live = own_live_capture && !external_capture;
    #[cfg(not(feature = "whisper"))]
    if in_process_live {
        anyhow::bail!(
            "in-process copilot live capture requires the whisper feature; external attachment remains final_only"
        );
    }
    let mut relay_client = if external_capture {
        match connect_capture_relay_with_retry(RelayCursor::default(), Duration::from_secs(2)) {
            Ok(client) => Some(client),
            Err(error) => {
                let plan =
                    minutes_core::copilot::plan_capture_attachment(true, false, own_live_capture);
                let CaptureAttachPlan::RefuseDuplicate { message } = plan else {
                    unreachable!("an active capture without a relay must be refused")
                };
                anyhow::bail!("{message} Attachment detail: {error}");
            }
        }
    } else {
        None
    };
    let evidence_mode = relay_client
        .as_ref()
        .map(|client| client.discovery().evidence_mode)
        .unwrap_or_else(|| copilot_evidence_mode(own_live_capture, external_capture, config));
    let capture_attachment = if in_process_live {
        if evidence_mode == CopilotEvidenceMode::InProcessPartials {
            "Coach is listening in real time. Its live capture will be finalized when Coach stops."
                .into()
        } else {
            "Coach is listening to completed sentences from its live capture.".into()
        }
    } else if let Some(client) = relay_client.as_ref() {
        let _ = client;
        "Coach is listening to the current recording. Your recording remains in control of the microphone."
            .into()
    } else {
        copilot_capture_attachment()
    };
    eprintln!("Coach is getting ready...");
    tracing::debug!(%provider_selection, "Coach selected its fast model");
    eprintln!("{capture_attachment}");
    if evidence_mode == CopilotEvidenceMode::FinalOnly {
        eprintln!(
            "{}",
            minutes_core::copilot::CopilotInputMode::FinalOnly
                .user_message()
                .expect("final-only input has user guidance")
        );
    }
    if own_live_capture && external_capture {
        eprintln!("Coach attached to the active recording without opening another microphone.");
    }
    if !config.copilot.enabled {
        eprintln!("Coach is on for this meeting only.");
    }

    // Retrieval belongs exclusively to the slow worker. The first stable final
    // triggers an asynchronous refresh; fast requests start with an empty card.
    let battle_card = BattleCard::empty();
    let grounding = config.copilot.history_grounding.then(|| {
        Arc::new(RepositoryGrounding::new(config.clone()))
            as Arc<dyn minutes_core::copilot::GroundingSource>
    });

    let runner = CopilotRunner::start_with_depth(
        model,
        NudgePolicy::for_mode(config.copilot.nudge_ttl_ms, mode),
        Duration::from_millis(config.copilot.partial_debounce_ms),
        grounding,
        DepthLaneConfig::new(
            Duration::from_secs(config.copilot.depth_refresh_secs),
            Duration::from_secs(config.copilot.grounding_refresh_secs),
        ),
    );
    let session_epoch = runner.session_epoch();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_signal = stop.clone();
    ctrlc::set_handler(move || {
        stop_for_signal.store(true, Ordering::Release);
    })?;

    let (partial_publisher, mut partial_subscriber) =
        if evidence_mode == CopilotEvidenceMode::InProcessPartials {
            let (publisher, subscriber) =
                live_partials::channel(session_epoch, DEFAULT_PARTIAL_CHANNEL_CAPACITY);
            (Some(publisher), Some(subscriber))
        } else {
            (None, None)
        };

    #[cfg(feature = "whisper")]
    let (capture_result_rx, capture_thread) = if in_process_live {
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let capture_stop = Arc::clone(&stop);
        let capture_config = config.clone();
        let thread = std::thread::Builder::new()
            .name("copilot-live-capture".into())
            .spawn(move || {
                let result = minutes_core::live_transcript::run_with_partials(
                    capture_stop,
                    &capture_config,
                    None,
                    partial_publisher,
                );
                let _ = result_tx.send(result);
            })?;
        (Some(result_rx), Some(thread))
    } else {
        (None, None)
    };
    #[cfg(not(feature = "whisper"))]
    type LiveCaptureResult = Result<(usize, f64, PathBuf), minutes_core::MinutesError>;
    #[cfg(not(feature = "whisper"))]
    let capture_result_rx: Option<std::sync::mpsc::Receiver<LiveCaptureResult>> = None;
    #[cfg(not(feature = "whisper"))]
    let capture_thread: Option<std::thread::JoinHandle<()>> = None;
    #[cfg(not(feature = "whisper"))]
    drop(partial_publisher);
    // From this point onward every return and unwind stops and joins capture,
    // allowing live_transcript::run_with_partials to finalize its WAV.
    let mut capture_guard = LiveCaptureGuard::new(Arc::clone(&stop), capture_thread);

    let mut cursor = minutes_core::events::latest_event_seq();
    let mut next_evidence_revision = cursor;
    let mut latest_partial_identity_seen = None;
    let mut utterances: VecDeque<CopilotUtterance> = VecDeque::new();
    let mut paused = false;
    let mut last_status_write = std::time::Instant::now() - Duration::from_secs(2);
    let mut status = CopilotSessionStatus {
        active: true,
        pid: Some(std::process::id()),
        goal: goal.into(),
        surface: surface.into(),
        cursor,
        relay_cursor: relay_client.as_ref().map(CaptureRelayClient::cursor),
        evidence_mode,
        capture_attachment: capture_attachment.clone(),
        provider_selection,
        setup_needed: None,
        input_mode: if evidence_mode == CopilotEvidenceMode::FinalOnly {
            minutes_core::copilot::CopilotInputMode::FinalOnly
        } else {
            minutes_core::copilot::CopilotInputMode::Realtime
        },
        health: runner.health(),
        updated_ts: chrono::Utc::now(),
    };
    minutes_core::copilot::write_session_status(&status)?;

    eprintln!(
        "Coach is listening in {mode} mode. Press Ctrl-C or run `minutes coach stop` to stop."
    );
    let mut capture_failure = None;
    let mut relay_reconnect_cursor = relay_client
        .as_ref()
        .map(CaptureRelayClient::cursor)
        .unwrap_or_default();
    let mut relay_reconnect_after = Instant::now();
    let mut relay_shutdown = false;

    while !stop.load(Ordering::Acquire) {
        if minutes_core::copilot::copilot_stop_path().exists() {
            break;
        }

        match minutes_core::copilot::take_feedback_request() {
            Ok(Some(feedback)) => {
                let outcome = runner.record_feedback(feedback.nudge_id, feedback.feedback);
                if outcome == minutes_core::copilot::FeedbackOutcome::DroppedQueueFull {
                    tracing::warn!("copilot feedback queue full; feedback was not applied");
                }
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(error = %error, "failed to read copilot feedback"),
        }

        let pause_requested = minutes_core::copilot::copilot_pause_path().exists();
        if pause_requested != paused {
            paused = pause_requested;
            if paused {
                runner.pause();
                eprintln!("Coach is paused. Your recording continues.");
            } else {
                runner.resume();
                eprintln!("Coach is listening again.");
            }
        }

        if let Some(result_rx) = capture_result_rx.as_ref() {
            match result_rx.try_recv() {
                Ok(Ok((lines, duration, path))) => {
                    eprintln!(
                        "In-process live capture ended: {lines} utterances in {duration:.0}s ({})",
                        path.display()
                    );
                    break;
                }
                Ok(Err(error)) => {
                    capture_failure = Some(anyhow::anyhow!(error));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    capture_failure = Some(anyhow::anyhow!(
                        "in-process live capture worker exited without a result"
                    ));
                    break;
                }
            }
        }

        if relay_client.is_none()
            && external_capture
            && !relay_shutdown
            && Instant::now() >= relay_reconnect_after
        {
            match CaptureRelayClient::connect(relay_reconnect_cursor.clone()) {
                Ok(client) => {
                    eprintln!(
                        "Reattached to capture owner PID {} at transcript cursor {} / nudge cursor {}.",
                        client.discovery().owner_pid,
                        relay_reconnect_cursor.transcript_seq,
                        relay_reconnect_cursor.nudge_seq
                    );
                    relay_client = Some(client);
                }
                Err(error) => {
                    tracing::warn!(error = %error, "capture relay reconnect failed");
                    relay_reconnect_after = Instant::now() + Duration::from_secs(1);
                }
            }
        }

        if let Some(client) = relay_client.as_mut() {
            let mut relay_error = None;
            loop {
                match client.try_recv() {
                    Ok(Some(RelayFrame::Transcript { update, .. })) => match update {
                        RelayTranscriptUpdate::Superseded {
                            through_utterance_sequence,
                            ..
                        } => {
                            runner.retract_partials(session_epoch, through_utterance_sequence);
                            utterances.retain(|utterance| {
                                utterance.update_kind == TranscriptUpdateKind::Final
                                    || utterance.utterance_sequence > through_utterance_sequence
                            });
                        }
                        RelayTranscriptUpdate::Utterance {
                            mut utterance,
                            producer_latency_ms,
                            ..
                        } if utterance.update_kind == TranscriptUpdateKind::Partial => {
                            let Some(text) = normalize_copilot_partial_text(&utterance.text) else {
                                continue;
                            };
                            utterance.text = text;
                            utterance.source = "capture-relay".into();
                            let identity = (utterance.utterance_sequence, utterance.revision);
                            if latest_partial_identity_seen != Some(identity) {
                                runner.supersede_partial_revision(
                                    session_epoch,
                                    identity.0,
                                    identity.1,
                                );
                                latest_partial_identity_seen = Some(identity);
                            }
                            utterances.retain(|existing| {
                                existing.update_kind == TranscriptUpdateKind::Final
                                    || existing.utterance_sequence != utterance.utterance_sequence
                            });
                            utterances.push_back(utterance.clone());
                            trim_copilot_utterances(&mut utterances);

                            next_evidence_revision = next_evidence_revision.saturating_add(1);
                            let now = Instant::now();
                            let audio_received_at = now
                                .checked_sub(Duration::from_millis(producer_latency_ms))
                                .unwrap_or(now);
                            let request = CopilotRequest {
                                goal: goal.into(),
                                mode,
                                session_epoch,
                                evidence_revision: next_evidence_revision,
                                evidence_utterance_sequence: utterance.utterance_sequence,
                                evidence_utterance_revision: utterance.revision,
                                update_kind: TranscriptUpdateKind::Partial,
                                utterances: utterances.iter().cloned().collect(),
                                battle_card: battle_card.clone(),
                                strategy_state: StrategyState::empty(),
                            };
                            let _ = runner.submit_with_latency(
                                request,
                                PartialLatencySeed {
                                    session_epoch,
                                    utterance_sequence: utterance.utterance_sequence,
                                    utterance_revision: utterance.revision,
                                    audio_received_at,
                                    partial_published_at: now,
                                    trigger_at: now,
                                    context_ready_at: Instant::now(),
                                },
                            );
                        }
                        RelayTranscriptUpdate::Utterance { .. } => {
                            // Finals remain sourced from the existing durable
                            // event cursor below. The relay mirror is only the
                            // low-latency transport and reconnect cursor.
                        }
                    },
                    Ok(Some(RelayFrame::Shutdown { reason })) => {
                        eprintln!("Capture owner stopped: {reason}. Copilot is stopping too.");
                        relay_shutdown = true;
                        stop.store(true, Ordering::Release);
                        break;
                    }
                    Ok(Some(RelayFrame::Gap {
                        stream,
                        requested_after,
                        available_from,
                    })) => {
                        eprintln!(
                            "Copilot reattached after missing some {stream} updates (cursor {requested_after}; replay starts at {available_from})."
                        );
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(error) => {
                        relay_error = Some(error);
                        break;
                    }
                }
            }
            relay_reconnect_cursor = client.cursor();
            if let Some(error) = relay_error {
                eprintln!(
                    "Capture attachment was interrupted ({error}). Reconnecting without opening another microphone..."
                );
                relay_client = None;
                relay_reconnect_after = Instant::now() + Duration::from_millis(250);
            }
        }

        if let Some(subscriber) = partial_subscriber.as_mut() {
            if let Some(identity) = subscriber.latest_identity() {
                if latest_partial_identity_seen != Some(identity) {
                    runner.supersede_partial_revision(session_epoch, identity.0, identity.1);
                    latest_partial_identity_seen = Some(identity);
                }
            }

            while let Some(partial_event) = subscriber.try_recv() {
                match partial_event {
                    LivePartialEvent::Superseded(signal) => {
                        runner.retract_partials(
                            signal.session_epoch,
                            signal.through_utterance_sequence,
                        );
                        utterances.retain(|utterance| {
                            utterance.update_kind == TranscriptUpdateKind::Final
                                || utterance.utterance_sequence > signal.through_utterance_sequence
                        });
                    }
                    LivePartialEvent::Partial(partial) => {
                        if !subscriber.is_current(&partial) {
                            continue;
                        }
                        let Some(text) = normalize_copilot_partial_text(&partial.text) else {
                            continue;
                        };
                        let trigger_at = Instant::now();
                        utterances.retain(|utterance| {
                            utterance.update_kind == TranscriptUpdateKind::Final
                                || utterance.utterance_sequence != partial.utterance_sequence
                        });
                        let duration_ms = partial
                            .partial_published_at
                            .saturating_duration_since(partial.audio_received_at)
                            .as_millis()
                            .min(u64::MAX as u128) as u64;
                        utterances.push_back(CopilotUtterance {
                            utterance_sequence: partial.utterance_sequence,
                            revision: partial.revision,
                            update_kind: TranscriptUpdateKind::Partial,
                            source: "in-process-live".into(),
                            text,
                            speaker: partial.speaker,
                            speaker_verified: false,
                            offset_ms: partial.offset_ms,
                            duration_ms,
                        });
                        while utterances.len() > 24
                            || utterances.iter().map(|item| item.text.len()).sum::<usize>() > 6_000
                        {
                            utterances.pop_front();
                        }

                        next_evidence_revision = next_evidence_revision.saturating_add(1);
                        let request = CopilotRequest {
                            goal: goal.into(),
                            mode,
                            session_epoch,
                            evidence_revision: next_evidence_revision,
                            evidence_utterance_sequence: partial.utterance_sequence,
                            evidence_utterance_revision: partial.revision,
                            update_kind: TranscriptUpdateKind::Partial,
                            utterances: utterances.iter().cloned().collect(),
                            battle_card: battle_card.clone(),
                            strategy_state: StrategyState::empty(),
                        };
                        let context_ready_at = Instant::now();
                        let _ = runner.submit_with_latency(
                            request,
                            PartialLatencySeed {
                                session_epoch,
                                utterance_sequence: partial.utterance_sequence,
                                utterance_revision: partial.revision,
                                audio_received_at: partial.audio_received_at,
                                partial_published_at: partial.partial_published_at,
                                trigger_at,
                                context_ready_at,
                            },
                        );
                    }
                }
            }
        }

        for envelope in minutes_core::events::read_events_since_seq(cursor, None) {
            cursor = cursor.max(envelope.seq);
            let minutes_core::events::MinutesEvent::LiveUtteranceFinal {
                source,
                text,
                speaker,
                offset_ms,
                duration_ms,
                ..
            } = envelope.event
            else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }

            if let Some(finalized_partial_through) = utterances
                .iter()
                .filter(|utterance| utterance.update_kind == TranscriptUpdateKind::Partial)
                .map(|utterance| utterance.utterance_sequence)
                .max()
            {
                // The final is authoritative for the current in-process
                // utterance. Invalidate model work before building its prompt,
                // and never carry the provisional text into a final request.
                runner.retract_partials(session_epoch, finalized_partial_through);
                utterances.retain(|utterance| {
                    utterance.update_kind == TranscriptUpdateKind::Final
                        || utterance.utterance_sequence > finalized_partial_through
                });
            }

            utterances.push_back(CopilotUtterance {
                utterance_sequence: envelope.seq,
                revision: envelope.seq,
                update_kind: TranscriptUpdateKind::Final,
                source,
                text,
                // `live.utterance.final` does not carry independent identity
                // verification. Preserve the raw value for future evidence,
                // but force the model-facing label to "the other speaker".
                speaker,
                speaker_verified: false,
                offset_ms,
                duration_ms,
            });
            while utterances.len() > 24
                || utterances.iter().map(|item| item.text.len()).sum::<usize>() > 6_000
            {
                utterances.pop_front();
            }

            next_evidence_revision = next_evidence_revision.saturating_add(1).max(envelope.seq);
            let request = CopilotRequest {
                goal: goal.into(),
                mode,
                session_epoch,
                evidence_revision: next_evidence_revision,
                evidence_utterance_sequence: envelope.seq,
                evidence_utterance_revision: 1,
                update_kind: TranscriptUpdateKind::Final,
                utterances: utterances.iter().cloned().collect(),
                battle_card: battle_card.clone(),
                strategy_state: StrategyState::empty(),
            };
            let _ = runner.submit(request);
        }

        while let Some(event) = runner.try_recv() {
            match event {
                RunnerEvent::Nudge(nudge) => {
                    // Close the poll-to-render gap against the producer itself,
                    // not only the runner's last mutex state.
                    if !copilot_nudge_is_fresh_for_render(&nudge, partial_subscriber.as_ref()) {
                        continue;
                    }
                    render_copilot_nudge(&nudge, surface)?;
                    if external_capture || in_process_live {
                        if let Err(error) = CaptureRelayClient::publish_nudge(nudge.clone()) {
                            tracing::warn!(error = %error, "failed to publish nudge to capture relay");
                        }
                    }
                }
                RunnerEvent::Degraded { error } => {
                    tracing::debug!(%error, "Coach suggestion failed");
                    eprintln!("{}.", CopilotState::Degraded.user_message());
                }
                RunnerEvent::DepthDegraded { error } => {
                    eprintln!(
                        "Copilot depth lane degraded: {error}. Fast coaching and capture continue unaffected."
                    );
                }
                RunnerEvent::RequestCancelled { .. }
                | RunnerEvent::EvidenceRetracted { .. }
                | RunnerEvent::StateChanged(_)
                | RunnerEvent::Model(_)
                | RunnerEvent::TopicShiftDetected { .. }
                | RunnerEvent::GroundingRefreshed { .. }
                | RunnerEvent::StrategyUpdated { .. }
                | RunnerEvent::PolicyAdjusted(_) => {}
            }
        }

        runner.tick(chrono::Utc::now());
        if last_status_write.elapsed() >= Duration::from_secs(1) {
            status.cursor = cursor;
            status.relay_cursor = relay_client.as_ref().map(CaptureRelayClient::cursor);
            status.health = runner.health();
            status.updated_ts = chrono::Utc::now();
            status.capture_attachment = if in_process_live || external_capture {
                capture_attachment.clone()
            } else {
                copilot_capture_attachment()
            };
            if let Err(error) = minutes_core::copilot::write_session_status(&status) {
                tracing::debug!(error = %error, "failed to update copilot status sidecar");
            }
            last_status_write = std::time::Instant::now();
        }

        std::thread::sleep(Duration::from_millis(200));
    }

    if capture_guard.stop_and_join().is_err() && capture_failure.is_none() {
        capture_failure = Some(anyhow::anyhow!("in-process live capture worker panicked"));
    }
    runner.stop();
    status.active = false;
    status.pid = None;
    status.health = runner.health();
    status.health.state = CopilotState::Off;
    status.updated_ts = chrono::Utc::now();
    minutes_core::copilot::write_session_status(&status)?;
    minutes_core::copilot::clear_session_controls()?;
    drop(session_guard);
    if in_process_live {
        eprintln!("Coach stopped. Its live capture was finalized safely.");
    } else {
        eprintln!("Coach stopped. Your recording was not changed.");
    }
    if let Some(error) = capture_failure {
        return Err(error);
    }
    Ok(())
}

fn render_copilot_nudge(nudge: &minutes_core::copilot::Nudge, surface: &str) -> Result<()> {
    let stdout = std::io::stdout();
    let terminal = stdout.is_terminal();
    let mut stdout = stdout.lock();
    render_copilot_nudge_to(nudge, surface, terminal, &mut stdout)
}

fn render_copilot_nudge_to(
    nudge: &minutes_core::copilot::Nudge,
    surface: &str,
    terminal: bool,
    writer: &mut dyn Write,
) -> Result<()> {
    if surface == "stdout" {
        writeln!(writer, "{}", serde_json::to_string(nudge)?)?;
        writer.flush()?;
        return Ok(());
    }

    writeln!(writer, "{}", format_copilot_nudge(nudge, terminal))?;
    writer.flush()?;
    Ok(())
}

fn copilot_nudge_is_fresh_for_render(
    nudge: &minutes_core::copilot::Nudge,
    partial_subscriber: Option<&minutes_core::live_partials::LivePartialSubscriber>,
) -> bool {
    nudge
        .grounded_partial_identity()
        .is_none_or(|(sequence, revision)| {
            partial_subscriber.is_some_and(|subscriber| {
                subscriber.is_identity_current(nudge.session_epoch, sequence, revision)
            })
        })
}

fn normalize_copilot_partial_text(text: &str) -> Option<String> {
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn trim_copilot_utterances(
    utterances: &mut std::collections::VecDeque<minutes_core::copilot::CopilotUtterance>,
) {
    while utterances.len() > 24
        || utterances.iter().map(|item| item.text.len()).sum::<usize>() > 6_000
    {
        utterances.pop_front();
    }
}

fn connect_capture_relay_with_retry(
    cursor: minutes_core::copilot::RelayCursor,
    timeout: Duration,
) -> Result<minutes_core::copilot::CaptureRelayClient, minutes_core::copilot::CaptureRelayError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match minutes_core::copilot::CaptureRelayClient::connect(cursor.clone()) {
            Ok(client) => return Ok(client),
            Err(error) if std::time::Instant::now() < deadline => {
                tracing::debug!(error = %error, "waiting for capture relay discovery");
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(error) => return Err(error),
        }
    }
}

fn copilot_external_capture_active() -> bool {
    minutes_core::pid::inspect_pid_file(&minutes_core::pid::pid_path()).is_active()
        || minutes_core::pid::inspect_pid_file(&minutes_core::pid::live_transcript_pid_path())
            .is_active()
}

fn copilot_evidence_mode(
    own_live_capture: bool,
    external_capture: bool,
    config: &Config,
) -> minutes_core::copilot::CopilotEvidenceMode {
    if own_live_capture
        && !external_capture
        && config.copilot.live_partials
        && config
            .effective_live_transcript_backend()
            .eq_ignore_ascii_case("whisper")
    {
        minutes_core::copilot::CopilotEvidenceMode::InProcessPartials
    } else {
        minutes_core::copilot::CopilotEvidenceMode::FinalOnly
    }
}

fn copilot_nudge_label(kind: minutes_core::copilot::NudgeKind) -> &'static str {
    use minutes_core::copilot::NudgeKind;

    match kind {
        NudgeKind::Say => "Try saying",
        NudgeKind::Ask => "Ask",
        NudgeKind::Clarify => "Clarify",
        NudgeKind::Hold => "Give it a moment",
        NudgeKind::Watch => "Keep an eye on",
    }
}

fn format_copilot_nudge(nudge: &minutes_core::copilot::Nudge, terminal: bool) -> String {
    let label = copilot_nudge_label(nudge.kind);
    if terminal {
        format!(
            "\n\x1b[1;36m┌ {label}\x1b[0m  \x1b[2m{}\x1b[0m\n\x1b[1m│ {}\x1b[0m\n\x1b[2m└ Based on what was just said\x1b[0m",
            nudge.source_chip, nudge.text
        )
    } else {
        format!("{label}: {} — Based on: {}", nudge.text, nudge.source_chip)
    }
}

fn format_copilot_setup_needed(setup: &minutes_core::copilot::CopilotSetupNeeded) -> String {
    format!(
        "{}\n\n{}:\n  {}\n\nThen start Coach again.",
        setup.message, setup.action.label, setup.action.command
    )
}

const COACH_WAITING_FOR_RECORDING: &str = "Coach is waiting for a recording to start.";

fn copilot_capture_attachment() -> String {
    let recording = minutes_core::pid::inspect_pid_file(&minutes_core::pid::pid_path());
    let live = minutes_core::pid::inspect_pid_file(&minutes_core::pid::live_transcript_pid_path());
    if recording.is_active() {
        return "Coach is listening to the current recording.".into();
    }
    if live.is_active() {
        return "Coach is listening to the live meeting.".into();
    }
    COACH_WAITING_FOR_RECORDING.into()
}

fn cmd_copilot_status() -> Result<()> {
    let mut status = minutes_core::copilot::read_session_status();
    if status.active {
        status.capture_attachment = copilot_capture_attachment();
    }
    println!("{}", format_copilot_status(&status));
    Ok(())
}

fn format_copilot_status(status: &minutes_core::copilot::CopilotSessionStatus) -> String {
    let mut lines = vec![format!("Coach status: {}.", status.user_summary())];
    if let Some(setup) = &status.setup_needed {
        lines.push(String::new());
        lines.push(format_copilot_setup_needed(setup));
        return lines.join("\n");
    }
    if !status.active {
        return lines.join("\n");
    }
    if !status.goal.trim().is_empty() {
        lines.push(format!("Meeting goal: {}", status.goal));
    }
    if let Some(message) = status.user_model_summary() {
        lines.push(message.into());
    }
    if let Some(message) = status.input_mode.user_message() {
        lines.push(message.into());
    }
    if !status.capture_attachment.trim().is_empty() {
        lines.push(status.capture_attachment.clone());
    }
    lines.join("\n")
}

fn cmd_copilot_pause() -> Result<()> {
    if !minutes_core::copilot::read_session_status().active {
        anyhow::bail!("Coach is not running");
    }
    minutes_core::copilot::request_pause()?;
    eprintln!("Coach will pause. Your recording continues.");
    Ok(())
}

fn cmd_copilot_resume() -> Result<()> {
    if !minutes_core::copilot::read_session_status().active {
        anyhow::bail!("Coach is not running");
    }
    minutes_core::copilot::request_resume()?;
    eprintln!("Coach will start listening again.");
    Ok(())
}

fn cmd_copilot_stop() -> Result<()> {
    if !minutes_core::copilot::read_session_status().active {
        eprintln!("Coach is already off.");
        return Ok(());
    }
    minutes_core::copilot::request_stop()?;
    eprintln!("Coach will stop. Your recording continues.");
    Ok(())
}

fn cmd_copilot_setup(config: &mut Config, forced_model: Option<&str>, retune: bool) -> Result<()> {
    use minutes_core::config::{
        decide_copilot_model, CopilotModelDecision, CopilotModelProbeResult,
    };

    let base_url = copilot_ollama_base_url();
    let forced_model = forced_model.map(str::trim);
    if forced_model.is_some_and(str::is_empty) {
        anyhow::bail!("Choose a model tag after `--model`.");
    }

    eprintln!("Checking Coach's private AI on this computer...");
    let initial_probe = probe_ollama_models(&base_url, OLLAMA_PROBE_TIMEOUT);

    // Preserve setup for explicitly selected non-Ollama implementations. The
    // Ollama API probe still happens first so an installed app is never
    // mistaken for missing just because its CLI is not on PATH.
    let requested_provider = match config.copilot.resolved_fast_provider() {
        "auto-local" => None,
        provider => Some(provider),
    };
    if forced_model.is_none()
        && !retune
        && requested_provider.is_some_and(|provider| provider != "ollama")
    {
        return finish_copilot_setup(&config.copilot);
    }

    let mut models = match initial_probe {
        Ok(models) => models,
        Err(error) => {
            tracing::debug!(%error, %base_url, "Coach could not reach the Ollama API");
            let cli_present = command_is_available("ollama");
            let brew_present = command_is_available("brew");
            match decide_copilot_setup(false, false, brew_present, cli_present) {
                CopilotSetupAction::StartOllama => {
                    eprintln!("Starting Coach's private AI...");
                    start_ollama(cli_present, brew_present)?;
                }
                CopilotSetupAction::InstallWithBrew => {
                    eprintln!("Installing the free private AI Coach uses...");
                    run_copilot_setup_step(
                        "brew",
                        &["install", "ollama"],
                        "Coach could not install Ollama. Download the free app at https://ollama.com/download, open it once, then run `minutes coach setup` again",
                    )?;
                    if !command_is_available("ollama") {
                        anyhow::bail!(ollama_download_guidance());
                    }
                    eprintln!("Starting Coach's private AI...");
                    start_ollama(true, true)?;
                }
                CopilotSetupAction::DownloadGuidance => {
                    anyhow::bail!(ollama_download_guidance());
                }
                CopilotSetupAction::Ready | CopilotSetupAction::PullModel => {
                    unreachable!("an unreachable API cannot be ready or pull a model")
                }
            }

            wait_for_ollama(&base_url, OLLAMA_START_TIMEOUT).ok_or_else(|| {
                anyhow::anyhow!(
                    "Coach's private AI did not start. Open Ollama once, then run `minutes coach setup` again. If Ollama is not installed, download it at https://ollama.com/download"
                )
            })?
        }
    };

    if forced_model.is_some() || retune {
        config.copilot.fast_provider = "ollama".into();
    }
    let user_override = configured_copilot_model_override(config, forced_model, retune);
    let hardware = if user_override.is_none() {
        let hardware = detect_coach_hardware()?;
        eprintln!(
            "Found {} GB of memory{}.",
            hardware.ram_gb,
            if hardware.apple_silicon {
                " and Apple silicon"
            } else {
                ""
            }
        );
        hardware
    } else {
        CoachHardware {
            ram_gb: 0,
            apple_silicon: false,
        }
    };
    let mut probe_results = Vec::<CopilotModelProbeResult>::new();

    loop {
        match decide_copilot_model(
            hardware.ram_gb,
            hardware.apple_silicon,
            user_override.as_deref(),
            &probe_results,
        ) {
            CopilotModelDecision::UserOverride { model_tag } => {
                eprintln!("Keeping your chosen Coach model, {model_tag}.");
                ensure_copilot_model_installed(&base_url, &model_tag, &mut models)?;
                let probe =
                    probe_copilot_model(&base_url, &model_tag, config.copilot.target_latency_ms)?;
                if !probe.within_budget {
                    eprintln!(
                        "Your chosen model is ready, though its test took {} ms (first response {} ms).",
                        probe.total_ms, probe.ttft_ms
                    );
                }
                persist_copilot_model(config, &model_tag)?;
                return finish_copilot_setup(&config.copilot);
            }
            CopilotModelDecision::ProbeManifest {
                tier,
                model_tag,
                approx_download_gb,
            } => {
                eprintln!(
                    "Your computer can run a stronger private AI. Setting up the {tier} model (~{approx_download_gb} GB download)…"
                );
                ensure_copilot_model_installed(&base_url, model_tag, &mut models)?;
                eprintln!("Checking how quickly {model_tag} can coach...");
                let probe = match probe_copilot_model(
                    &base_url,
                    model_tag,
                    config.copilot.target_latency_ms,
                ) {
                    Ok(probe) => probe,
                    Err(error) => {
                        tracing::debug!(%error, %model_tag, "Coach manifest model probe failed");
                        eprintln!("That model could not complete Coach's response test. Trying a smaller one...");
                        probe_results.push(CopilotModelProbeResult {
                            model_tag: model_tag.into(),
                            within_budget: false,
                        });
                        continue;
                    }
                };
                eprintln!(
                    "  First response: {} ms; complete nudge: {} ms.",
                    probe.ttft_ms, probe.total_ms
                );
                probe_results.push(CopilotModelProbeResult {
                    model_tag: model_tag.into(),
                    within_budget: probe.within_budget,
                });
                if !probe.within_budget {
                    eprintln!("That model was slower than Coach's target. Trying a smaller one...");
                }
            }
            CopilotModelDecision::SelectedManifest {
                tier, model_tag, ..
            } => {
                persist_copilot_model(config, model_tag)?;
                eprintln!("Coach selected the {tier} model for this computer.");
                return finish_copilot_setup(&config.copilot);
            }
            CopilotModelDecision::NoManifestModelWithinBudget => {
                anyhow::bail!(
                    "None of the reviewed Coach models met the response-time target on this computer. Use `minutes coach setup --model <tag>` to keep a model you choose."
                );
            }
        }
    }
}

const OLLAMA_DEFAULT_BASE_URL: &str = "http://localhost:11434";
const OLLAMA_DOWNLOAD_URL: &str = "https://ollama.com/download";
const OLLAMA_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const OLLAMA_START_TIMEOUT: Duration = Duration::from_secs(30);
const OLLAMA_SETUP_PREWARM_TIMEOUT: Duration = Duration::from_secs(120);
const COACH_INSTANT_TTFT_MS: u64 = 1_500;
const BYTES_PER_GIB: u64 = 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CoachHardware {
    ram_gb: u64,
    apple_silicon: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CoachLatencyProbe {
    ttft_ms: u64,
    total_ms: u64,
    within_budget: bool,
}

fn configured_copilot_model_override(
    config: &Config,
    forced_model: Option<&str>,
    retune: bool,
) -> Option<String> {
    use minutes_core::config::{copilot_manifest_contains, LEGACY_COPILOT_MODEL};

    if let Some(model) = forced_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        return Some(model.to_string());
    }
    if retune {
        return None;
    }
    let configured = config.copilot.fast_model.trim();
    let is_legacy = strip_latest_tag(configured).eq_ignore_ascii_case(LEGACY_COPILOT_MODEL);
    (!configured.is_empty() && !is_legacy && !copilot_manifest_contains(configured))
        .then(|| configured.to_string())
}

fn detect_coach_hardware() -> Result<CoachHardware> {
    let total_bytes = detect_total_memory_bytes()?;
    if total_bytes == 0 {
        anyhow::bail!("Coach could not determine this computer's memory.");
    }
    Ok(CoachHardware {
        ram_gb: total_bytes.div_ceil(BYTES_PER_GIB),
        apple_silicon: detect_apple_silicon()?,
    })
}

#[cfg(target_os = "macos")]
fn sysctl_value(name: &str) -> Result<String> {
    let output = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", name])
        .stdin(std::process::Stdio::null())
        .output()?;
    if !output.status.success() {
        anyhow::bail!("sysctl {name} failed");
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

#[cfg(target_os = "macos")]
fn detect_total_memory_bytes() -> Result<u64> {
    sysctl_value("hw.memsize")?
        .parse::<u64>()
        .map_err(Into::into)
}

#[cfg(target_os = "macos")]
fn detect_apple_silicon() -> Result<bool> {
    Ok(is_apple_silicon_brand(&sysctl_value(
        "machdep.cpu.brand_string",
    )?))
}

#[cfg(any(target_os = "macos", test))]
fn is_apple_silicon_brand(brand: &str) -> bool {
    brand.trim().starts_with("Apple M")
}

#[cfg(target_os = "linux")]
fn detect_total_memory_bytes() -> Result<u64> {
    parse_linux_meminfo_bytes(&std::fs::read_to_string("/proc/meminfo")?)
        .ok_or_else(|| anyhow::anyhow!("/proc/meminfo did not contain MemTotal"))
}

#[cfg(any(target_os = "linux", test))]
fn parse_linux_meminfo_bytes(contents: &str) -> Option<u64> {
    let kib = contents.lines().find_map(|line| {
        let value = line.strip_prefix("MemTotal:")?.trim();
        value.split_whitespace().next()?.parse::<u64>().ok()
    })?;
    kib.checked_mul(1024)
}

#[cfg(target_os = "linux")]
fn detect_apple_silicon() -> Result<bool> {
    Ok(false)
}

#[cfg(windows)]
fn detect_total_memory_bytes() -> Result<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(status.ullTotalPhys)
}

#[cfg(windows)]
fn detect_apple_silicon() -> Result<bool> {
    Ok(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn detect_total_memory_bytes() -> Result<u64> {
    anyhow::bail!("Coach hardware detection is not available on this platform")
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn detect_apple_silicon() -> Result<bool> {
    Ok(false)
}

fn ensure_copilot_model_installed(
    base_url: &str,
    model_name: &str,
    models: &mut Vec<String>,
) -> Result<()> {
    match decide_copilot_setup(
        true,
        ollama_model_is_present(models, model_name),
        false,
        false,
    ) {
        CopilotSetupAction::Ready => Ok(()),
        CopilotSetupAction::PullModel => {
            eprintln!(
                "Downloading {model_name} for Coach. It runs on your computer, and nothing leaves your machine."
            );
            pull_ollama_model(base_url, model_name)?;
            *models = wait_for_ollama(base_url, Duration::from_secs(5)).ok_or_else(|| {
                anyhow::anyhow!(
                    "Coach downloaded {model_name}, but its private AI stopped responding. Open Ollama, then run `minutes coach setup` again"
                )
            })?;
            if !ollama_model_is_present(models, model_name) {
                anyhow::bail!(
                    "Coach could not find {model_name} after downloading it. Open Ollama, then run `minutes coach setup` again"
                );
            }
            Ok(())
        }
        CopilotSetupAction::StartOllama
        | CopilotSetupAction::InstallWithBrew
        | CopilotSetupAction::DownloadGuidance => {
            unreachable!("a reachable API only needs a model decision")
        }
    }
}

fn probe_copilot_model(
    base_url: &str,
    model_name: &str,
    target_latency_ms: u64,
) -> Result<CoachLatencyProbe> {
    use minutes_core::copilot::{
        BattleCard, CancelToken, CopilotModel, CopilotRequest, CopilotUtterance, MeetingMode,
        ModelStreamEvent, OllamaCopilotModel, StrategyState, StrategyStateDraft,
        TranscriptUpdateKind,
    };
    use std::sync::{Arc, Mutex};

    let model = OllamaCopilotModel::new(base_url, model_name, OLLAMA_SETUP_PREWARM_TIMEOUT);
    model
        .prewarm()
        .map_err(|error| anyhow::anyhow!("Coach could not warm {model_name}: {error}"))?;

    let utterances = [
        "We need to choose the enterprise price today: $120k flat or $90k plus usage.",
        "Procurement says anything above $100k needs CFO approval and adds six weeks.",
        "The remaining decision is the $90k first-year offer and who gets approval before Thursday.",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, text)| CopilotUtterance {
        utterance_sequence: (index + 1) as u64,
        revision: (index + 1) as u64,
        update_kind: TranscriptUpdateKind::Final,
        source: "coach.setup.probe".into(),
        text: text.into(),
        speaker: None,
        speaker_verified: false,
        offset_ms: index as u64 * 10_000,
        duration_ms: 9_000,
    })
    .collect();
    let request = CopilotRequest {
        goal: "Leave with a pricing decision, an owner, and a dated follow-up.".into(),
        mode: MeetingMode::Decision,
        session_epoch: 1,
        evidence_revision: 3,
        evidence_utterance_sequence: 3,
        evidence_utterance_revision: 3,
        update_kind: TranscriptUpdateKind::Final,
        utterances,
        battle_card: BattleCard {
            rendered: "Procurement needs a named owner before Thursday.".into(),
            ..BattleCard::default()
        },
        strategy_state: StrategyState::from_draft(
            StrategyStateDraft {
                open_threads: vec!["Choose the enterprise price.".into()],
                unmet_goal_items: vec!["No owner or follow-up date is agreed.".into()],
                unresolved_objections: vec!["$120k exceeds the budget envelope.".into()],
                steer_toward: vec!["Close the price, owner, and date.".into()],
            },
            3,
        ),
    };

    let first_token = Arc::new(Mutex::new(None::<std::time::Instant>));
    let first_token_sink = Arc::clone(&first_token);
    let started = std::time::Instant::now();
    let sink = move |event: ModelStreamEvent| {
        if matches!(event, ModelStreamEvent::TextDelta(ref text) if !text.is_empty()) {
            let mut observed = first_token_sink.lock().unwrap();
            observed.get_or_insert_with(std::time::Instant::now);
        }
    };
    model
        .stream_structured(&request, &CancelToken::new(), &sink)
        .map_err(|error| {
            anyhow::anyhow!("Coach model {model_name} failed its response test: {error}")
        })?;
    let completed = std::time::Instant::now();
    let total_ms = duration_ms(completed.duration_since(started));
    let ttft_ms = first_token
        .lock()
        .unwrap()
        .map(|observed| duration_ms(observed.duration_since(started)))
        .unwrap_or(total_ms);
    let total_budget_ms = target_latency_ms.max(250);
    let ttft_budget_ms = total_budget_ms.min(COACH_INSTANT_TTFT_MS);
    Ok(CoachLatencyProbe {
        ttft_ms,
        total_ms,
        within_budget: ttft_ms <= ttft_budget_ms && total_ms <= total_budget_ms,
    })
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

fn persist_copilot_model(config: &mut Config, model_name: &str) -> Result<()> {
    config.copilot.fast_model = model_name.into();
    config.save().map_err(|error| {
        anyhow::anyhow!(
            "Coach selected {model_name}, but could not save it to {}: {error}",
            Config::config_path().display()
        )
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopilotSetupAction {
    Ready,
    PullModel,
    StartOllama,
    InstallWithBrew,
    DownloadGuidance,
}

fn decide_copilot_setup(
    api_reachable: bool,
    model_present: bool,
    brew_present: bool,
    cli_present: bool,
) -> CopilotSetupAction {
    if api_reachable {
        if model_present {
            CopilotSetupAction::Ready
        } else {
            CopilotSetupAction::PullModel
        }
    } else if cli_present {
        CopilotSetupAction::StartOllama
    } else if brew_present {
        CopilotSetupAction::InstallWithBrew
    } else {
        CopilotSetupAction::DownloadGuidance
    }
}

#[derive(Debug, Deserialize)]
struct OllamaTagsResponse {
    #[serde(default)]
    models: Vec<OllamaTag>,
}

#[derive(Debug, Deserialize)]
struct OllamaTag {
    #[serde(default)]
    name: String,
    #[serde(default)]
    model: String,
}

#[derive(Debug, Deserialize)]
struct OllamaPullFrame {
    #[serde(default)]
    status: String,
    completed: Option<u64>,
    total: Option<u64>,
    error: Option<String>,
}

fn copilot_ollama_base_url() -> String {
    std::env::var("OLLAMA_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| OLLAMA_DEFAULT_BASE_URL.into())
        .trim_end_matches('/')
        .to_string()
}

fn ollama_probe_agent(timeout: Duration) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .build(),
    )
}

fn ollama_pull_agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .http_status_as_error(false)
            .build(),
    )
}

fn probe_ollama_models(
    base_url: &str,
    timeout: Duration,
) -> std::result::Result<Vec<String>, String> {
    let url = format!("{base_url}/api/tags");
    let mut response = ollama_probe_agent(timeout)
        .get(&url)
        .call()
        .map_err(|error| error.to_string())?;
    if response.status().as_u16() != 200 {
        let status = response.status().as_u16();
        let body = response.body_mut().read_to_string().unwrap_or_default();
        return Err(format!("Ollama HTTP {status}: {body}"));
    }
    let body = response
        .body_mut()
        .read_to_string()
        .map_err(|error| error.to_string())?;
    parse_ollama_model_tags(&body)
}

fn parse_ollama_model_tags(body: &str) -> std::result::Result<Vec<String>, String> {
    let response: OllamaTagsResponse =
        serde_json::from_str(body).map_err(|error| error.to_string())?;
    let mut models = Vec::new();
    for tag in response.models {
        for name in [tag.name, tag.model] {
            let name = name.trim();
            if !name.is_empty() && !models.iter().any(|existing| existing == name) {
                models.push(name.to_string());
            }
        }
    }
    Ok(models)
}

fn ollama_model_is_present(models: &[String], requested: &str) -> bool {
    let requested = requested.trim();
    models.iter().any(|available| {
        let available = available.trim();
        available == requested
            || strip_latest_tag(available).eq_ignore_ascii_case(strip_latest_tag(requested))
    })
}

fn strip_latest_tag(model: &str) -> &str {
    model.strip_suffix(":latest").unwrap_or(model)
}

fn command_is_available(program: &str) -> bool {
    std::process::Command::new(program)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
}

fn start_ollama(cli_present: bool, brew_present: bool) -> Result<()> {
    if cli_present && brew_present {
        let brew_start = run_copilot_setup_step(
            "brew",
            &["services", "start", "ollama"],
            "Homebrew could not start Ollama",
        );
        if brew_start.is_ok() {
            return Ok(());
        }
        tracing::debug!("Homebrew could not start Ollama; trying the Ollama app directly");
    }

    std::process::Command::new("ollama")
        .arg("serve")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| {
            tracing::debug!(%error, "Coach could not start Ollama directly");
            anyhow::anyhow!(
                "Coach could not start its private AI. Open Ollama once, then run `minutes coach setup` again"
            )
        })
}

fn wait_for_ollama(base_url: &str, timeout: Duration) -> Option<Vec<String>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Ok(models) = probe_ollama_models(base_url, OLLAMA_PROBE_TIMEOUT) {
            return Some(models);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

fn pull_ollama_model(base_url: &str, model_name: &str) -> Result<()> {
    let url = format!("{base_url}/api/pull");
    let body = serde_json::json!({
        "name": model_name,
        "stream": true,
    });
    let mut response = ollama_pull_agent()
        .post(&url)
        .header("Content-Type", "application/json")
        .send_json(&body)
        .map_err(|error| {
            tracing::debug!(%error, "Coach model download request failed");
            anyhow::anyhow!(
                "Coach could not download {model_name}. Check your internet connection, then run `minutes coach setup` again"
            )
        })?;
    if response.status().as_u16() >= 400 {
        let status = response.status().as_u16();
        let response_body = response.body_mut().read_to_string().unwrap_or_default();
        tracing::debug!(status, body = %response_body, "Coach model download failed");
        anyhow::bail!(
            "Coach could not download {model_name}. Check your internet connection, then run `minutes coach setup` again"
        );
    }

    let mut response_body = response.into_body();
    let reader = BufReader::new(response_body.as_reader());
    let mut last_status = String::new();
    let mut last_percent = None;
    let mut succeeded = false;
    for line in reader.lines() {
        let line = line.map_err(|error| {
            tracing::debug!(%error, "Coach model download stream ended unexpectedly");
            anyhow::anyhow!(
                "Coach's download was interrupted. Check your internet connection, then run `minutes coach setup` again"
            )
        })?;
        if line.trim().is_empty() {
            continue;
        }
        let frame: OllamaPullFrame = serde_json::from_str(&line).map_err(|error| {
            tracing::debug!(%error, %line, "Coach received invalid Ollama download progress");
            anyhow::anyhow!("Coach received an invalid response while downloading {model_name}")
        })?;
        if let Some(error) = frame
            .error
            .as_deref()
            .filter(|error| !error.trim().is_empty())
        {
            tracing::debug!(%error, "Ollama rejected the Coach model download");
            anyhow::bail!("Coach could not download {model_name}: {error}");
        }
        report_ollama_pull_progress(&frame, &mut last_status, &mut last_percent);
        succeeded |= frame.status.eq_ignore_ascii_case("success");
    }

    if !succeeded {
        anyhow::bail!(
            "Coach's download ended before {model_name} was ready. Run `minutes coach setup` again"
        );
    }
    Ok(())
}

fn report_ollama_pull_progress(
    frame: &OllamaPullFrame,
    last_status: &mut String,
    last_percent: &mut Option<u64>,
) {
    let percent = frame
        .completed
        .zip(frame.total)
        .filter(|(_, total)| *total > 0)
        .map(|(completed, total)| completed.saturating_mul(100) / total);
    let status_changed = !frame.status.is_empty() && frame.status != *last_status;
    let percent_changed = percent.is_some_and(|percent| {
        last_percent.is_none_or(|previous| percent == 100 || percent >= previous.saturating_add(5))
    });
    if status_changed || percent_changed {
        if let Some(percent) = percent {
            eprintln!("  {}: {percent}%", frame.status);
        } else if !frame.status.is_empty() {
            eprintln!("  {}", frame.status);
        }
        *last_status = frame.status.clone();
        *last_percent = percent;
    }
}

fn ollama_download_guidance() -> String {
    format!(
        "Coach needs the free Ollama app for its private AI. Download it at {OLLAMA_DOWNLOAD_URL}, open it once, then run `minutes coach setup` again. Coach runs on your Mac, and nothing leaves your machine."
    )
}

fn finish_copilot_setup(config: &minutes_core::config::CopilotConfig) -> Result<()> {
    use minutes_core::copilot::{
        AppleFoundationCopilotModel, CloudCopilotModel, CopilotModel, FastModelRoute,
        OllamaCopilotModel, RoutingPolicy,
    };
    use std::sync::Arc;

    let candidates: Vec<Arc<dyn CopilotModel>> = vec![
        Arc::new(OllamaCopilotModel::new(
            copilot_ollama_base_url(),
            config.fast_model.clone(),
            OLLAMA_SETUP_PREWARM_TIMEOUT,
        )),
        Arc::new(AppleFoundationCopilotModel::new("apple-foundation-model")),
        Arc::new(CloudCopilotModel::new(config.fast_model.clone())),
    ];
    let requested_provider = match config.resolved_fast_provider() {
        "auto-local" => None,
        provider => Some(provider),
    };
    match minutes_core::copilot::route_fast_model(
        candidates,
        requested_provider,
        RoutingPolicy::local_first(config.allow_cloud, 4_096, config.target_latency_ms),
    ) {
        FastModelRoute::Selected { model, detail, .. } => {
            tracing::debug!(%detail, "Coach setup found a ready model");
            if model.provider_name() != "ollama" || model.prewarm().is_ok() {
                clear_copilot_setup_needed()?;
                eprintln!("Coach is ready to use.");
                return Ok(());
            }
        }
        FastModelRoute::SetupRequired { message, probes } => {
            tracing::debug!(%message, ?probes, "Coach setup needs a local model");
            if requested_provider.is_some_and(|provider| provider != "ollama") {
                anyhow::bail!(
                    "The AI selected in Coach settings is not available in this build. Choose Private AI in Settings, then run `minutes coach setup` again"
                );
            }
        }
    }

    anyhow::bail!(
        "Coach's private AI is installed but not ready. Open Ollama, then run `minutes coach setup` again"
    )
}

fn clear_copilot_setup_needed() -> Result<()> {
    let mut status = minutes_core::copilot::read_session_status();
    status.setup_needed = None;
    status.health.last_error = None;
    status.updated_ts = chrono::Utc::now();
    minutes_core::copilot::write_session_status(&status)?;
    Ok(())
}

fn run_copilot_setup_step(program: &str, args: &[&str], user_error: &str) -> Result<()> {
    tracing::debug!(program, ?args, "running Coach setup step");
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|error| {
            tracing::debug!(program, ?args, %error, "Coach setup step could not start");
            anyhow::anyhow!(user_error.to_string())
        })?;
    if !output.status.success() {
        tracing::debug!(
            program,
            ?args,
            status = ?output.status,
            stdout = %String::from_utf8_lossy(&output.stdout),
            stderr = %String::from_utf8_lossy(&output.stderr),
            "Coach setup step failed"
        );
        anyhow::bail!(user_error.to_string());
    }
    Ok(())
}

fn cmd_copilot_feedback(nudge_id: &str, rating: &str) -> Result<()> {
    use minutes_core::copilot::{CopilotFeedback, CopilotFeedbackRequest};

    if !minutes_core::copilot::read_session_status().active {
        anyhow::bail!("no active copilot session to rate");
    }
    let feedback = match rating {
        "helpful" => CopilotFeedback::Helpful,
        "not-helpful" => CopilotFeedback::NotHelpful,
        "dismissed" => CopilotFeedback::Dismissed,
        _ => anyhow::bail!("unsupported feedback rating '{rating}'"),
    };
    minutes_core::copilot::request_feedback(&CopilotFeedbackRequest {
        nudge_id: nudge_id.trim().into(),
        feedback,
    })?;
    eprintln!("Feedback queued for this copilot session.");
    Ok(())
}

fn cmd_events(
    limit: usize,
    event_type: Option<String>,
    since: Option<String>,
    follow: bool,
    since_seq: Option<u64>,
    _config: &Config,
) -> Result<()> {
    if since.is_some() && since_seq.is_some() {
        anyhow::bail!("use either --since or --since-seq, not both");
    }

    if follow {
        return cmd_events_follow(limit, event_type, since, since_seq);
    }

    let since_dt = parse_events_since(since.as_deref())?;

    let mut events = if let Some(seq) = since_seq {
        minutes_core::events::read_events_since_seq(
            seq,
            if event_type.is_some() {
                None
            } else {
                Some(limit)
            },
        )
    } else {
        minutes_core::events::read_events(
            since_dt,
            if event_type.is_some() {
                None
            } else {
                Some(limit)
            },
        )
    };
    filter_events_by_type(&mut events, event_type.as_deref());
    apply_events_limit(&mut events, limit, since_seq.is_some());
    let json = serde_json::to_string_pretty(&events)?;
    println!("{}", json);
    Ok(())
}

fn cmd_events_follow(
    limit: usize,
    event_type: Option<String>,
    since: Option<String>,
    since_seq: Option<u64>,
) -> Result<()> {
    let since_dt = parse_events_since(since.as_deref())?;
    let mut cursor = since_seq.unwrap_or(0);

    let mut initial_events = if let Some(seq) = since_seq {
        minutes_core::events::read_events_since_seq(seq, None)
    } else {
        minutes_core::events::read_events(
            since_dt,
            if event_type.is_some() {
                None
            } else {
                Some(limit)
            },
        )
    };
    filter_events_by_type(&mut initial_events, event_type.as_deref());
    apply_events_limit(&mut initial_events, limit, since_seq.is_some());

    for event in &initial_events {
        cursor = cursor.max(event.seq);
        print_event_jsonl(event)?;
    }

    if since_seq.is_none() && initial_events.is_empty() {
        cursor = minutes_core::events::latest_event_seq();
    }

    loop {
        std::thread::sleep(Duration::from_millis(500));
        for event in minutes_core::events::read_events_since_seq(cursor, None) {
            cursor = cursor.max(event.seq);
            if event_matches_type(&event, event_type.as_deref()) {
                print_event_jsonl(&event)?;
            }
        }
    }
}

fn filter_events_by_type(
    events: &mut Vec<minutes_core::events::EventEnvelope>,
    event_type: Option<&str>,
) {
    if let Some(event_type) = event_type {
        events.retain(|event| event_matches_type(event, Some(event_type)));
    }
}

fn apply_events_limit(
    events: &mut Vec<minutes_core::events::EventEnvelope>,
    limit: usize,
    since_seq_mode: bool,
) {
    if events.len() <= limit {
        return;
    }
    if since_seq_mode {
        events.truncate(limit);
    } else {
        let skip = events.len().saturating_sub(limit);
        events.drain(0..skip);
    }
}

fn event_matches_type(
    event: &minutes_core::events::EventEnvelope,
    event_type: Option<&str>,
) -> bool {
    let Some(event_type) = event_type else {
        return true;
    };

    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("event_type").cloned())
        .and_then(|value| value.as_str().map(str::to_owned))
        .map(|actual| actual == event_type)
        .unwrap_or(false)
}

#[allow(clippy::too_many_arguments)]
fn cmd_agent_annotate(
    agent_id: String,
    tools: Vec<String>,
    subkind: String,
    meeting_id: Option<String>,
    meeting_path: Option<String>,
    span_start_ms: Option<u64>,
    span_end_ms: Option<u64>,
    body: String,
    citations: Vec<String>,
    confidence: String,
    provenance: Option<String>,
) -> Result<()> {
    use minutes_core::events::{
        append_agent_annotation, AgentAnnotationAgent, AgentAnnotationRequest, AgentAnnotationSpan,
        AgentAnnotationTarget,
    };

    let span = match (span_start_ms, span_end_ms) {
        (Some(start_ms), Some(end_ms)) => Some(AgentAnnotationSpan { start_ms, end_ms }),
        (None, None) => None,
        _ => {
            let error = serde_json::json!({
                "ok": false,
                "error": "invalid_payload",
                "message": "--span-start-ms and --span-end-ms must be provided together",
                "agent_id": agent_id,
                "event_type": minutes_core::events::AGENT_ANNOTATION_EVENT_TYPE,
                "allowlist_path": minutes_core::events::agents_allowlist_path().display().to_string()
            });
            eprintln!("{}", serde_json::to_string_pretty(&error)?);
            std::process::exit(2);
        }
    };

    let provenance = provenance
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| anyhow::anyhow!("invalid --provenance JSON: {error}"))?
        .unwrap_or_else(|| serde_json::json!({}));

    let request = AgentAnnotationRequest {
        agent: AgentAnnotationAgent {
            id: agent_id,
            tools,
        },
        subkind,
        target: AgentAnnotationTarget {
            meeting_id,
            meeting_path,
            span,
        },
        body,
        citations,
        confidence,
        provenance,
    };

    match append_agent_annotation(request) {
        Ok(envelope) => {
            println!("{}", serde_json::to_string_pretty(&envelope)?);
            Ok(())
        }
        Err(error) => {
            eprintln!("{}", serde_json::to_string_pretty(&error.to_body())?);
            std::process::exit(2);
        }
    }
}

fn parse_events_since(raw: Option<&str>) -> Result<Option<chrono::DateTime<Local>>> {
    let Some(raw) = raw else {
        return Ok(None);
    };

    if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Ok(Some(parsed.with_timezone(&Local)));
    }

    if let Ok(date) = chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Ok(date
            .and_hms_opt(0, 0, 0)
            .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single()));
    }

    Err(anyhow::anyhow!(
        "invalid --since value '{}' (expected YYYY-MM-DD or RFC3339)",
        raw
    ))
}

fn print_event_jsonl(event: &minutes_core::events::EventEnvelope) -> Result<()> {
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{}", serde_json::to_string(event)?)?;
    stdout.flush()?;
    Ok(())
}

fn cmd_insights(
    kind: Option<String>,
    confidence: Option<String>,
    participant: Option<String>,
    since: Option<String>,
    limit: usize,
    actionable: bool,
) -> Result<()> {
    use minutes_core::events::{InsightConfidence, InsightFilter, InsightKind};

    let since_dt = if let Some(s) = since.as_deref() {
        match chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(d) => d
                .and_hms_opt(0, 0, 0)
                .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single()),
            Err(e) => {
                eprintln!("warning: invalid date '{}' (expected YYYY-MM-DD): {}", s, e);
                None
            }
        }
    } else {
        None
    };

    let kind_filter = match kind.as_deref() {
        Some("decision") => Some(InsightKind::Decision),
        Some("commitment") => Some(InsightKind::Commitment),
        Some("question") => Some(InsightKind::Question),
        Some(other) => {
            eprintln!("warning: unknown insight kind '{}', showing all", other);
            None
        }
        None => None,
    };

    let min_confidence = if actionable {
        Some(InsightConfidence::Strong)
    } else {
        confidence.as_deref().map(|c| match c {
            "tentative" => InsightConfidence::Tentative,
            "inferred" => InsightConfidence::Inferred,
            "strong" => InsightConfidence::Strong,
            "explicit" => InsightConfidence::Explicit,
            other => {
                eprintln!("warning: unknown confidence '{}', showing all", other);
                InsightConfidence::Tentative
            }
        })
    };

    let filter = InsightFilter {
        kind: kind_filter,
        min_confidence,
        participant,
        since: since_dt,
        limit: Some(limit),
    };

    let insights = minutes_core::events::read_insights(&filter);
    let output: Vec<serde_json::Value> = insights
        .into_iter()
        .map(|(ts, insight, meeting_title)| {
            serde_json::json!({
                "timestamp": ts.to_rfc3339(),
                "meeting_title": meeting_title,
                "kind": insight.kind,
                "content": insight.content,
                "confidence": insight.confidence,
                "participants": insight.participants,
                "owner": insight.owner,
                "deadline": insight.deadline,
                "topic": insight.topic,
                "source_meeting": insight.source_meeting,
            })
        })
        .collect();

    let json = serde_json::to_string_pretty(&output)?;
    println!("{}", json);
    Ok(())
}

fn cmd_context(action: ContextAction) -> Result<()> {
    match action {
        ContextAction::Status {
            session,
            path,
            json,
        } => cmd_context_status(session.as_deref(), path.as_deref(), json),
        ContextAction::Screen {
            session,
            path,
            at,
            limit,
            json,
        } => cmd_context_screen(
            session.as_deref(),
            path.as_deref(),
            at.as_deref(),
            limit,
            json,
        ),
        ContextAction::ActivitySummary {
            session,
            path,
            start,
            end,
            json,
        } => cmd_context_activity_summary(
            session.as_deref(),
            path.as_deref(),
            start.as_deref(),
            end.as_deref(),
            json,
        ),
        ContextAction::Search { query, limit, json } => cmd_context_search(&query, limit, json),
        ContextAction::GetMoment {
            session,
            path,
            at,
            before_minutes,
            after_minutes,
            json,
        } => cmd_context_get_moment(
            session.as_deref(),
            path.as_deref(),
            at.as_deref(),
            before_minutes,
            after_minutes,
            json,
        ),
    }
}

fn parse_rfc3339_local(raw: &str) -> Result<chrono::DateTime<Local>> {
    let parsed = chrono::DateTime::parse_from_rfc3339(raw)?;
    Ok(parsed.with_timezone(&Local))
}

fn resolve_context_session(
    session: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<minutes_core::context_store::ContextSession>> {
    if let Some(session_id) = session {
        return Ok(minutes_core::context_store::get_session(session_id)?);
    }
    if let Some(path) = path {
        let canonical = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .display()
            .to_string();
        if let Some(session) = minutes_core::context_store::get_session_for_artifact(&canonical)? {
            return Ok(Some(session));
        }
        let original = path.display().to_string();
        return Ok(minutes_core::context_store::get_session_for_artifact(
            &original,
        )?);
    }
    Ok(None)
}

fn resolve_context_session_or_latest(
    session: Option<&str>,
    path: Option<&Path>,
) -> Result<Option<minutes_core::context_store::ContextSession>> {
    match resolve_context_session(session, path)? {
        Some(session) => Ok(Some(session)),
        None if session.is_some() || path.is_some() => {
            anyhow::bail!("the requested context session or linked path was not found")
        }
        None => Ok(minutes_core::context_store::latest_context_session()?),
    }
}

fn cmd_context_status(session: Option<&str>, path: Option<&Path>, json: bool) -> Result<()> {
    let session = resolve_context_session_or_latest(session, path)?;
    let status = if let Some(session) = &session {
        minutes_core::context_store::screen_context_status_for_session(&session.id)?
            .unwrap_or_default()
    } else {
        minutes_core::context_store::ScreenContextStatus::default()
    };
    let output = serde_json::json!({
        "session": session,
        "screen_context": status,
        "desktop_context": {
            "configured": minutes_core::config::Config::load().desktop_context.enabled,
            "note": "Desktop context contains app/window metadata, not screen pixels."
        }
    });
    if !json {
        eprintln!(
            "Screen context: {} ({} successful captures)",
            serde_json::to_string(&status.state)?.trim_matches('"'),
            status.successful_capture_count
        );
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn cmd_context_screen(
    session: Option<&str>,
    path: Option<&Path>,
    at: Option<&str>,
    limit: usize,
    json: bool,
) -> Result<()> {
    let anchor = at.map(parse_rfc3339_local).transpose()?;
    let explicitly_selected = session.is_some() || path.is_some();
    let resolved = if let Some(session) = resolve_context_session(session, path)? {
        Some(session)
    } else if explicitly_selected {
        anyhow::bail!("the requested context session or linked path was not found");
    } else if let Some(anchor) = anchor {
        minutes_core::context_store::get_session_covering_time(anchor)?
    } else {
        minutes_core::context_store::latest_context_session()?
    };
    let Some(session) = resolved else {
        anyhow::bail!("no context session is available");
    };
    let output = minutes_core::context_store::get_screen_context(&session.id, anchor, limit)?;
    if !json {
        eprintln!(
            "Screen context: {} — {} verified image(s)",
            serde_json::to_string(&output.status.state)?.trim_matches('"'),
            output.images.len()
        );
        if let Some(reason) = &output.reason {
            eprintln!("  {reason}");
        }
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn summarize_counts(values: impl Iterator<Item = Option<String>>) -> Vec<ContextCount> {
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for value in values.flatten() {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            continue;
        }
        *counts.entry(trimmed.to_string()).or_insert(0) += 1;
    }
    let mut pairs = counts
        .into_iter()
        .map(|(name, count)| ContextCount { name, count })
        .collect::<Vec<_>>();
    pairs.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    pairs.truncate(10);
    pairs
}

fn cmd_context_activity_summary(
    session: Option<&str>,
    path: Option<&Path>,
    start: Option<&str>,
    end: Option<&str>,
    json: bool,
) -> Result<()> {
    let resolved_session = resolve_context_session(session, path)?;

    let (events, links, window_start, window_end) = if let Some(session_row) = &resolved_session {
        let events =
            minutes_core::context_store::list_events_for_session(&session_row.id, None, None)?;
        let links = minutes_core::context_store::list_links_for_session(&session_row.id)?;
        let start = session_row.started_at;
        let end = session_row.ended_at.unwrap_or_else(Local::now);
        (events, links, start, end)
    } else {
        let start_dt = start.map(parse_rfc3339_local).transpose()?.ok_or_else(|| {
            anyhow::anyhow!("provide --session, --path, or both --start and --end")
        })?;
        let end_dt = end.map(parse_rfc3339_local).transpose()?.ok_or_else(|| {
            anyhow::anyhow!("provide --session, --path, or both --start and --end")
        })?;
        let events = minutes_core::context_store::list_events_in_window(start_dt, end_dt)?;
        (events, vec![], start_dt, end_dt)
    };

    let output = ContextSummaryOutput {
        session: resolved_session,
        links,
        top_apps: summarize_counts(events.iter().map(|e| e.app_name.clone())),
        top_windows: summarize_counts(events.iter().map(|e| e.window_title.clone())),
        events,
        window: ContextWindow {
            start: window_start.to_rfc3339(),
            end: window_end.to_rfc3339(),
        },
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    eprintln!(
        "Desktop context summary: {} → {}",
        output.window.start, output.window.end
    );
    if let Some(session_row) = &output.session {
        eprintln!(
            "  session: {} [{} / {}]",
            session_row.id,
            serde_json::to_string(&session_row.session_type)?,
            serde_json::to_string(&session_row.state)?
        );
    }
    if !output.top_apps.is_empty() {
        eprintln!(
            "  top apps: {}",
            output
                .top_apps
                .iter()
                .map(|entry| format!("{} ({})", entry.name, entry.count))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !output.top_windows.is_empty() {
        eprintln!(
            "  top windows: {}",
            output
                .top_windows
                .iter()
                .map(|entry| format!("{} ({})", entry.name, entry.count))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn cmd_context_search(query: &str, limit: usize, json: bool) -> Result<()> {
    let results = minutes_core::context_store::search_events(query, limit)?;
    let output = ContextSearchOutput { results };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    if output.results.is_empty() {
        eprintln!("No desktop-context events found for \"{}\".", query);
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    eprintln!("Desktop-context matches for \"{}\":", query);
    for event in &output.results {
        eprintln!(
            "  {} — {}{}{}",
            event.observed_at.to_rfc3339(),
            event
                .app_name
                .as_deref()
                .or(event.bundle_id.as_deref())
                .unwrap_or("unknown"),
            event
                .window_title
                .as_deref()
                .map(|title| format!(" :: {}", title))
                .unwrap_or_default(),
            event
                .url
                .as_deref()
                .map(|url| format!(" <{}>", url))
                .unwrap_or_default()
        );
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

fn cmd_context_get_moment(
    session: Option<&str>,
    path: Option<&Path>,
    at: Option<&str>,
    before_minutes: i64,
    after_minutes: i64,
    json: bool,
) -> Result<()> {
    let resolved_session = resolve_context_session(session, path)?;
    let anchor = if let Some(session_row) = &resolved_session {
        session_row.started_at
    } else if let Some(raw) = at {
        parse_rfc3339_local(raw)?
    } else {
        anyhow::bail!("provide --session, --path, or --at");
    };

    let window_start = anchor - chrono::Duration::minutes(before_minutes);
    let window_end = anchor + chrono::Duration::minutes(after_minutes);
    let events = if let Some(session_row) = &resolved_session {
        minutes_core::context_store::list_events_for_session(
            &session_row.id,
            Some(window_start),
            Some(window_end),
        )?
    } else {
        minutes_core::context_store::list_events_in_window(window_start, window_end)?
    };
    let links = if let Some(session_row) = &resolved_session {
        minutes_core::context_store::list_links_for_session(&session_row.id)?
    } else {
        vec![]
    };

    let output = ContextMomentOutput {
        session: resolved_session,
        links,
        events,
        window: ContextWindow {
            start: window_start.to_rfc3339(),
            end: window_end.to_rfc3339(),
        },
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&output)?);
        return Ok(());
    }

    eprintln!(
        "Desktop-context moment window: {} → {}",
        output.window.start, output.window.end
    );
    if let Some(session_row) = &output.session {
        eprintln!("  session: {}", session_row.id);
    }
    for event in &output.events {
        eprintln!(
            "  {} — {}{}",
            event.observed_at.to_rfc3339(),
            event.app_name.as_deref().unwrap_or("unknown"),
            event
                .window_title
                .as_deref()
                .map(|title| format!(" :: {}", title))
                .unwrap_or_default()
        );
    }
    println!("{}", serde_json::to_string_pretty(&output)?);
    Ok(())
}

// ── Import ──────────────────────────────────────────────────

fn cmd_import(from: &str, dir: Option<&Path>, dry_run: bool, config: &Config) -> Result<()> {
    if dir.is_none() && looks_like_audio_path(from) {
        let path = Path::new(from);
        if dry_run {
            eprintln!(
                "Would process audio file as a meeting: minutes process \"{}\" --type meeting",
                path.display()
            );
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "dry-run",
                    "file": path.display().to_string(),
                    "content_type": "meeting",
                    "command": format!("minutes process \"{}\" --type meeting", path.display()),
                }))?
            );
            return Ok(());
        }

        eprintln!(
            "Processing audio file via import compatibility path. Preferred command: minutes process \"{}\" --type meeting",
            path.display()
        );
        return cmd_process(path, "meeting", None, None, config);
    }

    match from {
        "granola" => import_granola(dir, dry_run, config),
        other => anyhow::bail!(
            "Unknown import source: {}. Supported source: granola. To process an audio file, run: minutes process \"{}\" --type meeting",
            other,
            other
        ),
    }
}

fn looks_like_audio_path(value: &str) -> bool {
    Path::new(value)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "wav" | "m4a" | "mp3" | "ogg" | "webm" | "mp4" | "mov" | "aac"
            )
        })
        .unwrap_or(false)
}

fn import_granola(dir: Option<&Path>, dry_run: bool, config: &Config) -> Result<()> {
    let source_dir = dir.map(PathBuf::from).unwrap_or_else(|| {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("/tmp"))
            .join(".granola-archivist")
            .join("output")
    });

    if !source_dir.exists() {
        anyhow::bail!(
            "Granola export directory not found: {}\n\
             Export your Granola meetings into it first (see \"Switching from Granola?\" in \
             the README for the granola-cli route), or pass an existing export dir with --dir",
            source_dir.display()
        );
    }

    let output_dir = &config.output_dir;
    std::fs::create_dir_all(output_dir)?;

    let mut imported = 0;
    let mut skipped = 0;

    for entry in std::fs::read_dir(&source_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let content = std::fs::read_to_string(&path)?;

        // Parse Granola format
        let title = content
            .lines()
            .find(|l| l.starts_with("# Meeting:"))
            .map(|l| l.trim_start_matches("# Meeting:").trim().to_string())
            .unwrap_or_else(|| "Untitled Granola Meeting".into());

        let date = content
            .lines()
            .find(|l| l.starts_with("Date:"))
            .and_then(|l| {
                let raw = l.trim_start_matches("Date:").trim();
                // Parse "2026-01-19 @ 20:27" format
                let cleaned = raw.replace(" @ ", "T").replace(" @", "T");
                if cleaned.len() >= 10 {
                    Some(cleaned)
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "2026-01-01T00:00:00".into());

        let attendees_line = content
            .lines()
            .find(|l| l.starts_with("Attendees:"))
            .map(|l| l.trim_start_matches("Attendees:").trim().to_string())
            .unwrap_or_default();
        let attendees = minutes_core::markdown::parse_attendees_raw(&attendees_line);

        // Extract notes and transcript sections
        let notes_section = extract_section(&content, "## Your Notes");
        let transcript_section = extract_section(&content, "## Transcript");

        // Build the output filename
        let date_prefix = &date[..10.min(date.len())];
        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == ' ' {
                    c
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join("-");
        let filename = format!("{}-{}.md", date_prefix, slug);
        let output_path = output_dir.join(&filename);

        if output_path.exists() {
            skipped += 1;
            if dry_run {
                eprintln!("  SKIP (exists): {}", filename);
            }
            continue;
        }

        // Build Minutes-format markdown
        let mut output = String::new();
        output.push_str("---\n");
        output.push_str(&format!("title: {}\n", title));
        output.push_str("type: meeting\n");
        output.push_str(&format!("date: {}\n", date));
        output.push_str("source: granola-import\n");
        if !attendees.is_empty() {
            output.push_str("attendees:\n");
            for attendee in &attendees {
                output.push_str(&format!("  - {}\n", serde_json::to_string(attendee)?));
            }
        }
        if !attendees_line.is_empty() && attendees_line != "None" {
            output.push_str(&format!(
                "attendees_raw: {}\n",
                serde_json::to_string(&attendees_line)?
            ));
        }
        output.push_str("---\n\n");

        if let Some(notes) = &notes_section {
            output.push_str("## Notes\n\n");
            output.push_str(notes);
            output.push_str("\n\n");
        }

        if let Some(transcript) = &transcript_section {
            output.push_str("## Transcript\n\n");
            output.push_str(transcript);
            output.push('\n');
        }

        if dry_run {
            eprintln!("  WOULD IMPORT: {} -> {}", path.display(), filename);
        } else {
            std::fs::write(&output_path, &output)?;
            // Set permissions to 0600
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&output_path, std::fs::Permissions::from_mode(0o600))?;
            }
            eprintln!("  Imported: {}", filename);
        }

        imported += 1;
    }

    // Update relationship graph index after batch import
    if !dry_run && imported > 0 {
        if let Err(e) = minutes_core::graph::rebuild_index(config) {
            tracing::warn!(error = %e, "graph index rebuild failed (non-fatal)");
        }
    }

    let action = if dry_run { "Would import" } else { "Imported" };
    let json = serde_json::json!({
        "imported": imported,
        "skipped": skipped,
        "source": "granola",
        "output_dir": output_dir.display().to_string(),
        "dry_run": dry_run,
    });
    println!("{}", serde_json::to_string_pretty(&json)?);
    eprintln!(
        "\n{} {} meetings ({} skipped, already exist)",
        action, imported, skipped
    );

    Ok(())
}

fn extract_section(content: &str, heading: &str) -> Option<String> {
    let mut in_section = false;
    let mut section = String::new();

    for line in content.lines() {
        if line.starts_with(heading) {
            in_section = true;
            continue;
        }
        if in_section && line.starts_with("## ") {
            break; // Next section
        }
        if in_section {
            section.push_str(line);
            section.push('\n');
        }
    }

    let trimmed = section.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

// ── Vault commands ───────────────────────────────────────────

fn cmd_vault_setup(
    path: Option<PathBuf>,
    strategy_override: Option<String>,
    subdir: Option<String>,
    mut config: Config,
) -> Result<()> {
    use minutes_core::vault;

    // Apply custom subdir before any strategy logic uses it
    if let Some(ref sub) = subdir {
        let trimmed = sub.trim_matches('/');
        if trimmed.is_empty() {
            anyhow::bail!("--subdir cannot be empty");
        }
        if Path::new(sub).is_absolute() || sub.contains("..") {
            anyhow::bail!("--subdir must be a relative path without '..' components");
        }
        config.vault.meetings_subdir = trimmed.to_string();
    }

    let vault_path = if let Some(p) = path {
        // Expand ~ to home directory
        let expanded = if p.starts_with("~") {
            dirs::home_dir()
                .unwrap_or_default()
                .join(p.strip_prefix("~").unwrap_or(&p))
        } else {
            p
        };
        if !expanded.exists() {
            anyhow::bail!("path does not exist: {}", expanded.display());
        }
        expanded
    } else {
        // Auto-detect vaults
        eprintln!("Scanning for markdown vaults...\n");
        let vaults = vault::detect_vaults();

        if vaults.is_empty() {
            eprintln!("No Obsidian/Logseq vaults detected.");
            eprintln!("Run with --path to specify your vault location:");
            eprintln!("  minutes vault setup --path ~/Documents/life");
            return Ok(());
        }

        eprintln!("Found {} vault(s):\n", vaults.len());
        for (i, v) in vaults.iter().enumerate() {
            let cloud_note = match &v.cloud {
                Some(provider) => format!(" ({})", provider),
                None => String::new(),
            };
            let tcc_note = if v.tcc_protected {
                " [TCC-protected]"
            } else {
                ""
            };
            eprintln!(
                "  {}. {} — {}{}{}",
                i + 1,
                v.path.display(),
                v.kind,
                cloud_note,
                tcc_note
            );
        }

        if vaults.len() == 1 {
            eprintln!("\nUsing the only vault found.");
            vaults[0].path.clone()
        } else {
            eprintln!("\nRe-run with --path to select a vault:");
            eprintln!("  minutes vault setup --path {}", vaults[0].path.display());
            return Ok(());
        }
    };

    // Analyze the vault path
    let tcc = vault::is_tcc_protected(&vault_path);
    let cloud = vault::is_cloud_synced(&vault_path);
    let recommended = strategy_override
        .as_ref()
        .map(|s| match s.as_str() {
            "symlink" => vault::VaultStrategy::Symlink,
            "copy" => vault::VaultStrategy::Copy,
            "direct" => vault::VaultStrategy::Direct,
            _ => vault::recommend_strategy(&vault_path),
        })
        .unwrap_or_else(|| vault::recommend_strategy(&vault_path));

    eprintln!("\nVault: {}", vault_path.display());
    if let Some(ref provider) = cloud {
        eprintln!("Cloud sync: {} detected", provider);
    }
    if tcc {
        eprintln!("TCC: ~/Documents/ is macOS-protected (terminal needs Full Disk Access)");
    }
    eprintln!("Strategy: {}", recommended);

    // Show explanation
    match recommended {
        vault::VaultStrategy::Symlink => {
            let meetings_link = vault_path.join(&config.vault.meetings_subdir);
            eprintln!(
                "\nCreating symlink: {} → {}",
                meetings_link.display(),
                config.output_dir.display()
            );

            match vault::create_symlink(&meetings_link, &config.output_dir) {
                Ok(()) => {
                    eprintln!("Symlink created successfully.");
                }
                Err(minutes_core::error::VaultError::PermissionDenied(path)) => {
                    eprintln!("\nPermission denied: {}", path);
                    eprintln!("\nmacOS blocks terminal access to this directory.");
                    eprintln!("Options:");
                    eprintln!("  1. Use Minutes.app (Settings > Vault) — no FDA needed");
                    eprintln!("  2. Create the symlink manually:");
                    eprintln!(
                        "     ln -s {} {}",
                        config.output_dir.display(),
                        meetings_link.display()
                    );
                    eprintln!("  3. Grant Full Disk Access to your terminal:");
                    eprintln!("     System Settings > Privacy & Security > Full Disk Access");
                    return Ok(());
                }
                Err(minutes_core::error::VaultError::ExistingDirectory(path)) => {
                    eprintln!("\nDirectory already exists: {}", path);
                    eprintln!("Move or merge it first, then re-run this command.");
                    eprintln!(
                        "  mv {} {}/vault-backup-meetings",
                        path,
                        vault_path.display()
                    );
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        }
        vault::VaultStrategy::Copy => {
            let dest = vault::vault_meetings_dir(&config);
            if cloud.is_some() {
                eprintln!("\nCloud-synced vault detected — using copy strategy.");
                eprintln!("Meetings will be copied to: {}", dest.display());
                eprintln!("This works with iCloud, Obsidian Sync, Dropbox, etc.");
            } else if tcc {
                eprintln!("\nTCC-protected path — using copy strategy.");
                eprintln!("Note: copy requires write access to the vault directory.");
                eprintln!("If this fails at runtime, use Minutes.app or grant FDA.");
            }
        }
        vault::VaultStrategy::Direct => {
            eprintln!("\nDirect mode: setting output_dir to vault meetings path.");
            eprintln!("All meetings will be written directly to the vault.");
            config.output_dir = vault_path.join(&config.vault.meetings_subdir);
        }
    }

    // Save config
    config.vault.enabled = true;
    config.vault.path = vault_path;
    config.vault.strategy = recommended.to_string();

    config
        .save()
        .map_err(|e| anyhow::anyhow!("failed to save config: {}", e))?;
    eprintln!(
        "\nVault configuration saved to: {}",
        Config::config_path().display()
    );
    eprintln!("Run `minutes vault status` to check health.");

    Ok(())
}

fn cmd_vault_status(config: &Config) -> Result<()> {
    use minutes_core::vault;

    let status = vault::check_health(config);
    match status {
        vault::VaultStatus::NotConfigured => {
            eprintln!("Vault: not configured");
            eprintln!("Run `minutes vault setup` to connect a vault.");
        }
        vault::VaultStatus::Healthy { strategy, path } => {
            eprintln!("Vault: healthy");
            eprintln!("  Strategy: {}", strategy);
            eprintln!("  Path: {}", path.display());
            eprintln!("  Subdir: {}", config.vault.meetings_subdir);
        }
        vault::VaultStatus::BrokenSymlink { link_path, target } => {
            eprintln!("Vault: BROKEN SYMLINK");
            eprintln!("  Link: {}", link_path.display());
            eprintln!("  Target: {} (does not exist)", target.display());
            eprintln!("Run `minutes vault setup` to fix.");
        }
        vault::VaultStatus::PermissionDenied { path } => {
            eprintln!("Vault: PERMISSION DENIED");
            eprintln!("  Path: {}", path.display());
            eprintln!("Grant Full Disk Access or use Minutes.app.");
        }
        vault::VaultStatus::MissingVaultDir { path } => {
            eprintln!("Vault: MISSING DIRECTORY");
            eprintln!("  Expected: {}", path.display());
            eprintln!("Run `minutes vault setup` to reconfigure.");
        }
    }
    Ok(())
}

fn cmd_vault_unlink(mut config: Config) -> Result<()> {
    if !config.vault.enabled {
        eprintln!("Vault is not configured.");
        return Ok(());
    }

    let old_path = config.vault.path.display().to_string();
    config.vault.enabled = false;
    config.vault.path = PathBuf::new();
    config.vault.strategy = "auto".into();

    config
        .save()
        .map_err(|e| anyhow::anyhow!("failed to save config: {}", e))?;
    eprintln!("Vault unlinked (was: {})", old_path);
    eprintln!("Note: any symlinks or copied files remain on disk.");
    Ok(())
}

fn cmd_vault_sync(config: &Config) -> Result<()> {
    use minutes_core::vault;

    if !config.vault.enabled {
        eprintln!("Vault is not configured. Run `minutes vault setup` first.");
        return Ok(());
    }

    eprintln!("Syncing meetings to vault...");
    match vault::sync_all(config) {
        Ok(synced) => {
            if synced.is_empty() {
                eprintln!("No files to sync (strategy may not require copying).");
            } else {
                eprintln!("Synced {} file(s) to vault.", synced.len());
                for path in &synced {
                    eprintln!("  {}", path.display());
                }
            }
        }
        Err(e) => {
            eprintln!("Sync failed: {}", e);
        }
    }
    Ok(())
}

// ──────────────────────────────────────────────────────────────
// minutes health — system readiness diagnostics
// ──────────────────────────────────────────────────────────────

fn cmd_health(json: bool) -> Result<()> {
    let config = Config::load();
    let items = minutes_core::health::check_all(&config);

    if json {
        let attention_count = items
            .iter()
            .filter(|item| item.state == "attention")
            .count();
        let report = serde_json::json!({
            "engine": config.transcription.engine,
            "all_ready": attention_count == 0,
            "attention_count": attention_count,
            "items": items,
        });
        let envelope = json_envelope("minutes health", report);
        println!("{}", serde_json::to_string_pretty(&envelope)?);
    } else {
        let all_ready = items.iter().all(|i| i.state == "ready");
        for item in &items {
            let icon = match item.state.as_str() {
                "ready" => "\u{2713}", // ✓
                "attention" => "!",
                _ => "?",
            };
            let opt = if item.optional { " (optional)" } else { "" };
            eprintln!("  {} {}{}", icon, item.label, opt);
            eprintln!("    {}", item.detail);
        }
        if all_ready {
            eprintln!("\nAll systems ready.");
        } else {
            let attention_count = items.iter().filter(|i| i.state == "attention").count();
            eprintln!(
                "\n{} item{} need attention.",
                attention_count,
                if attention_count == 1 { "" } else { "s" }
            );
        }
    }

    Ok(())
}

// ──────────────────────────────────────────────────────────────
// minutes demo --full — Snow Crash themed sample meetings
// ──────────────────────────────────────────────────────────────

fn cmd_demo_full(config: &Config) -> Result<()> {
    let paths = demo_data::seed_demo_meetings(&config.output_dir)?;

    if paths.is_empty() {
        eprintln!("All demo meetings already exist. Run `minutes demo --clean --full` to re-seed.");
        return Ok(());
    }

    // Rebuild the relationship graph silently (suppress tracing for clean animation)
    {
        let quiet = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::ERROR)
            .with_target(false)
            .finish();
        tracing::subscriber::with_default(quiet, || {
            minutes_core::graph::rebuild_index(config).ok();
        });
    }

    // Demo has a fixed cast of 6 characters
    demo_data::present_demo(paths.len(), 6, &config.output_dir);

    Ok(())
}

// minutes demo — deterministic pipeline demo with bundled audio
// ──────────────────────────────────────────────────────────────

/// Bundled short speech WAV used by `minutes demo`.
/// If this file doesn't exist at build time, compilation fails — intentionally.
const DEMO_WAV: &[u8] = include_bytes!("../assets/demo.wav");

fn cmd_demo(config: &Config) -> Result<()> {
    // Ensure output directory exists
    config.ensure_dirs()?;

    // Write bundled WAV to temp file
    let demo_dir = config.output_dir.join(".demo-temp");
    std::fs::create_dir_all(&demo_dir)?;
    let demo_path = demo_dir.join("demo.wav");
    std::fs::write(&demo_path, DEMO_WAV)?;

    eprintln!("Running demo pipeline...");
    let result = minutes_core::pipeline::process_with_progress(
        &demo_path,
        ContentType::Memo,
        Some("Minutes Demo"),
        config,
        |stage| {
            let label = match stage {
                minutes_core::pipeline::PipelineStage::Transcribing => "Transcribing demo audio",
                minutes_core::pipeline::PipelineStage::Diarizing => "Analyzing speakers",
                minutes_core::pipeline::PipelineStage::Summarizing => "Generating summary",
                minutes_core::pipeline::PipelineStage::Saving => "Saving demo",
            };
            eprintln!("  {}", label);
        },
    );

    // Clean up temp file
    std::fs::remove_file(&demo_path).ok();
    std::fs::remove_dir_all(&demo_dir).ok();

    match result {
        Ok(result) => {
            eprintln!("\nDemo complete! Saved: {}", result.path.display());
            let result_json = serde_json::json!({
                "status": "done",
                "file": result.path.display().to_string(),
                "title": result.title,
                "words": result.word_count,
            });
            println!("{}", serde_json::to_string_pretty(&result_json)?);
            Ok(())
        }
        Err(e) => {
            eprintln!("\nDemo failed: {}", e);
            eprintln!("Run `minutes health` to check the speech model and audio pipeline.");
            eprintln!(
                "If the speech model is missing, run `minutes setup`; otherwise please report the demo failure."
            );
            Err(e.into())
        }
    }
}

#[cfg(feature = "whisper")]
fn cmd_dictate(stdout: bool, note_only: bool, config: &Config) -> Result<()> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let permission_preflight = minutes_core::capture::preflight_microphone_only();
    if let Some(reason) = &permission_preflight.blocking_reason {
        anyhow::bail!("{}", reason);
    }
    for warning in &permission_preflight.warnings {
        eprintln!("[minutes] {}", warning);
    }

    eprintln!("[minutes] Starting dictation (Ctrl-C to stop)...");
    if config.dictation.accumulate {
        eprintln!(
            "[minutes] Speak naturally. Text accumulates across pauses and is written when dictation ends."
        );
    } else {
        eprintln!("[minutes] Speak naturally. Text goes to clipboard after each pause.");
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);

    // Handle Ctrl-C (double press to force quit)
    ctrlc::set_handler(move || {
        if let InterruptAction::ForceExit(code) = handle_graceful_interrupt(
            &stop_clone,
            "Stopping dictation... (Ctrl+C again to force quit)",
        ) {
            std::process::exit(code);
        }
    })?;

    // Ignore SIGTERM — `minutes stop` uses sentinel file for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    let mut config = config.clone();
    if stdout {
        config.dictation.destination = "stdout".into();
        config.dictation.daily_note_log = !note_only;
    } else if note_only {
        config.dictation.destination = "daily_note".into();
    } else if config.dictation.destination == "insert" {
        config.dictation.destination = "clipboard".into();
    }

    minutes_core::dictation::run(
        stop_flag,
        &config,
        |event| {
            use minutes_core::dictation::DictationEvent;
            match event {
                DictationEvent::Listening => eprintln!("[minutes] Listening..."),
                DictationEvent::Accumulating => eprintln!("[minutes] Speaking detected..."),
                DictationEvent::Processing => eprintln!("[minutes] Transcribing..."),
                DictationEvent::PartialText(text) => {
                    // Clear line and show partial text (streaming preview)
                    eprint!("\r\x1b[K[minutes] {}", text);
                }
                DictationEvent::AudioLevel(_) => {}
                DictationEvent::SilenceCountdown { .. } => {} // CLI doesn't show countdown
                DictationEvent::Success => {
                    eprintln!(); // newline after partial text
                    if config.dictation.accumulate {
                        eprintln!("[minutes] Captured text");
                    } else {
                        eprintln!("[minutes] Done — text copied to clipboard");
                    }
                }
                DictationEvent::Error => eprintln!("[minutes] Transcription failed — audio saved"),
                DictationEvent::Cancelled => eprintln!("[minutes] Dictation cancelled"),
                DictationEvent::Yielded => {
                    eprintln!("[minutes] Recording started — yielding dictation")
                }
            }
        },
        |result| {
            if stdout {
                println!("{}", result.text);
            }
            if let Some(ref path) = result.file_path {
                eprintln!("[minutes] Saved: {}", path.display());
            }
            let (outcome, method, message) = match result.destination.as_str() {
                "stdout" => ("printed", "stdout", "Printed dictation to stdout."),
                "daily_note" => ("saved", "daily_note", "Saved dictation to the daily note."),
                _ => (
                    "copied",
                    "clipboard_only",
                    "Copied dictation to the clipboard.",
                ),
            };
            let record = minutes_core::dictation_memory::DictationMemoryRecord::new(
                minutes_core::dictation_memory::DictationMemoryInput {
                    raw_text: result.raw_text.clone(),
                    cleaned_text: result.text.clone(),
                    duration_secs: result.duration_secs,
                    engine_id: match config.dictation.backend.as_str() {
                        "whisper" | "" => format!("whisper:{}", config.dictation.model),
                        backend => backend.to_string(),
                    },
                    engine_descriptor_version: Some(config.dictation.model.clone()),
                    vocabulary_mode: None,
                    vocabulary_used: Vec::new(),
                    destination: result.destination.clone(),
                    insertion: minutes_core::dictation_memory::DictationInsertionMemory {
                        outcome: outcome.into(),
                        method: method.into(),
                        verified: true,
                        clipboard_restored: false,
                        message: message.into(),
                    },
                    target_context: None,
                    file_path: result.file_path.clone(),
                    daily_note_appended: result.daily_note_appended,
                },
            );
            if let Err(error) = minutes_core::dictation_memory::append_record(record) {
                eprintln!("[minutes] Could not update dictation history: {error}");
            }
        },
    )?;

    Ok(())
}

#[cfg(not(feature = "whisper"))]
fn cmd_dictate(_stdout: bool, _note_only: bool, _config: &Config) -> Result<()> {
    Err(anyhow::anyhow!(
        "`minutes dictate` requires the `whisper` feature. Reinstall without `--no-default-features` to use local dictation."
    ))
}

fn cmd_enroll(file: Option<&Path>, duration: u64, config: &Config) -> Result<()> {
    use minutes_core::voice;

    // Step 1: Check name — offer to set it if missing
    let my_name = match config.identity.name.as_ref() {
        Some(name) if !name.is_empty() => name.clone(),
        _ => {
            eprintln!(
                "Your name isn't set yet. This is needed so Minutes knows which speaker is you."
            );
            eprint!("What's your name? ");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            let name = input.trim().to_string();
            if name.is_empty() {
                return Err(anyhow::anyhow!("Name is required for voice enrollment."));
            }
            // Save to config file
            let config_path = dirs::config_dir()
                .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
                .join("minutes/config.toml");
            if config_path.exists() {
                let mut content = std::fs::read_to_string(&config_path)?;
                if content.contains("[identity]") {
                    // Add name under existing [identity] section
                    content =
                        content.replace("[identity]", &format!("[identity]\nname = \"{}\"", name));
                } else {
                    content.push_str(&format!("\n[identity]\nname = \"{}\"\n", name));
                }
                std::fs::write(&config_path, content)?;
                eprintln!("Saved to {}", config_path.display());
            }
            name
        }
    };

    // Step 2: Check diarization models
    if !minutes_core::diarize::models_installed(config) {
        eprintln!("Speaker diarization models aren't installed yet.");
        eprintln!("Run this first:  minutes setup --diarization");
        eprintln!("Then try:        minutes enroll");
        return Err(anyhow::anyhow!(
            "Diarization models required for voice enrollment."
        ));
    }

    // Step 3: Record or load audio
    eprintln!();
    eprintln!(
        "  \x1b[1;36m◉ Voice Enrollment\x1b[0m  \x1b[2mfor\x1b[0m \x1b[1m{}\x1b[0m",
        my_name
    );
    eprintln!();

    let audio_path = if let Some(path) = file {
        if !path.exists() {
            return Err(anyhow::anyhow!("File not found: {}", path.display()));
        }
        eprintln!("  Using audio file: {}", path.display());
        path.to_path_buf()
    } else {
        eprintln!("  This creates a voice profile so Minutes can identify you");
        eprintln!(
            "  in future meetings. Just talk normally for {} seconds.",
            duration
        );
        eprintln!();
        eprintln!("  Tips:");
        eprintln!("  - Use the same mic you use for meetings");
        eprintln!("  - Talk at your normal volume and pace");
        eprintln!("  - Say anything — read something aloud, describe your day");
        eprintln!();
        eprint!("  Ready? Press Enter to start recording...");
        let mut buf = String::new();
        std::io::stdin().read_line(&mut buf)?;

        eprintln!();
        eprintln!(
            "  \x1b[1;32m● REC\x1b[0m  \x1b[1mSpeak now!\x1b[0m  ({}s)",
            duration
        );
        eprintln!();

        let tmp_dir = std::env::temp_dir().join("minutes-enroll");
        std::fs::create_dir_all(&tmp_dir)?;
        let tmp_path = tmp_dir.join("enroll-sample.wav");
        let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag_clone = stop_flag.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(duration * 1000));
            flag_clone.store(true, std::sync::atomic::Ordering::Relaxed);
        });
        minutes_core::capture::record_to_wav(&tmp_path, stop_flag, config)?;
        eprintln!("  \x1b[1;32m✓\x1b[0m Recording captured.");
        tmp_path
    };

    // Step 4: Extract voice embedding
    eprintln!("  \x1b[2mAnalyzing your voice...\x1b[0m");
    let result = minutes_core::diarize::diarize(&audio_path, config)
        .ok_or_else(|| anyhow::anyhow!(
            "Could not analyze the recording. Make sure you spoke clearly and your mic is working.\n\
             Check with: minutes devices"
        ))?;

    if result.segments.is_empty() {
        return Err(anyhow::anyhow!(
            "No speech detected in the recording.\n\n\
             Try again:\n\
             - Make sure your mic is not muted\n\
             - Speak at normal volume\n\
             - Reduce background noise\n\
             - Check your mic: minutes devices"
        ));
    }

    if result.num_speakers > 1 {
        tracing::warn!(
            speakers = result.num_speakers,
            "multiple speakers detected during enrollment — picking an arbitrary one"
        );
        eprintln!(
            "  ⚠ Detected {} voices — the enrolled profile may not be yours.",
            result.num_speakers
        );
        eprintln!("  For best results, re-run in a quiet room with just you speaking.");
    }

    eprintln!("  \x1b[2mComputing voice profile...\x1b[0m");

    // Enrollment expects a single speaker, so just grab the first available
    // embedding. When multiple speakers are detected the choice is arbitrary
    // (HashMap order), but the user is warned above to re-record solo.
    let (_, embedding) = result
        .speaker_embeddings
        .iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Diarization produced no speaker embeddings."))?;
    let embedding = embedding.clone();

    // Step 5: Save
    let conn = voice::open_db().map_err(|e| anyhow::anyhow!("{}", e))?;
    let slug: String = my_name
        .to_lowercase()
        .chars()
        .map(|c: char| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    voice::save_profile_blended(
        &conn,
        &slug,
        &my_name,
        &embedding,
        "self-enrollment",
        voice::model_version(config),
    )
    .map_err(|e| anyhow::anyhow!("{}", e))?;

    let profiles = voice::list_profiles(&conn).map_err(|e| anyhow::anyhow!("{}", e))?;
    if let Some(p) = profiles.iter().find(|p| p.person_slug == slug) {
        eprintln!();
        eprintln!("  \x1b[1;32m✓ Voice profile saved!\x1b[0m");
        eprintln!("  \x1b[2m───────────────────────\x1b[0m");
        eprintln!("  \x1b[2mName:\x1b[0m     \x1b[1m{}\x1b[0m", p.name);
        eprintln!("  \x1b[2mSamples:\x1b[0m  {}", p.sample_count);
        eprintln!("  \x1b[2mModel:\x1b[0m    {}", p.model_version);
        eprintln!();
        eprintln!("  \x1b[36mWhat happens next:\x1b[0m");
        eprintln!("  \x1b[2m›\x1b[0m Your voice will be auto-identified in future meetings");
        eprintln!(
            "  \x1b[2m›\x1b[0m Your lines show as \x1b[1m[{}]\x1b[0m instead of [SPEAKER_X]",
            p.name
        );
        eprintln!("  \x1b[2m›\x1b[0m Run \x1b[33mminutes enroll\x1b[0m again to improve accuracy");
        eprintln!("  \x1b[2m›\x1b[0m Run \x1b[33mminutes voices\x1b[0m to see your profile");
    }

    if file.is_none() {
        std::fs::remove_file(&audio_path).ok();
    }
    Ok(())
}

fn cmd_voices(delete: bool, json: bool) -> Result<()> {
    use minutes_core::voice;
    let conn = voice::open_db().map_err(|e| anyhow::anyhow!("{}", e))?;
    if delete {
        let profiles = voice::list_profiles(&conn).map_err(|e| anyhow::anyhow!("{}", e))?;
        if profiles.is_empty() {
            eprintln!("No voice profiles enrolled.");
            return Ok(());
        }
        for p in &profiles {
            voice::delete_profile(&conn, &p.person_slug).map_err(|e| anyhow::anyhow!("{}", e))?;
            eprintln!("Deleted voice profile: {}", p.name);
        }
        return Ok(());
    }
    let profiles = voice::list_profiles(&conn).map_err(|e| anyhow::anyhow!("{}", e))?;
    if json {
        println!("{}", serde_json::to_string_pretty(&profiles)?);
        return Ok(());
    }
    if profiles.is_empty() {
        eprintln!("No voice profiles enrolled.\nRun: minutes enroll");
        return Ok(());
    }
    eprintln!("Voice profiles:");
    for p in &profiles {
        eprintln!(
            "  {} — {} samples, {} ({})",
            p.name, p.sample_count, p.source, p.model_version
        );
        eprintln!(
            "    enrolled: {}, updated: {}",
            p.enrolled_at.get(..10).unwrap_or(&p.enrolled_at),
            p.updated_at.get(..10).unwrap_or(&p.updated_at)
        );
    }
    Ok(())
}

fn cmd_confirm(
    meeting_path: &Path,
    speaker: Option<&str>,
    name: Option<&str>,
    save_voice: bool,
    config: &Config,
) -> Result<()> {
    use minutes_core::diarize::{AttributionSource, Confidence};
    use minutes_core::overlays;
    use minutes_core::voice;

    if !meeting_path.exists() {
        return Err(anyhow::anyhow!(
            "Meeting not found: {}",
            meeting_path.display()
        ));
    }

    // Read the meeting file
    let content = std::fs::read_to_string(meeting_path)?;
    let (yaml_str, _body) = minutes_core::markdown::split_frontmatter(&content);

    if yaml_str.is_empty() {
        return Err(anyhow::anyhow!("Meeting has no YAML frontmatter"));
    }

    // Parse existing frontmatter
    let mut frontmatter: minutes_core::markdown::Frontmatter = serde_yaml::from_str(yaml_str)
        .map_err(|e| anyhow::anyhow!("Failed to parse frontmatter: {}", e))?;

    if frontmatter.speaker_map.is_empty() {
        eprintln!("No speaker attributions found in this meeting.");
        eprintln!("Process the meeting with diarization enabled first.");
        return Ok(());
    }

    // Load meeting embeddings (for optional voice save)
    let meeting_embeddings = voice::load_meeting_embeddings(meeting_path);
    let mut overlay_writes: Vec<(String, String, String)> = Vec::new();

    // Non-interactive mode: confirm a specific speaker
    if let (Some(speaker_label), Some(new_name)) = (speaker, name) {
        let found = frontmatter
            .speaker_map
            .iter_mut()
            .find(|a| a.speaker_label == speaker_label);

        if let Some(attr) = found {
            let old_confidence = attr.confidence;
            let old_name = attr.name.clone();
            attr.name = new_name.to_string();
            attr.confidence = Confidence::High;
            attr.source = AttributionSource::Manual;
            overlay_writes.push((speaker_label.to_string(), new_name.to_string(), old_name));
            eprintln!(
                "Confirmed: {} = {} (was {:?} → High)",
                speaker_label, new_name, old_confidence
            );

            // Optionally save voice profile
            if save_voice {
                if let Some(ref embeddings) = meeting_embeddings {
                    if let Some(embedding) = embeddings.get(speaker_label) {
                        let conn = voice::open_db().map_err(|e| anyhow::anyhow!("{}", e))?;
                        let slug: String = new_name
                            .to_lowercase()
                            .chars()
                            .map(|c: char| if c.is_alphanumeric() { c } else { '-' })
                            .collect::<String>()
                            .trim_matches('-')
                            .to_string();
                        voice::save_profile_blended(
                            &conn,
                            &slug,
                            new_name,
                            embedding,
                            "confirmed",
                            voice::model_version(config),
                        )
                        .map_err(|e| anyhow::anyhow!("{}", e))?;
                        eprintln!(
                            "Voice profile saved for {} (from confirmed meeting)",
                            new_name
                        );
                    } else {
                        eprintln!(
                            "Warning: no embedding found for {} in meeting sidecar",
                            speaker_label
                        );
                    }
                } else {
                    eprintln!("Warning: no meeting embeddings sidecar found (meeting was processed before Level 3)");
                }
            }
        } else {
            return Err(anyhow::anyhow!(
                "Speaker '{}' not found in speaker_map. Available: {}",
                speaker_label,
                frontmatter
                    .speaker_map
                    .iter()
                    .map(|a| a.speaker_label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    } else {
        // Interactive mode: walk through all attributions
        eprintln!("Speaker attributions for: {}", frontmatter.title);
        eprintln!();

        for attr in &mut frontmatter.speaker_map {
            if attr.confidence == Confidence::High {
                eprintln!(
                    "  {} = {} (high, {:?}) ✓",
                    attr.speaker_label, attr.name, attr.source
                );
                continue;
            }

            eprint!(
                "  {} = {} ({:?}, {:?}) — confirm? [Y/n/name]: ",
                attr.speaker_label, attr.name, attr.confidence, attr.source
            );

            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            let input = input.trim();

            if input.is_empty()
                || input.eq_ignore_ascii_case("y")
                || input.eq_ignore_ascii_case("yes")
            {
                let old_name = attr.name.clone();
                attr.confidence = Confidence::High;
                attr.source = AttributionSource::Manual;
                overlay_writes.push((attr.speaker_label.clone(), attr.name.clone(), old_name));
                eprintln!("    → Confirmed: {} = {}", attr.speaker_label, attr.name);
            } else if input.eq_ignore_ascii_case("n") || input.eq_ignore_ascii_case("no") {
                eprintln!("    → Skipped");
            } else {
                // User typed a different name
                let old_name = attr.name.clone();
                attr.name = input.to_string();
                attr.confidence = Confidence::High;
                attr.source = AttributionSource::Manual;
                overlay_writes.push((attr.speaker_label.clone(), attr.name.clone(), old_name));
                eprintln!("    → Updated: {} = {}", attr.speaker_label, attr.name);
            }
        }

        // Ask about saving voice profiles for confirmed speakers
        if save_voice {
            if let Some(ref embeddings) = meeting_embeddings {
                let conn = voice::open_db().map_err(|e| anyhow::anyhow!("{}", e))?;
                for attr in &frontmatter.speaker_map {
                    if attr.confidence == Confidence::High
                        && attr.source == AttributionSource::Manual
                    {
                        if let Some(embedding) = embeddings.get(&attr.speaker_label) {
                            let slug: String = attr
                                .name
                                .to_lowercase()
                                .chars()
                                .map(|c: char| if c.is_alphanumeric() { c } else { '-' })
                                .collect::<String>()
                                .trim_matches('-')
                                .to_string();
                            voice::save_profile_blended(
                                &conn,
                                &slug,
                                &attr.name,
                                embedding,
                                "confirmed",
                                voice::model_version(config),
                            )
                            .map_err(|e| anyhow::anyhow!("{}", e))?;
                            eprintln!("  Voice profile saved for {}", attr.name);
                        }
                    }
                }
            } else {
                eprintln!("No meeting embeddings sidecar — voice profiles not saved");
            }
        }
    }

    for (speaker_label, confirmed_name, previous_name) in &overlay_writes {
        overlays::write_speaker_confirmation(
            meeting_path,
            speaker_label,
            confirmed_name,
            Some(previous_name),
            Some("minutes confirm"),
        )
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    }

    if !overlay_writes.is_empty() {
        if let Err(error) = minutes_core::graph::rebuild_index(config) {
            eprintln!(
                "Warning: speaker overlay saved, but graph rebuild failed: {}",
                error
            );
        }
    }

    let confirmed_count = frontmatter
        .speaker_map
        .iter()
        .filter(|a| a.confidence == Confidence::High)
        .count();
    eprintln!(
        "\nSpeaker overlay updated: {}/{} speakers confirmed. Meeting markdown was not rewritten.",
        confirmed_count,
        frontmatter.speaker_map.len()
    );

    Ok(())
}

#[cfg(feature = "whisper")]
fn cmd_live(config: &Config) -> Result<()> {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let permission_preflight = minutes_core::capture::preflight_microphone_only();
    if let Some(reason) = &permission_preflight.blocking_reason {
        anyhow::bail!("{}", reason);
    }
    for warning in &permission_preflight.warnings {
        eprintln!("[minutes] {}", warning);
    }

    eprintln!("Starting live transcript session...");
    if config.transcription.engine == "apple-speech" {
        eprintln!(
            "[minutes] Apple Speech experimental live path selected. If unavailable or weak, Minutes will fall back to Parakeet or Whisper for this session."
        );
    }
    eprintln!("Press Ctrl-C or run `minutes stop` to end.\n");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    // Handle Ctrl-C (double press to force quit)
    ctrlc::set_handler(move || {
        if let InterruptAction::ForceExit(code) = handle_graceful_interrupt(
            &stop_clone,
            "Stopping gracefully... (Ctrl+C again to force quit)",
        ) {
            std::process::exit(code);
        }
    })
    .ok();

    // Ignore SIGTERM — `minutes stop` uses sentinel file for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    // No sentinel watcher needed — run_inner already polls check_and_clear_sentinel
    // directly in its main loop, avoiding the thread-join and double-consume race.
    let live_context_session_id =
        minutes_core::desktop_context::maybe_start_live_transcript_session(
            &config.desktop_context,
            Local::now(),
        );
    let _desktop_context_collector = live_context_session_id.as_ref().and_then(|session_id| {
        match minutes_core::desktop_context::DesktopContextCollector::start(
            session_id.clone(),
            minutes_core::desktop_context::DesktopContextSessionKind::LiveTranscript,
            config.desktop_context.clone(),
        ) {
            Ok(collector) => Some(collector),
            Err(error) => {
                tracing::warn!(error = %error, "desktop context collector unavailable for CLI live transcript");
                None
            }
        }
    });

    let relay_epoch = chrono::Utc::now().timestamp_millis().unsigned_abs().max(1);
    let (partial_publisher, partial_subscriber) = minutes_core::live_partials::channel(
        relay_epoch,
        minutes_core::live_partials::DEFAULT_PARTIAL_CHANNEL_CAPACITY,
    );
    let _capture_relay = match minutes_core::copilot::CaptureRelayServer::start(
        minutes_core::copilot::CopilotEvidenceMode::CaptureRelayPartials,
        Some(partial_subscriber),
    ) {
        Ok(relay) => Some(relay),
        Err(minutes_core::copilot::CaptureRelayError::AlreadyOwned(owner_pid)) => {
            anyhow::bail!(
                "Another Minutes process (PID {owner_pid}) already owns capture. Live Transcript did not open a second microphone; use `minutes transcript` to follow the existing session."
            );
        }
        Err(minutes_core::copilot::CaptureRelayError::OwnershipBusy) => {
            anyhow::bail!(
                "Another Minutes process is starting or stopping capture. Live Transcript did not open a second microphone; wait a moment and try again."
            );
        }
        Err(error) => {
            eprintln!(
                "[minutes] Live coaching cannot attach to this transcript: {error}. Live transcription continues, but Minutes will not open a second microphone for coaching."
            );
            None
        }
    };

    match minutes_core::live_transcript::run_with_partials(
        stop,
        config,
        live_context_session_id,
        Some(partial_publisher),
    ) {
        Ok((lines, duration, path)) => {
            eprintln!("\nLive transcript complete:");
            eprintln!("  {} utterances in {:.0}s", lines, duration);
            eprintln!("  Saved to: {}", path.display());
            Ok(())
        }
        Err(e) => {
            eprintln!("Live transcript error: {}", e);
            Err(e.into())
        }
    }
}

#[cfg(not(feature = "whisper"))]
fn cmd_live(_config: &Config) -> Result<()> {
    Err(anyhow::anyhow!(
        "`minutes live` requires the `whisper` feature. Reinstall without `--no-default-features` to use live transcription."
    ))
}

#[cfg(feature = "whisper")]
fn cmd_transcript(since: Option<&str>, status: bool, format: &str) -> Result<()> {
    if status {
        let s = minutes_core::live_transcript::session_status();
        if format == "json" {
            println!("{}", serde_json::to_string_pretty(&s)?);
        } else {
            if s.active {
                let source_label = match s.source {
                    Some(minutes_core::live_transcript::TranscriptSource::RecordingSidecar) => {
                        " (from recording)"
                    }
                    _ => "",
                };
                eprintln!(
                    "Live transcript: ACTIVE{} (PID: {})",
                    source_label,
                    s.pid.unwrap_or(0)
                );
            } else {
                eprintln!("Live transcript: inactive");
            }
            eprintln!("  Lines: {}", s.line_count);
            eprintln!("  Duration: {:.0}s", s.duration_secs);
            if let Some(ref diagnostic) = s.diagnostic {
                eprintln!("  Diagnostic: {}", diagnostic);
            }
            if let Some(ref p) = s.jsonl_path {
                eprintln!("  File: {}", p);
            }
        }
        return Ok(());
    }

    let lines = match since {
        Some(s) if s.ends_with('m') || s.ends_with('s') => {
            // Duration-based: "5m" or "30s"
            let (num_str, unit) = s.split_at(s.len() - 1);
            let num: u64 = num_str
                .parse()
                .map_err(|_| anyhow::anyhow!("invalid duration: {}", s))?;
            let ms = match unit {
                "m" => num
                    .checked_mul(60_000)
                    .ok_or_else(|| anyhow::anyhow!("duration too large: {}", s))?,
                "s" => num
                    .checked_mul(1000)
                    .ok_or_else(|| anyhow::anyhow!("duration too large: {}", s))?,
                _ => anyhow::bail!("invalid duration unit: {}", unit),
            };
            minutes_core::live_transcript::read_since_duration(ms)?
        }
        Some(s) => {
            // Line number
            let n: usize = s.parse().map_err(|_| {
                anyhow::anyhow!(
                    "invalid --since value: '{}'. Use a line number (42) or duration (5m, 30s)",
                    s
                )
            })?;
            minutes_core::live_transcript::read_since_line(n)?
        }
        None => {
            // All lines
            minutes_core::live_transcript::read_since_line(0)?
        }
    };

    if format == "json" {
        for line in &lines {
            println!("{}", serde_json::to_string(line)?);
        }
    } else {
        for line in &lines {
            let ts = line.ts.format("%H:%M:%S");
            let speaker = line.speaker.as_deref().unwrap_or("?");
            println!("[{}] [{}] {}", ts, speaker, line.text);
        }
    }

    Ok(())
}

#[cfg(not(feature = "whisper"))]
fn cmd_transcript(_since: Option<&str>, _status: bool, _format: &str) -> Result<()> {
    Err(anyhow::anyhow!(
        "`minutes transcript` requires the `whisper` feature. Reinstall without `--no-default-features` to read live transcripts."
    ))
}
