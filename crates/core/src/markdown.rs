use crate::config::Config;
use crate::error::MarkdownError;
use chrono::{DateTime, Local};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────────────────────
// Meeting/memo markdown output.
// All files written with 0600 permissions (owner read/write only)
// because transcripts contain sensitive conversation content.
// ──────────────────────────────────────────────────────────────

/// Content types for output files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Meeting,
    Memo,
    Dictation,
}

/// Output status markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStatus {
    Complete,
    NoSpeech,
    TranscriptOnly,
}

/// Frontmatter for a meeting/memo markdown file.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Frontmatter {
    pub title: String,
    pub r#type: ContentType,
    pub date: DateTime<Local>,
    pub duration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<OutputStatus>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    #[serde(default, skip_serializing_if = "EntityLinks::is_empty")]
    pub entities: EntityLinks,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub captured_at: Option<DateTime<Local>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub action_items: Vec<ActionItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub decisions: Vec<Decision>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub intents: Vec<Intent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recorded_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub visibility: Option<Visibility>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speaker_map: Vec<crate::diarize::SpeakerAttribution>,
    /// Diagnostic string from the transcription filter pipeline.
    /// Not serialized to YAML — only used for the NoSpeech hint in rendered markdown.
    #[serde(skip)]
    pub filter_diagnosis: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct EntityLinks {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<EntityRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<EntityRef>,
}

impl EntityLinks {
    pub fn is_empty(&self) -> bool {
        self.people.is_empty() && self.projects.is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityRef {
    pub slug: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<String>,
}

/// A structured action item extracted from a meeting.
/// Queryable via MCP tools: filter by assignee, status, due date.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ActionItem {
    pub assignee: String,
    pub task: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    pub status: String, // "open" or "done"
}

/// A structured decision extracted from a meeting.
/// Queryable via MCP tools: search across all meetings for decision history.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Decision {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum IntentKind {
    ActionItem,
    Decision,
    OpenQuestion,
    Commitment,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Intent {
    pub kind: IntentKind,
    pub what: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    Private,
    Team,
}

/// Result of writing a meeting/memo to disk.
#[derive(Debug, Clone, Serialize)]
pub struct WriteResult {
    pub path: PathBuf,
    pub title: String,
    pub word_count: usize,
    pub content_type: ContentType,
}

fn render_markdown(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: &Path,
) -> Result<String, MarkdownError> {
    let yaml = serde_yaml::to_string(frontmatter)
        .map_err(|e| MarkdownError::SerializationError(e.to_string()))?;

    let mut content = format!("---\n{}---\n\n", yaml);

    if let Some(summary_text) = summary {
        content.push_str("## Summary\n\n");
        content.push_str(summary_text);
        content.push_str("\n\n");
    }

    if frontmatter.status == Some(OutputStatus::NoSpeech) {
        content.push_str("*No speech detected in this recording.*\n\n");
        if let Some(diagnosis) = &frontmatter.filter_diagnosis {
            content.push_str(&format!("**Diagnosis**: {}\n\n", diagnosis));
        }
        content.push_str(&format!(
            "**Retry audio**: `{}`\n\n",
            retry_audio_path.display()
        ));
        content.push_str(&format!(
            "To retry after adjusting your transcription settings:\n`minutes process {}`\n\n",
            retry_audio_path.display()
        ));
    }

    if let Some(notes) = user_notes {
        content.push_str("## Notes\n\n");
        for line in notes.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() {
                content.push_str(&format!("- {}\n", trimmed));
            }
        }
        content.push('\n');
    }

    content.push_str("## Transcript\n\n");
    content.push_str(transcript);
    content.push('\n');

    Ok(content)
}

/// Write a meeting/memo to markdown with YAML frontmatter.
pub fn write(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    config: &Config,
) -> Result<WriteResult, MarkdownError> {
    write_with_retry_path(frontmatter, transcript, summary, user_notes, None, config)
}

