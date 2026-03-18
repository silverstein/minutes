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

    match cli.command {
        Commands::Record { title } => cmd_record(title, &config),
        Commands::Stop => cmd_stop(&config),
        Commands::Status => cmd_status(),
        Commands::Search {
            query,
            content_type,
            since,
            limit,
        } => cmd_search(&query, content_type, since, limit, &config),
        Commands::List {
            limit,
            content_type,
        } => cmd_list(limit, content_type, &config),
        Commands::Process {
            path,
            content_type,
            title,
        } => cmd_process(&path, &content_type, title.as_deref(), &config),
        Commands::Watch { dir } => cmd_watch(dir.as_deref(), &config),
        Commands::Setup { model, list } => cmd_setup(&model, list),
        Commands::Logs { errors, lines } => cmd_logs(errors, lines),
    }
}

fn cmd_record(title: Option<String>, config: &Config) -> Result<()> {
    // Ensure directories exist
    config.ensure_dirs()?;

    // Check if already recording
    minutes_core::pid::create().map_err(|e| anyhow::anyhow!("{}", e))?;

    eprintln!("Recording... (press Ctrl-C or run `minutes stop` to finish)");

    // Set up signal handler for graceful shutdown
    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        tx.send(()).ok();
    })?;

    // TODO(P1a.2): Replace with actual audio capture via cpal + BlackHole
    // For now, create a placeholder WAV file so the pipeline can be tested
    let wav_path = minutes_core::pid::current_wav_path();
    create_placeholder_wav(&wav_path)?;

    // Wait for stop signal (Ctrl-C or SIGTERM)
    rx.recv().ok();

    eprintln!("\nStopping recording...");

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
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }

            // Poll for PID file removal (recording process cleans up on exit)
            let timeout = std::time::Duration::from_secs(120);
            let start = std::time::Instant::now();
            let pid_path = minutes_core::pid::pid_path();

            while pid_path.exists() && start.elapsed() < timeout {
                std::thread::sleep(std::time::Duration::from_millis(500));
            }

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

fn cmd_list(limit: usize, content_type: Option<String>, config: &Config) -> Result<()> {
    // List is just search with an empty query — returns all files
    let filters = minutes_core::search::SearchFilters {
        content_type,
        since: None,
        attendee: None,
    };

    // Walk directory and collect all markdown files with frontmatter
    let dir = &config.output_dir;
    if !dir.exists() {
        eprintln!("No meetings directory found at {}", dir.display());
        return Ok(());
    }

    let mut entries: Vec<minutes_core::search::SearchResult> = Vec::new();
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
    {
        let path = entry.path();
        let content = std::fs::read_to_string(path)?;
        let title = extract_title(&content)
            .unwrap_or_else(|| path.file_stem().unwrap().to_string_lossy().to_string());
        let date = extract_date(&content).unwrap_or_default();
        let ct = extract_type(&content).unwrap_or_else(|| "meeting".into());

        if let Some(ref type_filter) = filters.content_type {
            if ct != *type_filter {
                continue;
            }
        }

        entries.push(minutes_core::search::SearchResult {
            path: path.to_path_buf(),
            title,
            date,
            content_type: ct,
            snippet: String::new(),
        });
    }

    entries.sort_by(|a, b| b.date.cmp(&a.date));
    let limited: Vec<_> = entries.into_iter().take(limit).collect();

    if limited.is_empty() {
        eprintln!("No meetings or memos found.");
        return Ok(());
    }

    for entry in &limited {
        eprintln!(
            "  {} — {} [{}]",
            entry.date, entry.title, entry.content_type
        );
    }

    let json = serde_json::to_string_pretty(&limited)?;
    println!("{}", json);
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

fn cmd_watch(dir: Option<&Path>, config: &Config) -> Result<()> {
    config.ensure_dirs()?;

    // Set up Ctrl-C handler to exit gracefully
    let (tx, rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        tx.send(()).ok();
    })?;

    // Run watcher in a separate thread so we can catch Ctrl-C
    let config_clone = config.clone();
    let dir_clone = dir.map(|d| d.to_path_buf());
    let watcher_thread =
        std::thread::spawn(move || minutes_core::watch::run(dir_clone.as_deref(), &config_clone));

    // Wait for Ctrl-C
    rx.recv().ok();
    eprintln!("\nStopping watcher...");

    // The watcher thread will be cleaned up when the process exits
    // The LockGuard in watch.rs will release the lock on drop
    drop(watcher_thread);

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

    // TODO(P1a.8): Implement actual model download from huggingface
    eprintln!("Model download not yet implemented.");
    eprintln!("For now, manually download the {} model from:", model);
    eprintln!("  https://huggingface.co/ggerganov/whisper.cpp/tree/main");
    eprintln!("Place the ggml-{}.bin file in:", model);
    eprintln!("  {}", Config::default().transcription.model_path.display());

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

/// Create a small placeholder WAV file for pipeline testing.
fn create_placeholder_wav(path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    let mut writer = hound::WavWriter::create(path, spec)?;
    // Write 1 second of silence (16000 samples at 16kHz)
    for _ in 0..16000 {
        writer.write_sample(0i16)?;
    }
    writer.finalize()?;
    Ok(())
}

// Simple frontmatter extractors for the list command
fn extract_frontmatter_field(content: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix(&prefix) {
            return Some(
                value
                    .trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}

fn extract_title(content: &str) -> Option<String> {
    extract_frontmatter_field(content, "title")
}

fn extract_date(content: &str) -> Option<String> {
    extract_frontmatter_field(content, "date")
}

fn extract_type(content: &str) -> Option<String> {
    extract_frontmatter_field(content, "type")
}
