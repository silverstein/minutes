use anyhow::Result;
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
        } => cmd_search(
            &query,
            content_type,
            since,
            limit,
            intents_only,
            intent_kind,
            owner,
            &config,
        ),
        Commands::Actions { assignee } => cmd_actions(assignee.as_deref(), &config),
        Commands::Consistency {
            owner,
            stale_after_days,
        } => cmd_consistency(owner.as_deref(), stale_after_days, &config),
        Commands::Person { name } => cmd_person(&name, &config),
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
        Commands::Devices => cmd_devices(),
        Commands::Setup { model, list } => cmd_setup(&model, list),
        Commands::Qmd { action, collection } => cmd_qmd(&action, &collection, &config),
        Commands::Service { action } => cmd_service(&action),
        Commands::Logs { errors, lines } => cmd_logs(errors, lines),
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

            // Send SIGTERM to the recording process
            let rc = unsafe { libc::kill(pid as i32, libc::SIGTERM) };
            if rc != 0 {
                let err = std::io::Error::last_os_error();
                anyhow::bail!("could not signal recording process (PID {}): {}", pid, err);
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

fn cmd_search(
    query: &str,
    content_type: Option<String>,
    since: Option<String>,
    limit: usize,
    intents_only: bool,
    intent_kind: Option<String>,
    owner: Option<String>,
    config: &Config,
) -> Result<()> {
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee: None,
        intent_kind: intent_kind.as_deref().map(parse_intent_kind).transpose()?,
        owner,
    };

    if intents_only {
        let results = minutes_core::search::search_intents(query, config, &filters)
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        let limited: Vec<_> = results.into_iter().take(limit).collect();

        if limited.is_empty() {
            eprintln!("No intent records found for \"{}\"", query);
            println!("[]");
            return Ok(());
        }

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
        return Ok(());
    }

    let results = minutes_core::search::search(query, config, &filters)?;
    let limited: Vec<_> = results.into_iter().take(limit).collect();

    if limited.is_empty() {
        eprintln!("No results found for \"{}\"", query);
        println!("[]");
        return Ok(());
    }

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
    cmd_search("", content_type, None, limit, false, None, None, config)
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
    Ok(())
}

fn cmd_setup(model: &str, list: bool) -> Result<()> {
    if list {
        eprintln!("Available whisper models:");
        eprintln!("  tiny      75 MB   (fastest, lowest quality)");
        eprintln!("  base     142 MB");
        eprintln!("  small    466 MB   (recommended default)");
        eprintln!("  medium   1.5 GB");
        eprintln!("  large-v3 3.1 GB   (best quality, slower)");
        return Ok(());
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
        return Ok(());
    }

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model
    );

    eprintln!("Downloading whisper model: {} ...", model);
    eprintln!("  From: {}", url);
    eprintln!("  To:   {}", dest.display());

    // Use curl for the download (available on all macOS systems)
    let status = std::process::Command::new("curl")
        .args(["-L", "-o", dest.to_str().unwrap(), &url, "--progress-bar"])
        .status()?;

    if !status.success() {
        // Clean up partial download
        std::fs::remove_file(&dest).ok();
        anyhow::bail!("download failed. Check your internet connection and try again.");
    }

    let size = std::fs::metadata(&dest)?.len();
    eprintln!(
        "\nDone! Model saved to {} ({:.0} MB)",
        dest.display(),
        size as f64 / 1_048_576.0
    );

    // Update config hint
    eprintln!("\nTo use this model, add to ~/.config/minutes/config.toml:");
    eprintln!("  [transcription]");
    eprintln!("  model = \"{}\"", model);

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
