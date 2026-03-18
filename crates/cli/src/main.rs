use anyhow::Result;
use clap::{Parser, Subcommand};
use minutes_core::{Config, ContentType};
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
    },

    /// Show open action items across all meetings
    Actions {
        /// Filter by assignee name
        #[arg(short, long)]
        assignee: Option<String>,
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
        Commands::Record { title, context } => cmd_record(title, context, &config),
        Commands::Note { text, meeting } => cmd_note(&text, meeting.as_deref(), &config),
        Commands::Stop => cmd_stop(&config),
        Commands::Status => cmd_status(),
        Commands::Search {
            query,
            content_type,
            since,
            limit,
        } => cmd_search(&query, content_type, since, limit, &config),
        Commands::Actions { assignee } => cmd_actions(assignee.as_deref(), &config),
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

fn cmd_record(title: Option<String>, context: Option<String>, config: &Config) -> Result<()> {
    // Ensure directories exist
    config.ensure_dirs()?;

    // Check if already recording
    minutes_core::pid::create().map_err(|e| anyhow::anyhow!("{}", e))?;

    // Save recording start time (for timestamping notes)
    minutes_core::notes::save_recording_start()?;

    // Save pre-meeting context if provided
    if let Some(ref ctx) = context {
        minutes_core::notes::save_context(ctx)?;
        eprintln!("Context saved: {}", ctx);
    }

    eprintln!("Recording... (press Ctrl-C or run `minutes stop` to finish)");
    eprintln!("  Tip: add notes with `minutes note \"your note\"` in another terminal");

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
    let content_type = ContentType::Meeting;
    let result = minutes_core::process(&wav_path, content_type, title.as_deref(), config)?;

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

            eprint!("Transcribing");
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
    config: &Config,
) -> Result<()> {
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since,
        attendee: None,
    };

    let results = minutes_core::search::search(query, config, &filters)?;
    let limited: Vec<_> = results.into_iter().take(limit).collect();

    if limited.is_empty() {
        eprintln!("No results found for \"{}\"", query);
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
    cmd_search("", content_type, None, limit, config)
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

fn cmd_service(action: &str) -> Result<()> {
    let plist_name = "dev.getminutes.watcher";
    let plist_dest = dirs::home_dir()
        .unwrap_or_default()
        .join("Library/LaunchAgents")
        .join(format!("{}.plist", plist_name));

    match action {
        "install" => {
            let minutes_bin = std::env::current_exe()
                .unwrap_or_else(|_| PathBuf::from("minutes"));
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
        _ => anyhow::bail!("Unknown action: {}. Use install, uninstall, or status.", action),
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

// Frontmatter parsing is in minutes_core::markdown::{split_frontmatter, extract_field}
