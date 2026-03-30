use minutes_core::config::Config;
use minutes_core::markdown::{split_frontmatter, Frontmatter, IntentKind};
use minutes_core::search::{self, SearchFilters};
use std::path::{Path, PathBuf};

pub const ACTIVE_MEETING_FILE: &str = "CURRENT_MEETING.md";

fn intent_label(kind: IntentKind) -> &'static str {
    match kind {
        IntentKind::ActionItem => "action-item",
        IntentKind::Commitment => "commitment",
        IntentKind::Decision => "decision",
        IntentKind::OpenQuestion => "open-question",
    }
}

/// Stable assistant workspace used by the singleton assistant session.
pub fn workspace_dir() -> PathBuf {
    Config::minutes_dir().join("assistant")
}

/// Ensure the singleton assistant workspace exists and return its path.
pub fn create_workspace(config: &Config) -> Result<PathBuf, String> {
    let workspace = workspace_dir();

    std::fs::create_dir_all(&workspace)
        .map_err(|e| format!("Failed to create workspace: {}", e))?;

    let meetings_link = workspace.join("meetings");
    if !meetings_link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&config.output_dir, &meetings_link)
            .map_err(|e| format!("Failed to symlink meetings dir: {}", e))?;
    }

    // Skills and agents live in ~/.minutes/.agents/ and are symlinked
    // into the workspace's .claude/ directory. This is set up once
    // (manually or by minutes setup) — we don't touch .claude/ here
    // to avoid overwriting user configuration.

    // git init on the assistant workspace (idempotent) so Claude Code discovers CLAUDE.md.
    if !workspace.join(".git").exists() {
        let git_status = std::process::Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&workspace)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("Failed to run git init: {}", e))?;
        if !git_status.success() {
            return Err("git init failed in assistant workspace. Is git installed?".into());
        }
    }

    Ok(workspace)
}

/// Generate CLAUDE.md for discussing a specific meeting.
pub fn generate_meeting_context(meeting_path: &Path, config: &Config) -> Result<String, String> {
    let content =
        std::fs::read_to_string(meeting_path).map_err(|e| format!("Cannot read meeting: {}", e))?;

    let (fm_str, body) = split_frontmatter(&content);
    let fm: Frontmatter =
        serde_yaml::from_str(fm_str).map_err(|e| format!("Bad frontmatter: {}", e))?;

    let content_type = match fm.r#type {
        minutes_core::markdown::ContentType::Meeting => "meeting",
        minutes_core::markdown::ContentType::Memo => "memo",
        minutes_core::markdown::ContentType::Dictation => "dictation",
    };

    let mut md = String::with_capacity(4096);
    md.push_str("# Meeting Context\n\n");
    md.push_str("You are helping the user analyze a specific meeting recording.\n\n");

    md.push_str(&format!("## {}\n", fm.title));
    md.push_str(&format!(
        "- **Date**: {}\n",
        fm.date.format("%B %d, %Y %H:%M")
    ));
    md.push_str(&format!("- **Duration**: {}\n", fm.duration));
    md.push_str(&format!("- **Type**: {}\n", content_type));
    if !fm.attendees.is_empty() {
        md.push_str(&format!("- **Attendees**: {}\n", fm.attendees.join(", ")));
    }
    if let Some(ref ctx) = fm.context {
        md.push_str(&format!("- **Context**: {}\n", ctx));
    }
    if let Some(ref cal) = fm.calendar_event {
        md.push_str(&format!("- **Calendar**: {}\n", cal));
    }
    md.push('\n');

    // Decisions
    if !fm.decisions.is_empty() {
        md.push_str("## Decisions Made\n");
        for d in &fm.decisions {
            md.push_str(&format!("- {}", d.text));
            if let Some(ref topic) = d.topic {
                md.push_str(&format!(" (topic: {})", topic));
            }
            md.push('\n');
        }
        md.push('\n');
    }

    // Open intents (action items + commitments)
    let open_intents: Vec<_> = fm.intents.iter().filter(|i| i.status == "open").collect();
    if !open_intents.is_empty() {
        md.push_str("## Open Action Items\n");
        for i in open_intents {
            md.push_str(&format!("- **{}**: {}", intent_label(i.kind), i.what));
            if let Some(ref who) = i.who {
                md.push_str(&format!(" (@{})", who));
            }
            if let Some(ref by) = i.by_date {
                md.push_str(&format!(" — due {}", by));
            }
            md.push('\n');
        }
        md.push('\n');
    }

    // Include the full body (summary + transcript)
    md.push_str("## Full Content\n\n");
    md.push_str("The complete meeting transcript follows. It is also available at:\n");
    md.push_str(&format!("`{}`\n\n", meeting_path.display()));
    // Truncate very long transcripts to avoid blowing out context
    let body_chars: Vec<char> = body.chars().collect();
    if body_chars.len() > 12000 {
        let truncated: String = body_chars[..12000].iter().collect();
        md.push_str(&truncated);
        md.push_str("\n\n...[transcript truncated — read the full file for the rest]\n");
    } else {
        md.push_str(body);
    }

    md.push_str("\n\n## Instructions\n\n");
    md.push_str("- Answer questions about this meeting\n");
    md.push_str("- Help draft follow-up messages based on the discussion\n");
    md.push_str("- Extract key takeaways the user might have missed\n");
    md.push_str(&format!(
        "- All meetings are at `{}` if you need to cross-reference\n",
        config.output_dir.display()
    ));
    md.push_str("- You can create files in this directory to save artifacts\n");

    Ok(md)
}