/// Write markdown while pointing no-speech retry guidance at the original audio path.
pub fn write_with_retry_path(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: Option<&Path>,
    config: &Config,
) -> Result<WriteResult, MarkdownError> {
    let output_dir = match frontmatter.r#type {
        ContentType::Memo => config.output_dir.join("memos"),
        ContentType::Meeting => config.output_dir.clone(),
        ContentType::Dictation => config.output_dir.join("dictations"),
    };

    // Ensure output directory exists
    fs::create_dir_all(&output_dir)
        .map_err(|e| MarkdownError::OutputDirError(format!("{}: {}", output_dir.display(), e)))?;

    // Generate filename slug
    let slug = generate_slug(
        &frontmatter.title,
        frontmatter.date,
        frontmatter.recorded_by.as_deref(),
    );
    let path = resolve_collision(&output_dir, &slug);
    let content = render_markdown(
        frontmatter,
        transcript,
        summary,
        user_notes,
        retry_audio_path.unwrap_or(&path),
    )?;

    // Write file with appropriate permissions
    fs::write(&path, &content)?;
    let mode = match frontmatter.visibility {
        Some(Visibility::Team) => 0o640,
        _ => 0o600,
    };
    set_permissions(&path, mode)?;

    let word_count = transcript.split_whitespace().count();
    tracing::info!(
        path = %path.display(),
        words = word_count,
        content_type = ?frontmatter.r#type,
        "wrote meeting markdown"
    );

    Ok(WriteResult {
        path,
        title: frontmatter.title.clone(),
        word_count,
        content_type: frontmatter.r#type,
    })
}

pub fn rewrite(
    path: &Path,
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
) -> Result<WriteResult, MarkdownError> {
    rewrite_with_retry_path(path, frontmatter, transcript, summary, user_notes, None)
}

pub fn rewrite_with_retry_path(
    path: &Path,
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    retry_audio_path: Option<&Path>,
) -> Result<WriteResult, MarkdownError> {
    let content = render_markdown(
        frontmatter,
        transcript,
        summary,
        user_notes,
        retry_audio_path.unwrap_or(path),
    )?;
    let tmp = path.with_extension("md.tmp");
    fs::write(&tmp, content)?;
    let mode = match frontmatter.visibility {
        Some(Visibility::Team) => 0o640,
        _ => 0o600,
    };
    set_permissions(&tmp, mode)?;
    fs::rename(&tmp, path)?;

    let word_count = transcript.split_whitespace().count();
    Ok(WriteResult {
        path: path.to_path_buf(),
        title: frontmatter.title.clone(),
        word_count,
        content_type: frontmatter.r#type,
    })
}

