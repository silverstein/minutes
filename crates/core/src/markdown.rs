use crate::config::Config;
use crate::error::MarkdownError;
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

// ──────────────────────────────────────────────────────────────
// Meeting/memo markdown output.
// All files written with 0600 permissions (owner read/write only)
// because transcripts contain sensitive conversation content.
// ──────────────────────────────────────────────────────────────

/// Content types for output files.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentType {
    Meeting,
    Memo,
}

/// Output status markers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputStatus {
    Complete,
    NoSpeech,
    TranscriptOnly,
}

/// Frontmatter for a meeting/memo markdown file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frontmatter {
    pub title: String,
    pub r#type: ContentType,
    pub date: DateTime<Local>,
    pub duration: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<OutputStatus>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calendar_event: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub people: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

/// Result of writing a meeting/memo to disk.
#[derive(Debug, Clone, Serialize)]
pub struct WriteResult {
    pub path: PathBuf,
    pub title: String,
    pub word_count: usize,
    pub content_type: ContentType,
}

/// Write a meeting/memo to markdown with YAML frontmatter.
pub fn write(
    frontmatter: &Frontmatter,
    transcript: &str,
    summary: Option<&str>,
    user_notes: Option<&str>,
    config: &Config,
) -> Result<WriteResult, MarkdownError> {
    let output_dir = match frontmatter.r#type {
        ContentType::Memo => config.output_dir.join("memos"),
        ContentType::Meeting => config.output_dir.clone(),
    };

    // Ensure output directory exists
    fs::create_dir_all(&output_dir)
        .map_err(|e| MarkdownError::OutputDirError(format!("{}: {}", output_dir.display(), e)))?;

    // Generate filename slug
    let slug = generate_slug(&frontmatter.title, frontmatter.date);
    let path = resolve_collision(&output_dir, &slug);

    // Build markdown content
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
        content.push_str(&format!(
            "To retry with a different model:\n`minutes process {} --model large-v3`\n\n",
            path.display()
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

    // Write file with 0600 permissions
    fs::write(&path, &content)?;
    set_permissions_0600(&path)?;

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

/// Generate a URL-safe filename slug from title and date.
fn generate_slug(title: &str, date: DateTime<Local>) -> String {
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

    let slug = if title_slug.is_empty() {
        format!("{}-untitled", date_prefix)
    } else {
        // Truncate long titles
        let truncated: String = title_slug.chars().take(60).collect();
        format!("{}-{}", date_prefix, truncated)
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

/// Set file permissions to 0600 (owner read/write only).
fn set_permissions_0600(path: &Path) -> Result<(), MarkdownError> {
    let perms = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, perms)?;
    Ok(())
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
            context: None,
        }
    }

    #[test]
    fn generates_correct_slug() {
        let date = Local::now();
        let slug = generate_slug("Advisor Pricing Discussion", date);
        let prefix = date.format("%Y-%m-%d").to_string();
        assert!(slug.starts_with(&prefix));
        assert!(slug.contains("advisor-pricing-discussion"));
        assert!(slug.ends_with(".md"));
    }

    #[test]
    fn generates_untitled_slug_for_empty_title() {
        let date = Local::now();
        let slug = generate_slug("", date);
        assert!(slug.contains("untitled"));
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
    fn no_speech_output_includes_retry_instructions() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let fm = Frontmatter {
            status: Some(OutputStatus::NoSpeech),
            ..test_frontmatter()
        };

        let result = write(&fm, "", None, None, &config).unwrap();
        let content = fs::read_to_string(&result.path).unwrap();
        assert!(content.contains("No speech detected"));
        assert!(content.contains("minutes process"));
    }
}