/// Generate CLAUDE.md for general meeting assistant mode.
pub fn generate_assistant_context(config: &Config) -> Result<String, String> {
    let mut md = String::with_capacity(4096);
    md.push_str("# Minutes Assistant\n\n");
    md.push_str("You are a meeting intelligence assistant with access to the user's complete meeting history.\n\n");
    md.push_str(&format!(
        "## Meeting Directory\n`{}`\n\n",
        config.output_dir.display()
    ));

    // Recent meetings — just walk dir and sort by date, no full-text search
    let filters = SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };

    if let Ok(results) = search::search("", config, &filters) {
        let recent: Vec<_> = results.into_iter().take(10).collect();
        if !recent.is_empty() {
            md.push_str("## Recent Meetings\n");
            for r in &recent {
                md.push_str(&format!(
                    "- **{}** ({}) [{}] — `{}`\n",
                    r.title,
                    r.date,
                    r.content_type,
                    r.path.display()
                ));
            }
            md.push('\n');
        }
    }

    // Open intents via search_intents (not the legacy find_open_actions)
    let intent_filters = SearchFilters {
        content_type: None,
        since: None,
        attendee: None,
        intent_kind: None,
        owner: None,
        recorded_by: None,
    };
    if let Ok(intents) = search::search_intents("", config, &intent_filters) {
        let open: Vec<_> = intents
            .into_iter()
            .filter(|i| i.status == "open")
            .take(15)
            .collect();
        if !open.is_empty() {
            md.push_str("## Open Action Items & Commitments\n");
            for i in &open {
                md.push_str(&format!(
                    "- [{}] **{}**: {}",
                    i.title,
                    intent_label(i.kind),
                    i.what
                ));
                if let Some(ref who) = i.who {
                    md.push_str(&format!(" (@{})", who));
                }
                if let Some(ref by) = i.by_date {
                    md.push_str(&format!(" — due {}", by));
                }
                md.push('\n');
            }
            md.push('\n');
        }
    }

    md.push_str("## Minutes CLI Commands\n");
    md.push_str("The `minutes` CLI is available on PATH:\n");
    md.push_str("- `minutes search \"topic\"` — full-text search across all meetings\n");
    md.push_str("- `minutes actions` — show all open action items\n");
    md.push_str("- `minutes actions --assignee \"name\"` — filter by person\n");
    md.push_str("- `minutes consistency` — flag conflicting decisions and stale commitments\n");
    md.push_str("- `minutes person \"name\"` — build a profile across meetings\n");
    md.push_str("- `minutes list` — list recent meetings and memos\n");
    md.push_str("- `minutes record` / `minutes stop` — start/stop recording\n");
    md.push_str("- `minutes live` / `minutes stop` — start/stop live transcript (real-time)\n");
    md.push_str("- `minutes transcript --since 5m` — read last 5 minutes of live transcript\n");
    md.push_str("- `minutes transcript --status` — check if a live session is active\n");
    md.push_str("- `minutes note \"text\"` — add a timestamped note to current recording\n");
    md.push_str("- `minutes process <file>` — process an audio file\n");
    md.push_str("- `minutes qmd status` — check QMD collection status\n");
    md.push_str("- `minutes qmd register` — register meetings as a QMD collection\n");
    md.push_str(&format!(
        "- `grep -ril \"keyword\" {}/` — raw file search\n",
        config.output_dir.display()
    ));

    md.push_str("\n## Integrations\n\n");
    md.push_str("**QMD** — Minutes can register its output directory as a QMD collection for semantic search.\n");
    md.push_str("Run `minutes qmd status` to check, `minutes qmd register` to set up.\n");
    md.push_str(
        "Once registered, `qmd search \"topic\" -c minutes` searches meetings semantically.\n\n",
    );
    md.push_str(
        "**PARA / Obsidian** — If the user has a PARA knowledge graph at ~/Documents/life/,\n",
    );
    md.push_str(
        "older meetings may live in areas/meetings/. Minutes outputs to ~/meetings/ by default.\n",
    );
    md.push_str("These can be unified by symlinking or configuring output_dir in config.toml.\n\n");
    md.push_str("**Daily Notes** — Minutes can append session summaries to daily notes.\n");
    md.push_str("Configure in ~/.config/minutes/config.toml under [daily_notes].\n");

    md.push_str("\n## Active Meeting Focus\n\n");
    md.push_str(&format!(
        "If `{}` exists in this directory, treat it as the current meeting focus and read it before answering.\n",
        ACTIVE_MEETING_FILE
    ));
    md.push_str(
        "If that file does not exist, operate in general assistant mode across all meetings.\n",
    );

    md.push_str("\n## Instructions\n\n");
    md.push_str("- Synthesize information across multiple meetings\n");
    md.push_str("- Track decisions, action items, and commitments\n");
    md.push_str("- Help prepare for upcoming meetings\n");
    md.push_str("- Create follow-up documents, reports, and summaries\n");
    md.push_str("- Always cite which meeting your information comes from\n");
    md.push_str("- You can create files in this directory to save artifacts\n");

    Ok(md)
}