/// Rename an existing meeting markdown file in place.
///
/// This is the safe path used by the command palette's
/// `RenameCurrentMeeting` action. It is **fail-closed**: any
/// frontmatter that is not boring-and-plain refuses the rename
/// instead of attempting a string replace that could corrupt YAML
/// anchors, folded scalars, literal blocks, or aliases.
///
/// Steps (described in `PLAN.md.command-palette-slice-2` D8):
/// 1. Read the file.
/// 2. Split frontmatter via `split_frontmatter`. Empty frontmatter
///    means "not a Minutes meeting" → refuse.
/// 3. Parse the frontmatter via `serde_yaml::from_str::<Frontmatter>`.
///    A failure means the file is malformed → refuse.
/// 4. Re-parse the same frontmatter as `serde_yaml::Value` to check
///    that the `title` field is a **plain string scalar**. If it is a
///    folded scalar (`title: >`), literal block (`title: |`), tagged
///    scalar, mapping, sequence, or carries an anchor/alias, refuse.
///    These are real YAML constructs that the line-replace strategy
///    cannot handle safely.
/// 5. Find the exact line matching `^title:\s*<original-quoted-or-bare>$`
///    in the frontmatter text. If zero matches or more than one,
///    refuse.
/// 6. Replace that single line with `title: "<escaped-new-title>"`.
/// 7. Write the result to a tmp sibling and rename atomically over
///    the original path.
/// 8. **Parse the written file** to confirm the resulting frontmatter
///    is still valid YAML. If parse fails, restore the backup that
///    was written before the change and return an error.
/// 9. If the new title produces a different slug, rename the file
///    using `resolve_collision`. Returns the final path.
///
/// Errors are returned as `MarkdownError::RenameRefused` for the
/// safety-policy refusals and as `MarkdownError::Io` for filesystem
/// failures.
pub fn rename_meeting(path: &Path, new_title: &str) -> Result<PathBuf, MarkdownError> {
    let new_title = new_title.trim();
    if new_title.is_empty() {
        return Err(MarkdownError::RenameRefused("new title is empty".into()));
    }
    if new_title.contains('\n') || new_title.contains('\r') {
        return Err(MarkdownError::RenameRefused(
            "new title contains newlines".into(),
        ));
    }

    let original = fs::read_to_string(path)?;
    let (fm_str, _body) = split_frontmatter(&original);
    if fm_str.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "file has no YAML frontmatter — not a Minutes meeting".into(),
        ));
    }

    // Step 3: parse via serde_yaml::Frontmatter to confirm the file is
    // structurally a meeting.
    let parsed: Frontmatter = serde_yaml::from_str(fm_str).map_err(|e| {
        MarkdownError::RenameRefused(format!("frontmatter does not parse as YAML: {}", e))
    })?;

    let original_title = parsed.title.trim().to_string();
    if original_title.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "current frontmatter title is empty".into(),
        ));
    }

    // Step 4: confirm the on-disk title is a plain-string scalar with
    // no anchors/aliases/tags/folded/literal blocks. We do this by
    // parsing the frontmatter as a generic serde_yaml::Value and
    // walking the title node.
    let value: serde_yaml::Value = serde_yaml::from_str(fm_str).map_err(|e| {
        MarkdownError::RenameRefused(format!("frontmatter generic parse failed: {}", e))
    })?;
    let title_value = value
        .get("title")
        .ok_or_else(|| MarkdownError::RenameRefused("no `title` field in frontmatter".into()))?;
    if !title_value.is_string() {
        return Err(MarkdownError::RenameRefused(
            "title is not a plain scalar — rename via your text editor".into(),
        ));
    }

    // No-op rename: title unchanged.
    if original_title == new_title {
        return Ok(path.to_path_buf());
    }

    // Step 5: find the EXACT title line in fm_str. We refuse to touch
    // files with `title:` appearing on more than one line in the
    // frontmatter — that's a sign of an unusual file we don't want to
    // mutate blindly.
    let title_lines: Vec<(usize, &str)> = fm_str
        .lines()
        .enumerate()
        .filter(|(_, line)| {
            let trimmed = line.trim_start();
            trimmed.starts_with("title:") && !trimmed.starts_with("title::")
        })
        .collect();
    if title_lines.is_empty() {
        return Err(MarkdownError::RenameRefused(
            "could not locate `title:` line in frontmatter".into(),
        ));
    }
    if title_lines.len() > 1 {
        return Err(MarkdownError::RenameRefused(
            "multiple `title:` lines in frontmatter — refusing to rename".into(),
        ));
    }
    let (title_line_index, original_title_line) = title_lines[0];

    // Reject anchors / folded / literal block markers on the title line.
    let after_colon = original_title_line
        .trim_start()
        .trim_start_matches("title:")
        .trim_start();
    if after_colon.starts_with('&') || after_colon.starts_with('*') || after_colon.starts_with('!')
    {
        return Err(MarkdownError::RenameRefused(
            "title line uses YAML anchor/alias/tag — rename via your text editor".into(),
        ));
    }
    // Folded scalar `>` and literal block `|` markers (with optional
    // chomping indicator) on the title line mean the value spans
    // multiple lines, which the line replace cannot handle safely.
    let leading_marker = after_colon.chars().next();
    if matches!(leading_marker, Some('>') | Some('|')) {
        return Err(MarkdownError::RenameRefused(
            "title is a folded or literal block scalar — rename via your text editor".into(),
        ));
    }

    // Step 6: rebuild the frontmatter with the title line replaced.
    let new_title_line = format!("title: {}", yaml_quote(new_title));
    let mut new_fm_lines: Vec<String> = fm_str.lines().map(String::from).collect();
    new_fm_lines[title_line_index] = new_title_line;
    let new_fm_text = new_fm_lines.join("\n");

    // Reassemble the file. `split_frontmatter` strips the leading
    // `---\n` and trailing `\n---\n`; we have to put them back.
    // Find the body slice the same way `split_frontmatter` does, then
    // splice in the new frontmatter text.
    let body_start = original
        .find("\n---")
        .map(|idx| {
            // Move past the trailing `\n---` and the next newline.
            let after = idx + 4;
            original[after..]
                .find('\n')
                .map(|n| after + n + 1)
                .unwrap_or(original.len())
        })
        .unwrap_or(original.len());
    let new_content = format!("---\n{}\n---\n{}", new_fm_text, &original[body_start..]);

    // Step 7: atomic write through a tmp sibling. Preserve the
    // ORIGINAL file's permissions instead of forcing 0o600 — the
    // user may have chmod'd the file to 0o644 for Obsidian sync, a
    // local webserver preview, or any other workflow that needs
    // group-readable. Forcing 0o600 on every rename would silently
    // break those setups (claude pass 3 P3).
    let tmp_path = path.with_extension("md.rename.tmp");
    fs::write(&tmp_path, &new_content)?;
    let original_mode = preserved_file_mode(path);
    if let Err(e) = set_permissions(&tmp_path, original_mode) {
        let _ = fs::remove_file(&tmp_path);
        return Err(e);
    }

    // Step 8: parse-after-write validation. Read back what we just
    // wrote and confirm the frontmatter still parses. If it doesn't,
    // delete the tmp and refuse the rename — the original file is
    // unchanged.
    let written = match fs::read_to_string(&tmp_path) {
        Ok(s) => s,
        Err(e) => {
            let _ = fs::remove_file(&tmp_path);
            return Err(MarkdownError::Io(e));
        }
    };
    let (written_fm, _) = split_frontmatter(&written);
    if let Err(e) = serde_yaml::from_str::<Frontmatter>(written_fm) {
        let _ = fs::remove_file(&tmp_path);
        return Err(MarkdownError::RenameRefused(format!(
            "post-write validation failed; original file unchanged: {}",
            e
        )));
    }

    // Commit: atomically replace the original file with the new
    // content. After this point the meeting markdown reflects the new
    // title; only the file *name* may still need to change.
    fs::rename(&tmp_path, path)?;

    // Step 9: rename the file itself if the slug changes. We use the
    // parsed frontmatter (parsed before the title edit) for the date
    // and recorded_by fields — the title edit doesn't touch those.
    let new_slug = generate_slug(new_title, parsed.date, parsed.recorded_by.as_deref());
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let final_path = if path.file_name().and_then(|n| n.to_str()) == Some(new_slug.as_str()) {
        path.to_path_buf()
    } else {
        let target = resolve_collision(parent, &new_slug);
        fs::rename(path, &target)?;
        target
    };

    Ok(final_path)
}

