use minutes_core::config::Config;
use minutes_core::markdown::{split_frontmatter, Frontmatter, IntentKind};
use minutes_core::search::{self, SearchFilters};
use std::fs;
use std::path::{Path, PathBuf};

pub const ACTIVE_MEETING_FILE: &str = "CURRENT_MEETING.md";
pub const ACTIVE_ARTIFACT_FILE: &str = "CURRENT_ARTIFACT.md";

const ARTIFACT_INSTRUCTION: &str = "If CURRENT_ARTIFACT.md exists in this directory, the user has that file open in the Minutes viewer. You can read and edit it at the path shown. Changes you make will appear in real time in the viewer.";
const ASSISTANT_SKILL_BUNDLE_ENV: &str = "MINUTES_ASSISTANT_SKILL_BUNDLE_ROOT";

#[derive(Debug, Clone)]
struct AssistantSkillBundle {
    agents_skills: PathBuf,
    opencode_skills: PathBuf,
    opencode_commands: PathBuf,
}

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

impl AssistantSkillBundle {
    fn from_root(root: PathBuf) -> Option<Self> {
        let agents_skills = root.join("agents-skills");
        let opencode_skills = root.join("opencode-skills");
        let opencode_commands = root.join("opencode-commands");
        if agents_skills.exists() && opencode_skills.exists() && opencode_commands.exists() {
            Some(Self {
                agents_skills,
                opencode_skills,
                opencode_commands,
            })
        } else {
            None
        }
    }
}

fn repo_assistant_skill_bundle() -> Option<AssistantSkillBundle> {
    AssistantSkillBundle::from_root(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join("assistant-skill-bundle"),
    )
    .or_else(|| {
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let agents_skills = repo_root.join(".agents").join("skills");
        let opencode_skills = repo_root.join(".opencode").join("skills");
        let opencode_commands = repo_root.join(".opencode").join("commands");
        if agents_skills.exists() && opencode_skills.exists() && opencode_commands.exists() {
            Some(AssistantSkillBundle {
                agents_skills,
                opencode_skills,
                opencode_commands,
            })
        } else {
            None
        }
    })
}

fn bundled_assistant_skill_bundle() -> Option<AssistantSkillBundle> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let macos_dir = exe_dir.file_name().and_then(|name| name.to_str()) == Some("MacOS");
    let resources_parent = if macos_dir {
        exe_dir.parent()?.join("Resources")
    } else {
        exe_dir.to_path_buf()
    };

    [
        resources_parent.join("assistant-skill-bundle"),
        resources_parent
            .join("resources")
            .join("assistant-skill-bundle"),
    ]
    .into_iter()
    .find_map(AssistantSkillBundle::from_root)
}

fn resolve_assistant_skill_bundle() -> Option<AssistantSkillBundle> {
    if let Some(override_root) = std::env::var_os(ASSISTANT_SKILL_BUNDLE_ENV) {
        if let Some(bundle) = AssistantSkillBundle::from_root(PathBuf::from(override_root)) {
            return Some(bundle);
        }
    }

    bundled_assistant_skill_bundle().or_else(repo_assistant_skill_bundle)
}

fn remove_path(path: &Path) -> Result<(), String> {
    if !path.exists() && fs::symlink_metadata(path).is_err() {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(path)
        .map_err(|e| format!("Failed to inspect {}: {}", path.display(), e))?;
    if metadata.file_type().is_symlink() || metadata.is_file() {
        fs::remove_file(path).map_err(|e| format!("Failed to remove {}: {}", path.display(), e))
    } else {
        fs::remove_dir_all(path).map_err(|e| format!("Failed to remove {}: {}", path.display(), e))
    }
}

fn copy_dir_recursive(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target)
        .map_err(|e| format!("Failed to create {}: {}", target.display(), e))?;
    for entry in
        fs::read_dir(source).map_err(|e| format!("Failed to read {}: {}", source.display(), e))?
    {
        let entry = entry.map_err(|e| format!("Bad dir entry in {}: {}", source.display(), e))?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        let metadata = entry
            .metadata()
            .map_err(|e| format!("Failed to stat {}: {}", from.display(), e))?;
        if metadata.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            if let Some(parent) = to.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
            }
            fs::copy(&from, &to).map_err(|e| {
                format!(
                    "Failed to copy {} to {}: {}",
                    from.display(),
                    to.display(),
                    e
                )
            })?;
        }
    }
    Ok(())
}

fn replace_dir_contents(target: &Path, source: &Path) -> Result<(), String> {
    remove_path(target)?;
    copy_dir_recursive(source, target)
}

