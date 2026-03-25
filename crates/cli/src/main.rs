use anyhow::Result;
use chrono::TimeZone;
use clap::{Parser, Subcommand};
use minutes_core::{CaptureMode, Config, ContentType};
use serde::Serialize;
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

    /// Check if a recording is in progress
    Status,

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
    },

    /// Watch a folder for new audio files and process them automatically
    Watch {
        /// Directory to watch (default: ~/.minutes/inbox/)
        dir: Option<PathBuf>,
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
    },

    /// List available audio input devices
    Devices,

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

    let config = Config::load();

    // Rotate old log files at startup
    minutes_core::logging::rotate_logs().ok();

    match cli.command {
        Commands::Record {
            title,
            context,
            mode,
        } => cmd_record(title, context, &mode, &config),
        Commands::Note { text, meeting } => cmd_note(&text, meeting.as_deref(), &config),
        Commands::Stop => cmd_stop(&config),
        Commands::Status => cmd_status(),
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
        } => {
            #[cfg(feature = "diarize")]
            {
                cmd_people(rebuild, json, limit, &config)
            }
            #[cfg(not(feature = "diarize"))]
            {
                let _ = (rebuild, json, limit);
                anyhow::bail!("The `people` command requires the diarize feature. Rebuild with: cargo install --path crates/cli")
            }
        }
        Commands::Commitments { person, json } => {
            #[cfg(feature = "diarize")]
            {
                cmd_commitments(person.as_deref(), json, &config)
            }
            #[cfg(not(feature = "diarize"))]
            {
                let _ = (person, json);
                anyhow::bail!("The `commitments` command requires the diarize feature. Rebuild with: cargo install --path crates/cli")
            }
        }
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
        Commands::Process {
            path,
            content_type,
            note,
            title,
        } => {
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
        Commands::Watch { dir } => cmd_watch(dir.as_deref(), &config),
        Commands::Dictate { stdout, note_only } => cmd_dictate(stdout, note_only, &config),
        Commands::Devices => cmd_devices(),
        Commands::Setup {
            model,
            list,
            diarization,
        } => cmd_setup(&model, list, diarization),
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
        Commands::Import { from, dir, dry_run } => {
            cmd_import(&from, dir.as_deref(), dry_run, &config)
        }
        Commands::Vault { action } => match action {
            VaultAction::Setup { path, strategy } => cmd_vault_setup(path, strategy, config),
            VaultAction::Status => cmd_vault_status(&config),
            VaultAction::Unlink => cmd_vault_unlink(config),
            VaultAction::Sync => cmd_vault_sync(&config),
        },
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

fn live_stage_label(
    stage: minutes_core::pipeline::PipelineStage,
    mode: CaptureMode,
) -> &'static str {
    match (stage, mode) {
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::Meeting) => {
            "Transcribing meeting"
        }
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::QuickThought) => {
            "Transcribing quick thought"
        }
        (minutes_core::pipeline::PipelineStage::Diarizing, _) => "Separating speakers",
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::Meeting) => {
            "Generating meeting summary"
        }
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::QuickThought) => {
            "Generating memo summary"
        }
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::Meeting) => "Saving meeting",
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::QuickThought) => {
            "Saving quick thought"
        }
        (minutes_core::pipeline::PipelineStage::Transcribing, CaptureMode::Dictation) => {
            "Transcribing dictation"
        }
        (minutes_core::pipeline::PipelineStage::Summarizing, CaptureMode::Dictation) => {
            "Generating dictation summary"
        }
        (minutes_core::pipeline::PipelineStage::Saving, CaptureMode::Dictation) => {
            "Saving dictation"
        }
    }
}

