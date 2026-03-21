use crate::parse::parse_meeting;
use crate::types::{ActionItem, ParsedMeeting};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// List meetings from a directory, sorted by date descending.
pub fn list_meetings(dir: &Path, limit: usize) -> Vec<ParsedMeeting> {
    let mut meetings: Vec<ParsedMeeting> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|e| parse_meeting(e.path()).ok())
        .collect();

    meetings.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    meetings.truncate(limit);
    meetings
}

/// Search meetings by a text query in title and body.
pub fn search_meetings(dir: &Path, query: &str, limit: usize) -> Vec<ParsedMeeting> {
    let query_lower = query.to_lowercase();

    let mut results: Vec<ParsedMeeting> = WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
        .filter_map(|e| parse_meeting(e.path()).ok())
        .filter(|m| {
            m.frontmatter.title.to_lowercase().contains(&query_lower)
                || m.body.to_lowercase().contains(&query_lower)
        })
        .collect();

    results.sort_by(|a, b| b.frontmatter.date.cmp(&a.frontmatter.date));
    results.truncate(limit);
    results
}

/// Find open action items across all meetings in a directory.
pub fn find_open_actions(dir: &Path, assignee: Option<&str>) -> Vec<(PathBuf, ActionItem)> {
    let mut results = Vec::new();

    for entry in WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
    {
        if let Ok(meeting) = parse_meeting(entry.path()) {
            for item in &meeting.frontmatter.action_items {
                if item.status != "open" {
                    continue;
                }
                if let Some(filter) = assignee {
                    if !item.assignee.eq_ignore_ascii_case(filter) {
                        continue;
                    }
                }
                results.push((meeting.path.clone(), item.clone()));
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn write_meeting(dir: &Path, name: &str, title: &str, body: &str) {
        let path = dir.join(name);
        let content = format!(
            "---\ntitle: {}\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n{}\n",
            title, body
        );
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn list_meetings_returns_sorted() {
        let dir = TempDir::new().unwrap();
        write_meeting(dir.path(), "a.md", "First", "Body A");
        write_meeting(dir.path(), "b.md", "Second", "Body B");

        let meetings = list_meetings(dir.path(), 10);
        assert_eq!(meetings.len(), 2);
    }

    #[test]
    fn search_meetings_filters_by_query() {
        let dir = TempDir::new().unwrap();
        write_meeting(
            dir.path(),
            "a.md",
            "Pricing Discussion",
            "Talked about pricing",
        );
        write_meeting(dir.path(), "b.md", "Onboarding", "New hire onboarding");

        let results = search_meetings(dir.path(), "pricing", 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].frontmatter.title, "Pricing Discussion");
    }

    #[test]
    fn find_open_actions_works() {
        let dir = TempDir::new().unwrap();
        let content = "---\ntitle: Test\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items:\n  - assignee: mat\n    task: Send doc\n    status: open\ndecisions: []\nintents: []\n---\n\nTranscript\n";
        std::fs::write(dir.path().join("test.md"), content).unwrap();

        let actions = find_open_actions(dir.path(), None);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].1.assignee, "mat");

        let filtered = find_open_actions(dir.path(), Some("nobody"));
        assert!(filtered.is_empty());
    }
}