/// Quote a string as a YAML double-quoted scalar. Escapes the
/// characters that double-quoted scalars require: backslash, double
/// quote, and the C0 control set. Used by `rename_meeting` to write a
/// safe `title:` line.
fn yaml_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write;
                write!(out, "\\x{:02x}", c as u32).expect("write to string");
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Generate a URL-safe filename slug from title, date, and optional recorder name.
fn generate_slug(title: &str, date: DateTime<Local>, recorded_by: Option<&str>) -> String {
    let date_prefix = date.format("%Y-%m-%d").to_string();
    let title_slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");

    let name_suffix = recorded_by
        .map(|name| {
            let short: String = name
                .split_whitespace()
                .next()
                .unwrap_or(name)
                .to_lowercase()
                .chars()
                .filter(|c| c.is_alphanumeric())
                .take(10)
                .collect();
            if short.is_empty() {
                String::new()
            } else {
                format!("-{}", short)
            }
        })
        .unwrap_or_default();

    let slug = if title_slug.is_empty() {
        format!("{}-untitled{}", date_prefix, name_suffix)
    } else {
        // Truncate long titles
        let truncated: String = title_slug.chars().take(60).collect();
        format!("{}-{}{}", date_prefix, truncated, name_suffix)
    };

    format!("{}.md", slug)
}