fn sync_assistant_skill_mirrors(workspace: &Path) -> Result<(), String> {
    let Some(bundle) = resolve_assistant_skill_bundle() else {
        return Ok(());
    };

    let agents_root = workspace.join(".agents");
    fs::create_dir_all(&agents_root)
        .map_err(|e| format!("Failed to create {}: {}", agents_root.display(), e))?;
    replace_dir_contents(&agents_root.join("skills"), &bundle.agents_skills)?;

    let opencode_root = workspace.join(".opencode");
    fs::create_dir_all(&opencode_root)
        .map_err(|e| format!("Failed to create {}: {}", opencode_root.display(), e))?;
    replace_dir_contents(&opencode_root.join("skills"), &bundle.opencode_skills)?;
    replace_dir_contents(&opencode_root.join("commands"), &bundle.opencode_commands)?;

    Ok(())
}

/// Ensure the singleton assistant workspace exists and return its path.
pub fn create_workspace(config: &Config) -> Result<PathBuf, String> {
    let workspace = workspace_dir();

    fs::create_dir_all(&workspace).map_err(|e| format!("Failed to create workspace: {}", e))?;

    let meetings_link = workspace.join("meetings");
    if !meetings_link.exists() {
        #[cfg(unix)]
        std::os::unix::fs::symlink(&config.output_dir, &meetings_link)
            .map_err(|e| format!("Failed to symlink meetings dir: {}", e))?;
    }

    // Refresh assistant-local skill mirrors from the generated portable
    // trees so Recall sees the same Minutes lifecycle skills as the repo.
    sync_assistant_skill_mirrors(&workspace)?;

    // .claude is still user-owned. We keep the existing symlink layout
    // intact and only refresh the app-managed .agents/.opencode mirrors.

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

    md.push_str("\n## Open Artifact\n\n");
    md.push_str(ARTIFACT_INSTRUCTION);
    md.push('\n');

    md.push_str("\n## Instructions\n\n");
    md.push_str("- Synthesize information across multiple meetings\n");
    md.push_str("- Track decisions, action items, and commitments\n");
    md.push_str("- Help prepare for upcoming meetings\n");
    md.push_str("- Create follow-up documents, reports, and summaries\n");
    md.push_str("- Always cite which meeting your information comes from\n");
    md.push_str("- You can create files in this directory to save artifacts\n");

    Ok(md)
}

fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid file name: {}", path.display()))?;
    let temp_path = path.with_file_name(format!(".{}.tmp", file_name));
    std::fs::write(&temp_path, content)
        .map_err(|e| format!("Failed to write temp file {}: {}", temp_path.display(), e))?;
    std::fs::rename(&temp_path, path).map_err(|e| {
        format!(
            "Failed to atomically replace {} with {}: {}",
            path.display(),
            temp_path.display(),
            e
        )
    })
}

fn ensure_agents_artifact_instruction(workspace: &Path) -> Result<(), String> {
    let agents_path = workspace.join("AGENTS.md");
    if !agents_path.exists() {
        return Ok(());
    }

    let existing = std::fs::read_to_string(&agents_path)
        .map_err(|e| format!("Failed to read {}: {}", agents_path.display(), e))?;
    if existing.contains(ACTIVE_ARTIFACT_FILE) {
        return Ok(());
    }

    let mut updated = existing.trim_end().to_string();
    if !updated.is_empty() {
        updated.push_str("\n\n");
    }
    updated.push_str("## Open Artifact\n\n");
    updated.push_str(ARTIFACT_INSTRUCTION);
    updated.push('\n');

    write_atomic(&agents_path, &updated)
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

    write_atomic(&claude_md_path, &content)?;
    ensure_agents_artifact_instruction(workspace)
}

pub fn write_active_meeting_context(
    workspace: &Path,
    meeting_path: &Path,
    config: &Config,
) -> Result<(), String> {
    let meeting_md = generate_meeting_context(meeting_path, config)?;
    write_atomic(&workspace.join(ACTIVE_MEETING_FILE), &meeting_md)
}

