use anyhow::Result;
use chrono::{Local, TimeZone};
use clap::{Parser, Subcommand};
use minutes_core::capture::RecordingIntent;
use minutes_core::{CaptureMode, Config, ContentType};
use serde::Serialize;

mod dashboard;
use std::path::{Path, PathBuf};

/// minutes — conversation memory for AI assistants.
/// Every meeting, every idea, every voice note — searchable by your AI.
#[derive(Parser)]
#[command(name = "minutes", version, about, long_about = None)]
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

    /// Keep a recording alive (reset auto-stop timers)
    Extend,

    /// Hidden worker that processes queued jobs.
    #[command(hide = true)]
    ProcessQueue,

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
    },

    /// Show open action items across all meetings
    Actions {
        /// Filter by assignee name
        #[arg(short, long)]
        assignee: Option<String>,
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
    },

    /// List recent meetings and voice memos
    List {
        /// Maximum number of results
        #[arg(short, long, default_value = "10")]
        limit: usize,

        /// Filter by type: meeting or memo
        #[arg(short = 't', long)]
        content_type: Option<String>,
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

    /// Clean up hallucinated repetitions in existing transcripts
    Clean {
        /// Path to meeting .md file, or "all" to clean all meetings
        meeting: String,

        /// Actually modify the files (default: dry-run showing what would change)
        #[arg(long)]
        apply: bool,
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
        #[arg(long, default_value = "tdt-ctc-110m")]
        parakeet_model: String,
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
    },

    /// List available audio input devices
    Devices,

    /// List audio input devices grouped by category (Microphones / System Audio / Virtual)
    Sources,

    /// Install or uninstall the folder watcher as a login service
    Service {
        /// Action: install or uninstall
        #[arg(value_parser = ["install", "uninstall", "status"])]
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
    Demo,

    /// Output the JSON Schema for the meeting frontmatter format
    Schema,

    /// Get a meeting by filename slug
    Get {
        /// Filename slug to match (e.g., "2026-03-17-advisor-call")
        slug: String,
    },

    /// Show recent events from the event log
    Events {
        /// Maximum number of events
        #[arg(short, long, default_value = "50")]
        limit: usize,

        /// Only events since this date (ISO format)
        #[arg(long)]
        since: Option<String>,
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

    /// Import meetings from another app (e.g., Granola)
    Import {
        /// Source app: granola
        #[arg(value_parser = ["granola"])]
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

fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize logging
    let log_level = if cli.verbose { "debug" } else { "info" };
    tracing_subscriber::fmt()
        .with_env_filter(log_level)
        .with_target(false)
        .init();

    let mut config = Config::load();

    // Rotate old log files at startup
    minutes_core::logging::rotate_logs().ok();

    match cli.command {
        Commands::Record {
            title,
            context,
            mode,
            intent,
            allow_degraded,
            language,
            device,
            source,
            call,
            diagnose,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }

            // CLI flags > [recording.sources] > device
            if !source.is_empty() {
                if source.len() >= 2 {
                    // Multi-source CLI recording: use voice device, warn that
                    // call device capture is handled by the desktop app
                    eprintln!(
                        "[minutes] Multi-source CLI recording is not yet implemented.\n\
                         Using '{}' as the recording device.\n\
                         For dual-source call capture, use the Minutes desktop app.",
                        source[0]
                    );
                }
                config.recording.device = source.first().cloned();
            } else if call {
                // --call with auto-detect: resolve loopback device
                if let Some(loopback) = minutes_core::capture::detect_loopback_device() {
                    eprintln!(
                        "[minutes] Multi-source CLI recording is not yet implemented.\n\
                         Detected system audio device: {}\n\
                         For dual-source call capture, use the Minutes desktop app.\n\
                         Recording from microphone with --intent call.",
                        loopback
                    );
                } else {
                    eprintln!(
                        "[minutes] No system audio device detected.\n\
                         To capture call audio, install a loopback driver:\n\
                           macOS: brew install blackhole-2ch\n\
                         Or use the Minutes desktop app for native call capture."
                    );
                }
                if let Some(dev) = device {
                    config.recording.device = Some(dev);
                }
            } else if let Some(dev) = device {
                config.recording.device = Some(dev);
            }

            if let Some(wav_path) = diagnose {
                return cmd_diagnose(&wav_path, title.as_deref(), &config);
            }

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
                &config,
            )
        }
        Commands::Note { text, meeting } => cmd_note(&text, meeting.as_deref(), &config),
        Commands::Stop => cmd_stop(&config),
        Commands::Extend => {
            if !minutes_core::pid::status().recording {
                eprintln!("No active recording to extend.");
                std::process::exit(1);
            }
            minutes_core::capture::write_extend_sentinel()?;
            eprintln!("Recording extended — auto-stop timers reset.");
            Ok(())
        }
        Commands::ProcessQueue => cmd_process_queue(&config),
        Commands::PreflightRecord {
            mode,
            intent,
            allow_degraded,
            json,
        } => cmd_preflight_record(&mode, &intent, allow_degraded, json, &config),
        Commands::Status => cmd_status(),
        Commands::Jobs { all, json, limit } => cmd_jobs(all, json, limit),
        Commands::Paths { json } => cmd_paths(json, &config),
        Commands::Search {
            query,
            content_type,
            since,
            limit,
            intents_only,
            intent_kind,
            owner,
            format,
        } => cmd_search(
            &query,
            content_type,
            since,
            limit,
            intents_only,
            intent_kind,
            owner,
            &format,
            &config,
        ),
        Commands::Actions { assignee } => cmd_actions(assignee.as_deref(), &config),
        Commands::Consistency {
            owner,
            stale_after_days,
        } => cmd_consistency(owner.as_deref(), stale_after_days, &config),
        Commands::Person { name } => cmd_person(&name, &config),
        Commands::People {
            rebuild,
            json,
            limit,
        } => cmd_people(rebuild, json, limit, &config),
        Commands::Commitments { person, json } => cmd_commitments(person.as_deref(), json, &config),
        Commands::Research {
            query,
            content_type,
            since,
            attendee,
        } => cmd_research(&query, content_type, since, attendee, &config),
        Commands::List {
            limit,
            content_type,
        } => cmd_list(limit, content_type, &config),
        Commands::Export {
            content_type,
            output,
        } => cmd_export(content_type, output, &config),
        Commands::Clean { meeting, apply } => cmd_clean(&meeting, apply, &config),
        Commands::Process {
            path,
            content_type,
            note,
            title,
            language,
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            // Save note as context for the pipeline
            if let Some(ref n) = note {
                minutes_core::notes::save_context(n)?;
            }
            let result = cmd_process(&path, &content_type, title.as_deref(), &config);
            if note.is_some() {
                minutes_core::notes::cleanup();
            }
            result
        }
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
        } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
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
        } => {
            if parakeet {
                cmd_setup_parakeet(&parakeet_model)
            } else {
                cmd_setup(&model, list, diarization)
            }
        }
        Commands::Qmd { action, collection } => cmd_qmd(&action, &collection, &config),
        Commands::Service { action } => {
            #[cfg(target_os = "macos")]
            {
                cmd_service(&action)
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = action;
                eprintln!("The service command uses macOS launchd and is only available on macOS.");
                eprintln!("On Linux, use systemd or cron to run `minutes watch`.");
                eprintln!("On Windows, use Task Scheduler to run `minutes watch`.");
                Ok(())
            }
        }
        Commands::Logs { errors, lines } => cmd_logs(errors, lines),
        Commands::Health { json } => cmd_health(json),
        Commands::Demo => cmd_demo(&config),
        Commands::Schema => cmd_schema(),
        Commands::Get { slug } => cmd_get(&slug, &config),
        Commands::Events { limit, since } => cmd_events(limit, since, &config),
        Commands::Insights {
            kind,
            confidence,
            participant,
            since,
            limit,
            actionable,
        } => cmd_insights(kind, confidence, participant, since, limit, actionable),
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
        Commands::Live { language } => {
            if let Some(lang) = language {
                config.transcription.language = Some(lang);
            }
            cmd_live(&config)
        }
        Commands::Transcript {
            since,
            status,
            format,
        } => cmd_transcript(since.as_deref(), status, &format),
        Commands::Dashboard { port, no_open } => dashboard::serve(&config, port, !no_open),
    }
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