fn cmd_record(
    title: Option<String>,
    context: Option<String>,
    mode: &str,
    config: &Config,
) -> Result<()> {
    // Ensure directories exist
    config.ensure_dirs()?;
    let capture_mode = capture_mode_from_str(mode)?;

    // Check if already recording
    minutes_core::pid::create().map_err(|e| anyhow::anyhow!("{}", e))?;
    minutes_core::pid::write_recording_metadata(capture_mode).ok();

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
    }

    // Set up stop flag for signal handler
    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = std::sync::Arc::clone(&stop_flag);
    ctrlc::set_handler(move || {
        eprintln!("\nStopping recording...");
        stop_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    })?;

    // Record audio from default input device
    let wav_path = minutes_core::pid::current_wav_path();
    minutes_core::capture::record_to_wav(&wav_path, stop_flag, config)
        .map_err(|e| anyhow::anyhow!("{}", e))?;

    // Run pipeline on the captured audio
    let content_type = capture_mode.content_type();
    let result = minutes_core::pipeline::process_with_progress(
        &wav_path,
        content_type,
        title.as_deref(),
        config,
        |stage| {
            let label = live_stage_label(stage, capture_mode);
            let _ = minutes_core::pid::set_processing_status(Some(label), Some(capture_mode));
        },
    );

    if let Err(err) = result {
        minutes_core::pid::remove().ok();
        minutes_core::pid::clear_processing_status().ok();
        minutes_core::pid::clear_recording_metadata().ok();
        minutes_core::notes::cleanup();
        return Err(err.into());
    }

    let result = result?;

    // Emit RecordingCompleted event (AudioProcessed already emitted by pipeline)
    minutes_core::events::append_event(minutes_core::events::recording_completed_event(
        &result, "live",
    ));

    // Write result file for `minutes stop` to read
    let result_json = serde_json::to_string_pretty(&serde_json::json!({
        "status": "done",
        "file": result.path.display().to_string(),
        "title": result.title,
        "words": result.word_count,
    }))?;
    std::fs::write(minutes_core::pid::last_result_path(), &result_json)?;

    // Clean up
    minutes_core::pid::remove().ok();
    minutes_core::pid::clear_processing_status().ok();
    minutes_core::pid::clear_recording_metadata().ok();
    minutes_core::notes::cleanup(); // Remove notes + context + recording-start files
    if wav_path.exists() {
        std::fs::remove_file(&wav_path).ok();
    }

    eprintln!("Saved: {}", result.path.display());
    // Print JSON to stdout for programmatic consumption (MCPB)
    println!("{}", result_json);

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
                eprintln!("Recording stopped but no result file found.");
            }

            Ok(())
        }
        Ok(None) => {
            eprintln!("No recording in progress.");
            Ok(())
        }
        Err(e) => Err(anyhow::anyhow!("{}", e)),
    }
}

fn cmd_status() -> Result<()> {
    let status = minutes_core::pid::status();
    let json = serde_json::to_string_pretty(&status)?;
    println!("{}", json);
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

#[cfg(feature = "diarize")]
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

    let people = graph::relationship_map(config).map_err(|e| anyhow::anyhow!("{}", e))?;

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

#[cfg(feature = "diarize")]
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

    let config = Config::default();
    let model_dir = &config.diarization.model_path;
    std::fs::create_dir_all(model_dir)?;

    let models = [
        (
            diarize::SEGMENTATION_MODEL,
            diarize::SEGMENTATION_MODEL_URL,
            "segmentation",
        ),
        (
            diarize::EMBEDDING_MODEL,
            diarize::EMBEDDING_MODEL_URL,
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
}

// Frontmatter parsing is in minutes_core::markdown::{split_frontmatter, extract_field}

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
    mut config: Config,
) -> Result<()> {
    use minutes_core::vault;

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
    eprintln!("[minutes] Speak naturally. Text goes to clipboard after each pause.");

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_clone = Arc::clone(&stop_flag);

    // Handle Ctrl-C
    ctrlc::set_handler(move || {
        eprintln!("\nStopping dictation...");
        stop_clone.store(true, Ordering::Relaxed);
    })?;

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
                    eprintln!("[minutes] Done — text copied to clipboard");
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