pub fn write_active_artifact_context(workspace: &Path, artifact_path: &Path) -> Result<(), String> {
    let content = std::fs::read_to_string(artifact_path)
        .map_err(|e| format!("Cannot read artifact {}: {}", artifact_path.display(), e))?;
    let preview = content.lines().take(200).collect::<Vec<_>>().join("\n");
    let file_name = artifact_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Artifact");

    let mut md = String::with_capacity(preview.len() + 512);
    md.push_str("# Open Artifact\n\n");
    md.push_str("The user currently has this artifact open in the Minutes viewer.\n\n");
    md.push_str(&format!("- **Path**: `{}`\n", artifact_path.display()));
    md.push_str(&format!("- **Filename**: {}\n", file_name));
    md.push_str("- **Editable**: yes\n\n");
    md.push_str("## Preview\n\n");
    if preview.trim().is_empty() {
        md.push_str("_File is empty._\n");
    } else {
        md.push_str("```md\n");
        md.push_str(&preview);
        if content.lines().count() > 200 {
            md.push_str("\n... [preview truncated]");
        }
        md.push_str("\n```\n");
    }

    write_atomic(&workspace.join(ACTIVE_ARTIFACT_FILE), &md)
}

pub fn clear_active_meeting_context(workspace: &Path) -> Result<(), String> {
    let active_path = workspace.join(ACTIVE_MEETING_FILE);
    if active_path.exists() {
        std::fs::remove_file(&active_path)
            .map_err(|e| format!("Failed to clear meeting context: {}", e))?;
    }
    Ok(())
}

pub fn clear_active_artifact_context(workspace: &Path) -> Result<(), String> {
    let active_path = workspace.join(ACTIVE_ARTIFACT_FILE);
    if active_path.exists() {
        std::fs::remove_file(&active_path)
            .map_err(|e| format!("Failed to clear artifact context: {}", e))?;
    }
    Ok(())
}

/// Clean transient context left behind by previous app versions or crashes.
pub fn cleanup_stale_workspaces() {
    let workspace = workspace_dir();
    clear_active_meeting_context(&workspace).ok();
    clear_active_artifact_context(&workspace).ok();

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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn assistant_context_mentions_open_artifact_contract() {
        let config = Config::default();
        let content = generate_assistant_context(&config).expect("assistant context");
        assert!(content.contains(ACTIVE_ARTIFACT_FILE));
        assert!(content.contains("Changes you make will appear in real time"));
    }

    #[test]
    fn write_active_artifact_context_truncates_preview_after_200_lines() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("assistant");
        fs::create_dir_all(&workspace).unwrap();
        let artifact_path = temp.path().join("notes.md");
        let body = (1..=205)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&artifact_path, body).unwrap();

        write_active_artifact_context(&workspace, &artifact_path).expect("artifact context");
        let written = fs::read_to_string(workspace.join(ACTIVE_ARTIFACT_FILE)).unwrap();

        assert!(written.contains("`"));
        assert!(written.contains("line 1"));
        assert!(written.contains("line 200"));
        assert!(!written.contains("line 205"));
        assert!(written.contains("[preview truncated]"));
    }

    #[test]
    fn sync_assistant_skill_mirrors_replaces_stale_flat_skills_with_generated_trees() {
        let temp = tempfile::tempdir().unwrap();
        let bundle_root = temp.path().join("bundle");
        let agents_source = bundle_root
            .join("agents-skills")
            .join("minutes")
            .join("minutes-prep");
        let opencode_skill = bundle_root.join("opencode-skills").join("minutes-prep");
        let opencode_command = bundle_root.join("opencode-commands");
        fs::create_dir_all(&agents_source).unwrap();
        fs::create_dir_all(&opencode_skill).unwrap();
        fs::create_dir_all(&opencode_command).unwrap();
        fs::write(agents_source.join("SKILL.md"), "codex prep").unwrap();
        fs::write(opencode_skill.join("SKILL.md"), "opencode prep").unwrap();
        fs::write(opencode_command.join("minutes-prep.md"), "command stub").unwrap();

        let workspace = temp.path().join("assistant");
        fs::create_dir_all(workspace.join(".agents/skills/minutes-search")).unwrap();
        fs::write(
            workspace.join(".agents/skills/minutes-search/SKILL.md"),
            "stale search",
        )
        .unwrap();

        std::env::set_var(ASSISTANT_SKILL_BUNDLE_ENV, &bundle_root);
        sync_assistant_skill_mirrors(&workspace).unwrap();
        std::env::remove_var(ASSISTANT_SKILL_BUNDLE_ENV);

        assert!(workspace
            .join(".agents/skills/minutes/minutes-prep/SKILL.md")
            .exists());
        assert!(!workspace
            .join(".agents/skills/minutes-search/SKILL.md")
            .exists());
        assert!(workspace
            .join(".opencode/skills/minutes-prep/SKILL.md")
            .exists());
        assert!(workspace
            .join(".opencode/commands/minutes-prep.md")
            .exists());
    }
}