pub fn write_assistant_context(workspace: &Path, config: &Config) -> Result<(), String> {
    let claude_md_path = workspace.join("CLAUDE.md");
    let assistant_md = generate_assistant_context(config)?;

    // Preserve live transcript markers only if a session is actually active (V3).
    // Don't trust stale markers in the file — verify via PID.
    let lt_pid = minutes_core::pid::live_transcript_pid_path();
    let live_actually_active = minutes_core::pid::check_pid_file(&lt_pid)
        .ok()
        .flatten()
        .is_some();

    let content = if live_actually_active {
        let marker_start = "<!-- LIVE_TRANSCRIPT_START -->";
        let marker_end = "<!-- LIVE_TRANSCRIPT_END -->";
        let existing = std::fs::read_to_string(&claude_md_path).unwrap_or_default();
        let live_section = if let (Some(start), Some(end)) =
            (existing.find(marker_start), existing.find(marker_end))
        {
            if start < end {
                Some(existing[start..end + marker_end.len()].to_string())
            } else {
                None
            }
        } else {
            None
        };

        if let Some(section) = live_section {
            format!("{}\n{}\n", assistant_md.trim_end(), section)
        } else {
            assistant_md.clone()
        }
    } else {
        assistant_md
    };

    std::fs::write(&claude_md_path, content)
        .map_err(|e| format!("Failed to write assistant context: {}", e))
}

pub fn write_active_meeting_context(
    workspace: &Path,
    meeting_path: &Path,
    config: &Config,
) -> Result<(), String> {
    let meeting_md = generate_meeting_context(meeting_path, config)?;
    std::fs::write(workspace.join(ACTIVE_MEETING_FILE), meeting_md)
        .map_err(|e| format!("Failed to write meeting context: {}", e))
}

pub fn clear_active_meeting_context(workspace: &Path) -> Result<(), String> {
    let active_path = workspace.join(ACTIVE_MEETING_FILE);
    if active_path.exists() {
        std::fs::remove_file(&active_path)
            .map_err(|e| format!("Failed to clear meeting context: {}", e))?;
    }
    Ok(())
}

/// Clean transient context left behind by previous app versions or crashes.
pub fn cleanup_stale_workspaces() {
    let workspace = workspace_dir();
    clear_active_meeting_context(&workspace).ok();

    if let Ok(entries) = std::fs::read_dir(&workspace) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                continue;
            };
            if path.is_dir() && name.starts_with("discuss-") {
                std::fs::remove_dir_all(path).ok();
            }
        }
    }
}
