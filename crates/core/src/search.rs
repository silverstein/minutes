use crate::config::Config;
use crate::error::SearchError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

// ──────────────────────────────────────────────────────────────
// Built-in search: walk dir + case-insensitive text match.
// Zero dependencies beyond walkdir. Fast enough for <1000 files.
//
// Config can swap to QMD engine for semantic search:
//   [search]
//   engine = "qmd"
//   qmd_collection = "meetings"
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
    pub snippet: String,
}

pub struct SearchFilters {
    pub content_type: Option<String>,
    pub since: Option<String>,
    pub attendee: Option<String>,
}

/// Search all markdown files in the meetings directory.
pub fn search(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<Vec<SearchResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
    {
        let path = entry.path();
        match process_file(path, &query_lower, filters) {
            Ok(Some(result)) => results.push(result),
            Ok(None) => {} // No match
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping file in search");
            }
        }
    }

    // Sort by date descending (newest first)
    results.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(results)
}

fn process_file(
    path: &Path,
    query: &str,
    filters: &SearchFilters,
) -> Result<Option<SearchResult>, SearchError> {
    let content = std::fs::read_to_string(path)?;

    // Parse frontmatter
    let (frontmatter_str, body) = split_frontmatter(&content);
    let title = extract_field(frontmatter_str, "title").unwrap_or_default();
    let date = extract_field(frontmatter_str, "date").unwrap_or_default();
    let content_type = extract_field(frontmatter_str, "type").unwrap_or_else(|| "meeting".into());

    // Apply filters
    if let Some(ref type_filter) = filters.content_type {
        if content_type != *type_filter {
            return Ok(None);
        }
    }
    if let Some(ref since) = filters.since {
        if date < *since {
            return Ok(None);
        }
    }
    if let Some(ref attendee) = filters.attendee {
        let attendees = extract_field(frontmatter_str, "attendees").unwrap_or_default();
        if !attendees.to_lowercase().contains(&attendee.to_lowercase()) {
            return Ok(None);
        }
    }

    // Text search (case-insensitive)
    let body_lower = body.to_lowercase();
    let title_lower = title.to_lowercase();

    if body_lower.contains(query) || title_lower.contains(query) {
        let snippet = extract_snippet(body, query);
        Ok(Some(SearchResult {
            path: path.to_path_buf(),
            title,
            date,
            content_type,
            snippet,
        }))
    } else {
        Ok(None)
    }
}

/// Split content into frontmatter and body.
fn split_frontmatter(content: &str) -> (&str, &str) {
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

/// Extract a simple key: value field from YAML frontmatter.
fn extract_field(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{}:", key);
    for line in frontmatter.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with(&prefix) {
            let value = trimmed[prefix.len()..].trim();
            // Strip quotes
            let value = value.trim_matches('"').trim_matches('\'');
            return Some(value.to_string());
        }
    }
    None
}

/// Extract a snippet around the first match of the query.
fn extract_snippet(body: &str, query: &str) -> String {
    let lower = body.to_lowercase();
    if let Some(pos) = lower.find(query) {
        let start = body[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = body[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(body.len());

        let line = body[start..end].trim();
        if line.len() > 200 {
            format!("{}...", &line[..200])
        } else {
            line.to_string()
        }
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_file(dir: &Path, name: &str, content: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn search_finds_matching_content() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Test Meeting\ndate: 2026-03-17\ntype: meeting\n---\n\n## Transcript\n\nWe discussed pricing strategy in detail.",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
        };

        let results = search("pricing", &config, &filters).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].snippet.contains("pricing"));
    }

    #[test]
    fn search_returns_empty_for_no_match() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ndate: 2026-03-17\n---\n\nHello world.",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
        };

        let results = search("nonexistent", &config, &filters).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn search_is_case_insensitive() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ndate: 2026-03-17\n---\n\nPRICING discussion",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
        };

        let results = search("pricing", &config, &filters).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_empty_directory() {
        let dir = TempDir::new().unwrap();
        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
        };

        let results = search("anything", &config, &filters).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn split_frontmatter_works() {
        let content = "---\ntitle: Test\ndate: 2026-03-17\n---\n\nBody text here.";
        let (fm, body) = split_frontmatter(content);
        assert!(fm.contains("title: Test"));
        assert!(body.contains("Body text here"));
    }

    #[test]
    fn extract_field_finds_value() {
        let fm = "title: My Meeting\ndate: 2026-03-17\ntype: meeting";
        assert_eq!(extract_field(fm, "title"), Some("My Meeting".into()));
        assert_eq!(extract_field(fm, "type"), Some("meeting".into()));
        assert_eq!(extract_field(fm, "nonexistent"), None);
    }
}