fn cmd_record(
    title: Option<String>,
    context: Option<String>,
    mode: &str,
    intent: &str,
    allow_degraded: bool,
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

    // Check for conflicting live transcript session
    let lt_pid = minutes_core::pid::live_transcript_pid_path();
    if let Ok(Some(_)) = minutes_core::pid::check_pid_file(&lt_pid) {
        anyhow::bail!("live transcript in progress — run `minutes stop` first");
    }

    // Check if already recording
    minutes_core::pid::create().map_err(|e| anyhow::anyhow!("{}", e))?;
    minutes_core::pid::write_recording_metadata(capture_mode).ok();
    let recording_started_at = Local::now();

    // Save recording start time (for timestamping notes)
    minutes_core::notes::save_recording_start()?;

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
        if stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
            eprintln!("\nForce quit.");
            std::process::exit(1);
        }
        eprintln!("\nStopping recording... (Ctrl+C again to force quit)");
        stop_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    })?;

    // Ignore SIGTERM — `minutes stop` uses sentinel file for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    // Record audio from default input device
    let wav_path = minutes_core::pid::current_wav_path();
    minutes_core::capture::record_to_wav(&wav_path, stop_flag, config)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    let recording_finished_at = Local::now();
    let user_notes = minutes_core::notes::read_notes();
    let pre_context = minutes_core::notes::read_context();
    let calendar_event =
        if capture_mode.content_type() == ContentType::Meeting && config.calendar.enabled {
            minutes_core::calendar::events_overlapping_now()
                .into_iter()
                .next()
        } else {
            None
        };
    let queued = (|| -> Result<(minutes_core::jobs::ProcessingJob, String)> {
        let job = minutes_core::jobs::queue_live_capture(
            capture_mode,
            title.clone(),
            &wav_path,
            user_notes,
            pre_context,
            Some(recording_started_at),
            Some(recording_finished_at),
            calendar_event,
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
    if minutes_core::jobs::current_worker_pid().is_some() {
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

fn cmd_stop(_config: &Config) -> Result<()> {
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
                #[cfg(feature = "diarize")]
                if let Err(e) = minutes_core::graph::rebuild_index(_config) {
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
            // No batch recording — check for live transcript session
            let lt_pid_path = minutes_core::pid::live_transcript_pid_path();
            match minutes_core::pid::check_pid_file(&lt_pid_path) {
                Ok(Some(pid)) => {
                    eprintln!("Stopping live transcript (PID {})...", pid);
                    minutes_core::pid::write_stop_sentinel()
                        .map_err(|e| anyhow::anyhow!("failed to write stop sentinel: {}", e))?;
                    #[cfg(unix)]
                    {
                        let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
                        if rc != 0 {
                            tracing::warn!("SIGTERM failed for live transcript PID {}", pid);
                        }
                    }
                    // Poll for PID removal
                    let start = std::time::Instant::now();
                    eprint!("Finalizing live transcript");
                    while lt_pid_path.exists()
                        && start.elapsed() < std::time::Duration::from_secs(30)
                    {
                        std::thread::sleep(std::time::Duration::from_millis(500));
                        eprint!(".");
                    }
                    eprintln!();
                    if lt_pid_path.exists() {
                        anyhow::bail!("live transcript process did not stop within 30 seconds");
                    }
                    eprintln!("Live transcript stopped.");
                    Ok(())
                }
                _ => {
                    eprintln!("No recording or live transcript in progress.");
                    Ok(())
                }
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
        let title = job.title.unwrap_or_else(|| "Queued recording".into());
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
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("config_path: {}", report.config_path.display());
        println!("minutes_dir: {}", report.minutes_dir.display());
        println!("output_dir: {}", report.output_dir.display());
    }

    Ok(())
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
    config: &Config,
) -> Result<()> {
    let json_mode = format == "json";
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee: None,
        intent_kind: intent_kind.as_deref().map(parse_intent_kind).transpose()?,
        owner,
        recorded_by: None,
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
                let who = result.who.as_deref().unwrap_or("unassigned");
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

    let results = minutes_core::search::search(query, config, &filters)?;
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

fn cmd_actions(assignee: Option<&str>, config: &Config) -> Result<()> {
    let results = minutes_core::search::find_open_actions(config, assignee)
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

fn cmd_list(limit: usize, content_type: Option<String>, config: &Config) -> Result<()> {
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
            let who = stale.entry.who.as_deref().unwrap_or("unassigned");
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

fn cmd_people(rebuild: bool, json: bool, limit: usize, config: &Config) -> Result<()> {
    use minutes_core::graph;

    if rebuild || !graph::db_path().exists() {
        eprintln!("Building relationship index...");
        let stats = graph::rebuild_index(config).map_err(|e| anyhow::anyhow!("{}", e))?;
        eprintln!(
            "Index rebuilt: {} people, {} meetings, {} commitments in {}ms",
            stats.people_count, stats.meeting_count, stats.commitment_count, stats.rebuild_ms
        );
        if !stats.alias_suggestions.is_empty() {
            eprintln!("\nPossible duplicates:");
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
    config: &Config,
) -> Result<()> {
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee,
        intent_kind: None,
        owner: None,
        recorded_by: None,
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
            let owner = intent.who.as_deref().unwrap_or("unassigned");
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

fn cmd_process(
    path: &Path,
    content_type: &str,
    title: Option<&str>,
    config: &Config,
) -> Result<()> {
    if !path.exists() {
        anyhow::bail!("file not found: {}", path.display());
    }

    let ct = match content_type {
        "meeting" => ContentType::Meeting,
        "memo" => ContentType::Memo,
        other => anyhow::bail!("unknown content type: {}. Use 'meeting' or 'memo'.", other),
    };

    config.ensure_dirs()?;
    let result = minutes_core::process(path, ct, title, config)?;
    eprintln!("Saved: {}", result.path.display());

    let json = serde_json::to_string_pretty(&serde_json::json!({
        "status": "done",
        "file": result.path.display().to_string(),
        "title": result.title,
        "words": result.word_count,
    }))?;
    println!("{}", json);
    Ok(())
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
    let diarize_result = minutes_core::diarize::diarize(path, config);
    let diarization_embeddings = match &diarize_result {
        Some(result) => {
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
        None => {
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

fn cmd_watch(dir: Option<&Path>, config: &Config) -> Result<()> {
    config.ensure_dirs()?;

    // Set up Ctrl-C to release the lock and exit cleanly
    ctrlc::set_handler(move || {
        eprintln!("\nStopping watcher...");
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
    eprintln!("\nTip: Use a PulseAudio monitor source for system audio capture");

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
        eprintln!("    PipeWire monitor sources should appear here automatically");
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
        eprintln!("Parakeet models (alternative engine, --parakeet):");
        eprintln!("  tdt-ctc-110m  ~220 MB   (English, fast, default)");
        eprintln!("  tdt-600m      ~1.2 GB   (multilingual, 25 EU languages, best quality)");
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
    if dest.exists() {
        let size = std::fs::metadata(&dest)?.len();
        eprintln!(
            "Model already downloaded: {} ({:.0} MB)",
            dest.display(),
            size as f64 / 1_048_576.0
        );
    } else {
        let url = format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
            model
        );

        eprintln!("Downloading whisper model: {} ...", model);
        download_file(&url, &dest)?;

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

fn cmd_setup_diarization() -> Result<()> {
    use minutes_core::diarize;

    let config = Config::load();
    let emb_info = diarize::embedding_model_for_config(&config);
    let model_dir = &config.diarization.model_path;
    std::fs::create_dir_all(model_dir)?;

    eprintln!("Embedding model: {} ({})", config.diarization.embedding_model, emb_info.filename);

    let models: [(& str, &str, &str); 2] = [
        (
            diarize::SEGMENTATION_MODEL,
            diarize::SEGMENTATION_MODEL_URL,
            "segmentation",
        ),
        (
            emb_info.filename,
            emb_info.url,
            "speaker embedding",
        ),
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
    let valid_models = ["tdt-ctc-110m", "tdt-600m"];
    if !valid_models.contains(&model) {
        anyhow::bail!(
            "unknown parakeet model: {}. Available: {}",
            model,
            valid_models.join(", ")
        );
    }

    let config = Config::default();
    let model_dir = config.transcription.model_path.join("parakeet");
    std::fs::create_dir_all(&model_dir)?;

    let dest_model = model_dir.join(format!("{}.safetensors", model));
    let dest_vocab = model_dir.join("vocab.txt");

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
    } else {
        eprintln!("Parakeet model setup: {}", model);
        eprintln!();
        eprintln!("Parakeet models require a one-time conversion from NVIDIA's .nemo format.");
        eprintln!("Follow these steps:");
        eprintln!();
        eprintln!("  Step 1: Clone parakeet.cpp");
        eprintln!("    git clone https://github.com/Frikallo/parakeet.cpp");
        eprintln!("    cd parakeet.cpp");
        eprintln!();
        eprintln!("  Step 2: Download the .nemo model from HuggingFace");
        eprintln!(
            "    huggingface-cli download {} --include '*.nemo' --local-dir .",
            hf_repo
        );
        eprintln!();
        eprintln!("  Step 3: Convert to safetensors (generates model + vocab)");
        eprintln!(
            "    python scripts/convert_nemo.py *.nemo -o {}",
            dest_model.display()
        );
        eprintln!();
        eprintln!("  Step 4: Copy the vocab file");
        eprintln!("    cp vocab.txt {}", dest_vocab.display());
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

    // Verify parakeet binary is available
    eprintln!();
    match std::process::Command::new("parakeet")
        .arg("--help")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            eprintln!("parakeet binary found in PATH.");
        }
        _ => {
            eprintln!("Warning: `parakeet` binary not found in PATH.");
            eprintln!("See: https://github.com/Frikallo/parakeet.cpp");
        }
    }

    eprintln!();
    eprintln!("To use parakeet, add to ~/.config/minutes/config.toml:");
    eprintln!("  [transcription]");
    eprintln!("  engine = \"parakeet\"");
    eprintln!("  parakeet_model = \"{}\"", model);

    Ok(())
}

/// Download a file from a URL to a destination path, with progress reporting.
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
    let plist_name = "dev.getminutes.watcher";
    let plist_dest = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", plist_name));

    match action {
        "install" => {
            let minutes_bin = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("minutes"));
            let home = dirs::home_dir().unwrap_or_default();
            let log_dir = Config::minutes_dir().join("logs");
            std::fs::create_dir_all(&log_dir)?;
            std::fs::create_dir_all(home.join("Library/LaunchAgents"))?;

            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
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
        <string>{home}/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{log}</string>
    <key>StandardErrorPath</key>
    <string>{log}</string>
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
                label = plist_name,
                bin = minutes_bin.display(),
                home = home.display(),
                log = log_dir.join("watcher.log").display(),
            );

            std::fs::write(&plist_dest, &plist)?;

            let status = std::process::Command::new("launchctl")
                .args(["load", "-w", &plist_dest.to_string_lossy()])
                .status()?;

            if status.success() {
                eprintln!("Watcher service installed and started.");
                eprintln!("  Plist: {}", plist_dest.display());
                eprintln!("  Logs:  {}", log_dir.join("watcher.log").display());
                eprintln!("  It will auto-start on login and process audio in ~/.minutes/inbox/");
            } else {
                anyhow::bail!("launchctl load failed");
            }
        }
        "uninstall" => {
            if plist_dest.exists() {
                let _ = std::process::Command::new("launchctl")
                    .args(["unload", &plist_dest.to_string_lossy()])
                    .status();
                std::fs::remove_file(&plist_dest)?;
                eprintln!("Watcher service uninstalled.");
            } else {
                eprintln!("Service not installed.");
            }
        }
        "status" => {
            let output = std::process::Command::new("launchctl")
                .args(["list", plist_name])
                .output()?;
            if output.status.success() {
                eprintln!("Watcher service is running.");
                let stdout = String::from_utf8_lossy(&output.stdout);
                for line in stdout.lines() {
                    if line.contains("PID") || line.contains("LastExitStatus") {
                        eprintln!("  {}", line.trim());
                    }
                }
            } else {
                eprintln!("Watcher service is not running.");
                if plist_dest.exists() {
                    eprintln!("  Plist exists at: {}", plist_dest.display());
                    eprintln!("  Try: minutes service install");
                } else {
                    eprintln!("  Not installed. Run: minutes service install");
                }
            }
        }
        _ => anyhow::bail!(
            "Unknown action: {}. Use install, uninstall, or status.",
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
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard, OnceLock};

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
    fn parse_qmd_collection_path_reads_show_output() {
        let output = r#"Collection: minutes
  Path:     /Users/silverbook/meetings
  Pattern:  **/*.md
  Include:  yes (default)
"#;

        assert_eq!(
            parse_qmd_collection_path(output),
            Some(PathBuf::from("/Users/silverbook/meetings"))
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
    fn cmd_delete_force_permanently_removes() {
        with_temp_home(|dir| {
            let meetings = dir.join("meetings");
            std::fs::create_dir_all(&meetings).unwrap();
            let md = meetings.join("2026-04-01-force.md");
            std::fs::write(&md, "---\ntitle: Force\n---\nContent").unwrap();
            let wav = meetings.join("2026-04-01-force.wav");
            std::fs::write(&wav, b"fake audio").unwrap();

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
            assert!(
                !meetings.join("archive/2026-04-01-force.md").exists(),
                "nothing in archive for force delete"
            );
        });
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

    // Find associated audio file (.wav with same stem in same directory)
    let audio_path = md_path.with_extension("wav");
    let has_audio = audio_path.exists();

    if force {
        // Permanent delete
        std::fs::remove_file(&md_path)?;
        eprintln!("Deleted: {}", md_path.display());

        if with_audio && has_audio {
            std::fs::remove_file(&audio_path)?;
            eprintln!("Deleted audio: {}", audio_path.display());
        }
    } else {
        // Soft delete: move to archive directory
        let archive_dir = config.output_dir.join("archive");
        std::fs::create_dir_all(&archive_dir)?;

        let dest_md = archive_dir.join(md_path.file_name().unwrap());
        std::fs::rename(&md_path, &dest_md)?;
        eprintln!("Archived: {} → {}", title, dest_md.display());

        if with_audio && has_audio {
            let dest_audio = archive_dir.join(audio_path.file_name().unwrap());
            std::fs::rename(&audio_path, &dest_audio)?;
            eprintln!("Archived audio: {}", dest_audio.display());
        }
    }

    if has_audio && !with_audio {
        eprintln!(
            "Note: audio file still exists at {}. Use --with-audio to remove it.",
            audio_path.display()
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

fn cmd_get(slug: &str, config: &Config) -> Result<()> {
    match minutes_core::search::resolve_slug(slug, config) {
        Some(path) => {
            let content = std::fs::read_to_string(&path)?;
            println!("{}", content);
            Ok(())
        }
        None => {
            anyhow::bail!("no meeting found matching slug: {}", slug);
        }
    }
}

fn cmd_events(limit: usize, since: Option<String>, _config: &Config) -> Result<()> {
    let since_dt = since.as_deref().and_then(|s| {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
            .ok()
            .and_then(|d| d.and_hms_opt(0, 0, 0))
            .and_then(|ndt| chrono::Local.from_local_datetime(&ndt).single())
    });

    let events = minutes_core::events::read_events(since_dt, Some(limit));
    let json = serde_json::to_string_pretty(&events)?;
    println!("{}", json);
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

// ── Import ──────────────────────────────────────────────────

fn cmd_import(from: &str, dir: Option<&Path>, dry_run: bool, config: &Config) -> Result<()> {
    match from {
        "granola" => import_granola(dir, dry_run, config),
        other => anyhow::bail!("Unknown import source: {}. Supported: granola", other),
    }
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
             Export your Granola meetings first, or specify a directory with --dir",
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
        if !attendees_line.is_empty() && attendees_line != "None" {
            output.push_str(&format!("attendees_raw: {}\n", attendees_line));
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
        println!("{}", serde_json::to_string_pretty(&items)?);
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
// minutes demo — deterministic pipeline demo with bundled audio
// ──────────────────────────────────────────────────────────────

/// Bundled 3-second demo WAV (generated silence with a beep).
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
            eprintln!("Run `minutes setup` to download the speech model first.");
            Err(e.into())
        }
    }
}

fn cmd_dictate(stdout: bool, note_only: bool, config: &Config) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

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
        if stop_clone.load(Ordering::Relaxed) {
            eprintln!("\nForce quit.");
            std::process::exit(1);
        }
        eprintln!("\nStopping dictation... (Ctrl+C again to force quit)");
        stop_clone.store(true, Ordering::Relaxed);
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
        },
    )?;

    Ok(())
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
    voice::save_profile_blended(&conn, &slug, &my_name, &embedding, "self-enrollment", voice::model_version(config))
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
    use minutes_core::voice;

    if !meeting_path.exists() {
        return Err(anyhow::anyhow!(
            "Meeting not found: {}",
            meeting_path.display()
        ));
    }

    // Read the meeting file
    let content = std::fs::read_to_string(meeting_path)?;
    let (yaml_str, body) = minutes_core::markdown::split_frontmatter(&content);

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

    // Non-interactive mode: confirm a specific speaker
    if let (Some(speaker_label), Some(new_name)) = (speaker, name) {
        let found = frontmatter
            .speaker_map
            .iter_mut()
            .find(|a| a.speaker_label == speaker_label);

        if let Some(attr) = found {
            let old_confidence = attr.confidence;
            attr.name = new_name.to_string();
            attr.confidence = Confidence::High;
            attr.source = AttributionSource::Manual;
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
                        voice::save_profile_blended(&conn, &slug, new_name, embedding, "confirmed", voice::model_version(config))
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
                attr.confidence = Confidence::High;
                attr.source = AttributionSource::Manual;
                eprintln!("    → Confirmed: {} = {}", attr.speaker_label, attr.name);
            } else if input.eq_ignore_ascii_case("n") || input.eq_ignore_ascii_case("no") {
                eprintln!("    → Skipped");
            } else {
                // User typed a different name
                attr.name = input.to_string();
                attr.confidence = Confidence::High;
                attr.source = AttributionSource::Manual;
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

    // Rewrite the meeting file with updated speaker_map
    // Also apply High-confidence names to transcript body
    let new_body = minutes_core::diarize::apply_confirmed_names(body, &frontmatter.speaker_map);
    let new_yaml = serde_yaml::to_string(&frontmatter)
        .map_err(|e| anyhow::anyhow!("Failed to serialize frontmatter: {}", e))?;
    let new_content = format!("---\n{}---\n{}", new_yaml, new_body);
    std::fs::write(meeting_path, new_content)?;

    let confirmed_count = frontmatter
        .speaker_map
        .iter()
        .filter(|a| a.confidence == Confidence::High)
        .count();
    eprintln!(
        "\nMeeting updated: {}/{} speakers confirmed.",
        confirmed_count,
        frontmatter.speaker_map.len()
    );

    Ok(())
}

fn cmd_live(config: &Config) -> Result<()> {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    eprintln!("Starting live transcript session...");
    eprintln!("Press Ctrl-C or run `minutes stop` to end.\n");

    let stop = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop);

    // Handle Ctrl-C (double press to force quit)
    ctrlc::set_handler(move || {
        if stop_clone.load(Ordering::Relaxed) {
            eprintln!("\nForce quit.");
            std::process::exit(1);
        }
        eprintln!("\nStopping gracefully... (Ctrl+C again to force quit)");
        stop_clone.store(true, Ordering::Relaxed);
    })
    .ok();

    // Ignore SIGTERM — `minutes stop` uses sentinel file for graceful shutdown
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
    }

    // No sentinel watcher needed — run_inner already polls check_and_clear_sentinel
    // directly in its main loop, avoiding the thread-join and double-consume race.
    match minutes_core::live_transcript::run(stop, config) {
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

fn cmd_transcript(since: Option<&str>, status: bool, format: &str) -> Result<()> {
    if status {
        let s = minutes_core::live_transcript::session_status();
        if format == "json" {
            println!("{}", serde_json::to_string_pretty(&s)?);
        } else {
            if s.active {
                eprintln!("Live transcript: ACTIVE (PID: {})", s.pid.unwrap_or(0));
            } else {
                eprintln!("Live transcript: inactive");
            }
            eprintln!("  Lines: {}", s.line_count);
            eprintln!("  Duration: {:.0}s", s.duration_secs);
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