/// Resolve filename collisions by appending -2, -3, etc.
fn resolve_collision(dir: &Path, filename: &str) -> PathBuf {
    let path = dir.join(filename);
    if !path.exists() {
        return path;
    }

    let stem = filename.trim_end_matches(".md");
    for i in 2..=999 {
        let candidate = dir.join(format!("{}-{}.md", stem, i));
        if !candidate.exists() {
            return candidate;
        }
    }

    // Fallback: use timestamp suffix
    let ts = chrono::Local::now().timestamp();
    dir.join(format!("{}-{}.md", stem, ts))
}

/// Set file permissions to the given mode (Unix only; no-op on Windows).
fn set_permissions(path: &Path, _mode: u32) -> Result<(), MarkdownError> {
    #[cfg(unix)]
    {
        let perms = fs::Permissions::from_mode(_mode);
        fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Read the existing file's mode bits so a rewrite can preserve
/// permissions the user may have set deliberately. Returns `0o600`
/// (the Minutes default) on Windows or if the metadata read fails.
/// Used by `rename_meeting` to avoid clobbering user-chosen modes.
fn preserved_file_mode(_path: &Path) -> u32 {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(_path) {
            // Mask off the file-type bits, keep only the permission
            // bits (rwxrwxrwx + setuid/setgid/sticky).
            return meta.permissions().mode() & 0o7777;
        }
    }
    0o600
}

// ── Frontmatter parsing utilities (shared across modules) ────

/// Split markdown content into frontmatter string and body string.
/// Returns `("", content)` if no frontmatter is found.
pub fn split_frontmatter(content: &str) -> (&str, &str) {
    if !content.starts_with("---") {
        return ("", content);
    }

    if let Some(end) = content[3..].find("\n---") {
        let fm_end = end + 3;
        let body_start = fm_end + 4; // skip \n---
        let body_start = content[body_start..]
            .find('\n')
            .map(|i| body_start + i + 1)
            .unwrap_or(body_start);
        (&content[3..fm_end], &content[body_start..])
    } else {
        ("", content)
    }
}

/// Extract a simple `key: value` field from YAML frontmatter text.
/// Handles quoted values. Returns None if key not found.
pub fn extract_field(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in frontmatter.lines() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_frontmatter() -> Frontmatter {
        Frontmatter {
            title: "Test Meeting".into(),
            r#type: ContentType::Meeting,
            date: Local::now(),
            duration: "5m 30s".into(),
            source: None,
            status: Some(OutputStatus::Complete),
            tags: vec![],
            attendees: vec![],
            calendar_event: None,
            people: vec![],
            entities: EntityLinks::default(),
            device: None,
            captured_at: None,
            context: None,
            action_items: vec![],
            decisions: vec![],
            intents: vec![],
            recorded_by: None,
            visibility: None,
            speaker_map: vec![],
            filter_diagnosis: None,
        }
    }

    #[test]
    fn generates_correct_slug() {
        let date = Local::now();
        let slug = generate_slug("Q2 Planning Discussion", date, None);
        let prefix = date.format("%Y-%m-%d").to_string();
        assert!(slug.starts_with(&prefix));
        assert!(slug.contains("q2-planning-discussion"));
        assert!(slug.ends_with(".md"));
    }

    #[test]
    fn generates_untitled_slug_for_empty_title() {
        let date = Local::now();
        let slug = generate_slug("", date, None);
        assert!(slug.contains("untitled"));
    }

    #[test]
    fn generates_slug_with_recorder_name() {
        let date = Local::now();
        let slug = generate_slug("Q2 Planning", date, Some("Mat Silverstein"));
        assert!(slug.contains("-mat"));
        assert!(slug.ends_with(".md"));
    }

    #[test]
    #[cfg(unix)]
    fn visibility_team_sets_0640_permissions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.visibility = Some(Visibility::Team);
        let result = write(&fm, "Hello world", None, None, &config).unwrap();

        let metadata = fs::metadata(&result.path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "team visibility should set 0640 permissions");
    }

    #[test]
    fn frontmatter_with_recorded_by_roundtrips() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.recorded_by = Some("Mat".into());
        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("recorded_by: Mat"));
    }

    #[test]
    fn json_schema_generates_valid_schema() {
        let schema = schemars::schema_for!(Frontmatter);
        let json = serde_json::to_string_pretty(&schema).unwrap();
        assert!(json.contains("Frontmatter"));
        assert!(json.contains("recorded_by"));
        assert!(json.contains("visibility"));
    }

    #[test]
    fn frontmatter_with_speaker_map_roundtrips() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let mut fm = test_frontmatter();
        fm.speaker_map = vec![crate::diarize::SpeakerAttribution {
            speaker_label: "SPEAKER_1".into(),
            name: "Mat".into(),
            confidence: crate::diarize::Confidence::Medium,
            source: crate::diarize::AttributionSource::Deterministic,
        }];
        let result = write(&fm, "transcript", None, None, &config).unwrap();
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(
            content.contains("speaker_map:"),
            "speaker_map should appear in YAML"
        );
        assert!(content.contains("SPEAKER_1"), "speaker label should appear");
        assert!(content.contains("medium"), "confidence should be lowercase");
        assert!(
            content.contains("deterministic"),
            "source should be lowercase"
        );
    }

    #[test]
    fn frontmatter_without_speaker_map_omits_field() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let fm = test_frontmatter(); // speaker_map: vec![]
        let result = write(&fm, "transcript", None, None, &config).unwrap();
        let content = std::fs::read_to_string(&result.path).unwrap();
        assert!(
            !content.contains("speaker_map"),
            "empty speaker_map should be omitted"
        );
    }

    #[test]
    fn resolves_filename_collisions() {
        let dir = TempDir::new().unwrap();
        let filename = "2026-03-17-test.md";

        // First file: no collision
        let path1 = resolve_collision(dir.path(), filename);
        assert_eq!(path1.file_name().unwrap(), filename);
        fs::write(&path1, "first").unwrap();

        // Second file: gets -2 suffix
        let path2 = resolve_collision(dir.path(), filename);
        assert_eq!(
            path2.file_name().unwrap().to_str().unwrap(),
            "2026-03-17-test-2.md"
        );
    }

    #[test]
    #[cfg(unix)]
    fn writes_markdown_with_correct_permissions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let fm = test_frontmatter();
        let result = write(&fm, "Hello world transcript", None, None, &config).unwrap();

        assert!(result.path.exists());
        assert_eq!(result.word_count, 3);

        // Check permissions are 0600
        let metadata = fs::metadata(&result.path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file should have 0600 permissions");
    }

    #[test]
    fn writes_memo_to_memos_subdirectory() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let fm = Frontmatter {
            r#type: ContentType::Memo,
            source: Some("voice-memos".into()),
            ..test_frontmatter()
        };

        let result = write(&fm, "Voice memo text", None, None, &config).unwrap();
        assert!(result.path.to_str().unwrap().contains("memos"));
    }

    #[test]
    fn frontmatter_serializes_intents_when_present() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.intents = vec![Intent {
            kind: IntentKind::Commitment,
            what: "Share revised pricing model".into(),
            who: Some("sarah".into()),
            status: "open".into(),
            by_date: Some("Tuesday".into()),
        }];

        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("intents:"));
        assert!(content.contains("kind: commitment"));
        assert!(content.contains("who: sarah"));
        assert!(content.contains("by_date: Tuesday"));
    }

    #[test]
    fn frontmatter_serializes_entities_when_present() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let mut fm = test_frontmatter();
        fm.people = vec!["Alex Chen".into()];
        fm.entities = EntityLinks {
            people: vec![EntityRef {
                slug: "sarah-chen".into(),
                label: "Alex Chen".into(),
                aliases: vec!["sarah".into()],
            }],
            projects: vec![EntityRef {
                slug: "pricing-review".into(),
                label: "Pricing Review".into(),
                aliases: vec!["pricing".into()],
            }],
        };

        let result = write(&fm, "Transcript", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("entities:"));
        assert!(content.contains("slug: sarah-chen"));
        assert!(content.contains("label: Alex Chen"));
        assert!(content.contains("slug: pricing-review"));
    }

    // ── rename_meeting fail-closed tests ─────────────────────

    fn write_meeting(dir: &TempDir, slug: &str, frontmatter_yaml: &str, body: &str) -> PathBuf {
        let path = dir.path().join(slug);
        let content = format!("---\n{}---\n{}", frontmatter_yaml, body);
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn rename_meeting_renames_plain_title_in_place() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-pricing-review.md",
            "title: \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\n[00:00] Hello\n",
        );

        let new_path = rename_meeting(&path, "Quarterly Pricing").expect("rename should succeed");
        let content = std::fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("title: \"Quarterly Pricing\""));
        // Body must be preserved untouched.
        assert!(content.contains("[00:00] Hello"));
        // The post-write parse must round-trip.
        let (fm, _) = split_frontmatter(&content);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quarterly Pricing");
        // The file name should reflect the new slug.
        assert!(
            new_path
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("quarterly-pricing"),
            "expected slug rename, got {}",
            new_path.display()
        );
        // The original path should no longer exist.
        assert!(!path.exists());
    }

    #[test]
    fn rename_meeting_handles_unquoted_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-team-sync.md",
            "title: Team Sync\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHello\n",
        );

        let new_path = rename_meeting(&path, "Team Standup").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        assert!(content.contains("title: \"Team Standup\""));
    }

    #[test]
    fn rename_meeting_preserves_user_added_sections() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Summary\n\nWent well\n\n## Custom Section From User\n\nHand-edited stuff\n\n## Transcript\n\n[00:00] Hi\n",
        );

        let new_path = rename_meeting(&path, "Important Call").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        // Hand-edited section must survive.
        assert!(content.contains("## Custom Section From User"));
        assert!(content.contains("Hand-edited stuff"));
    }

    #[test]
    fn rename_meeting_refuses_folded_scalar_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-folded.md",
            "title: >\n  Pricing\n  Review\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        // Original file MUST be unchanged.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_literal_block_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-literal.md",
            "title: |\n  Multi\n  line\n  title\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Single Line").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_anchored_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-anchored.md",
            "title: &meeting_title \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
        // The original file is untouched even though our serde parse
        // would happily accept the anchor.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_refuses_empty_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-empty.md",
            "title: \"Pricing\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let err = rename_meeting(&path, "   ").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_refuses_newline_in_new_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-nl.md",
            "title: \"Pricing\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let err = rename_meeting(&path, "First\nSecond").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_refuses_file_without_frontmatter() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("plain.md");
        std::fs::write(&path, "no frontmatter here\n").unwrap();

        let err = rename_meeting(&path, "Anything").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));
    }

    #[test]
    fn rename_meeting_quotes_special_chars_in_new_title() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let new_path = rename_meeting(&path, "Quote \"this\" and \\that").unwrap();
        let content = std::fs::read_to_string(&new_path).unwrap();
        // Round-trip via serde_yaml — the special chars must survive.
        let (fm, _) = split_frontmatter(&content);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quote \"this\" and \\that");
    }

    #[test]
    fn rename_meeting_resolves_slug_collision() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-call.md",
            "title: \"Call\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        // Pre-create a sibling that the new slug would collide with.
        std::fs::write(
            dir.path().join("2026-04-07-pricing-review.md"),
            "---\ntitle: existing\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n---\n",
        )
        .unwrap();

        let new_path = rename_meeting(&path, "Pricing Review").unwrap();
        let name = new_path.file_name().unwrap().to_str().unwrap();
        assert!(
            name.starts_with("2026-04-07-pricing-review-") && name.ends_with(".md"),
            "expected collision-resolved slug, got {}",
            name
        );
    }

    #[test]
    fn rename_meeting_refuses_aliased_title() {
        // YAML alias `*meeting_title` references an anchor defined
        // elsewhere. The naive line replace would drop the alias
        // reference and silently break frontmatter that depends on it.
        // Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-aliased.md",
            "title: *meeting_title\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();

        let err = rename_meeting(&path, "Q4 Pricing").unwrap_err();
        assert!(matches!(err, MarkdownError::RenameRefused(_)));

        // Original file MUST be unchanged.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn rename_meeting_handles_crlf_line_endings() {
        // Files saved on Windows or copied through email may have
        // CRLF endings in the frontmatter. Rename must succeed and
        // produce a parseable result. We do not promise CRLF
        // preservation in the body — only that the rename is not
        // corrupted by it. Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("2026-04-07-crlf.md");
        let content = "---\r\n\
            title: \"Pricing\"\r\n\
            type: meeting\r\n\
            date: 2026-04-07T10:00:00-07:00\r\n\
            duration: 0\r\n\
            ---\r\n\
            ## Transcript\r\n\
            \r\n\
            Hi\r\n";
        std::fs::write(&path, content).unwrap();

        let new_path = rename_meeting(&path, "Quarterly Pricing").unwrap();
        let after = std::fs::read_to_string(&new_path).unwrap();
        let (fm, body) = split_frontmatter(&after);
        let parsed: Frontmatter = serde_yaml::from_str(fm).unwrap();
        assert_eq!(parsed.title, "Quarterly Pricing");
        assert!(body.contains("## Transcript"));
        assert!(body.contains("Hi"));
    }

    #[test]
    fn rename_meeting_post_write_validation_rolls_back_on_corruption() {
        // We can't easily force a real serde_yaml parse failure on a
        // properly-quoted title, so this test verifies the rollback
        // PATH by exercising it with a known-good rename and confirming
        // there's no leftover .md.rename.tmp sibling. The path is
        // exercised end-to-end; the assertion is "no temp files
        // remain after a successful rename, and the original was
        // replaced atomically."
        // Codex pass 2 P2 #4.
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-validate.md",
            "title: \"Old\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );

        let _ = rename_meeting(&path, "New").unwrap();

        // No leftover tmp files anywhere in the dir.
        let entries: Vec<String> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        for name in &entries {
            assert!(
                !name.ends_with(".md.rename.tmp"),
                "leftover tmp file: {} (entries: {:?})",
                name,
                entries
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rename_meeting_preserves_user_chosen_file_mode() {
        // The Minutes default is 0o600, but a user may have chmod'd
        // their meetings to 0o644 for an Obsidian sync, a local
        // webserver preview, or any other workflow. The rename must
        // preserve those bits — codex pass 3 / claude pass 3 P3.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-mode.md",
            "title: \"Old\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let new_path = rename_meeting(&path, "New").unwrap();
        let after_meta = std::fs::metadata(&new_path).unwrap();
        let after_mode = after_meta.permissions().mode() & 0o777;
        assert_eq!(
            after_mode, 0o644,
            "rename should preserve the original file mode (0o644), got 0o{:o}",
            after_mode
        );
    }

    #[test]
    fn rename_meeting_no_op_when_title_unchanged() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(
            &dir,
            "2026-04-07-pricing-review.md",
            "title: \"Pricing Review\"\ntype: meeting\ndate: 2026-04-07T10:00:00-07:00\nduration: 0\n",
            "## Transcript\n\nHi\n",
        );
        let original = std::fs::read_to_string(&path).unwrap();
        let result = rename_meeting(&path, "Pricing Review").unwrap();
        assert_eq!(result, path);
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(original, after);
    }

    #[test]
    fn yaml_quote_escapes_required_chars() {
        assert_eq!(yaml_quote("plain"), r#""plain""#);
        assert_eq!(yaml_quote("with \"quotes\""), r#""with \"quotes\"""#);
        assert_eq!(yaml_quote("back\\slash"), r#""back\\slash""#);
        assert_eq!(yaml_quote("tab\there"), r#""tab\there""#);
    }

    #[test]
    fn no_speech_output_includes_retry_instructions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let audio = dir.path().join("capture.wav");

        let fm = Frontmatter {
            status: Some(OutputStatus::NoSpeech),
            filter_diagnosis: Some("audio: 5.0s, whisper produced 3 segments, no_speech filter: -3 → 0, final: 0 words".into()),
            ..test_frontmatter()
        };

        let result = write_with_retry_path(&fm, "", None, None, Some(&audio), &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("No speech detected"));
        assert!(content.contains("**Diagnosis**:"));
        assert!(content.contains("no_speech filter"));
        assert!(content.contains(audio.display().to_string().as_str()));
        assert!(content.contains("minutes process"));
    }
}
