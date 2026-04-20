use crate::config::Config;
use crate::markdown::{ContentType, Frontmatter, WriteResult};
use chrono::{DateTime, Local};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub fn append_backlink(
    result: &WriteResult,
    note_date: DateTime<Local>,
    summary: Option<&str>,
    frontmatter: Option<&Frontmatter>,
    config: &Config,
) -> std::io::Result<Option<PathBuf>> {
    if !config.daily_notes.enabled {
        return Ok(None);
    }

    let note_dir = &config.daily_notes.path;
    fs::create_dir_all(note_dir)?;

    let note_path = note_dir.join(format!("{}.md", note_date.format("%Y-%m-%d")));
    let section = match result.content_type {
        ContentType::Meeting => "Meetings",
        ContentType::Memo => "Voice Memos",
        ContentType::Dictation => "Dictations",
    };
    let link_target = relative_or_absolute_link(note_dir, &result.path);
    let bullet = if let Some(excerpt) = summary_excerpt(summary) {
        format!("- [{}]({}) — {}", result.title, link_target, excerpt)
    } else {
        format!("- [{}]({})", result.title, link_target)
    };

    // Build sub-bullets from structured frontmatter data
    let sub_bullets = format_structured_data(frontmatter);
    let full_entry = if sub_bullets.is_empty() {
        format!("{}\n", bullet)
    } else {
        format!("{}\n{}", bullet, sub_bullets)
    };

    let mut content = if note_path.exists() {
        fs::read_to_string(&note_path)?
    } else {
        format!("# {}\n\n", note_date.format("%Y-%m-%d"))
    };

    if content.contains(&format!("]({})", link_target)) {
        return Ok(Some(note_path));
    }

    if let Some(index) = content.find(&format!("## {}\n", section)) {
        let insert_at = section_insert_position(&content[index..]).map(|offset| index + offset);
        let position = insert_at.unwrap_or(content.len());
        if position > 0 && !content[..position].ends_with('\n') {
            content.insert(position, '\n');
        }
        content.insert_str(position, &full_entry);
    } else {
        if !content.ends_with("\n\n") {
            if !content.ends_with('\n') {
                content.push('\n');
            }
            content.push('\n');
        }
        content.push_str(&format!("## {}\n\n{}", section, full_entry));
    }

    fs::write(&note_path, content)?;
    Ok(Some(note_path))
}

fn format_structured_data(frontmatter: Option<&Frontmatter>) -> String {
    let fm = match frontmatter {
        Some(fm) => fm,
        None => return String::new(),
    };

    let mut lines = Vec::new();

    for decision in &fm.decisions {
        let topic_suffix = decision
            .topic
            .as_deref()
            .map(|t| format!(" ({})", t))
            .unwrap_or_default();
        lines.push(format!(
            "  - **Decision**: {}{}",
            decision.text, topic_suffix
        ));
    }

    for item in &fm.action_items {
        if item.status == "done" {
            continue;
        }
        let due_suffix = item
            .due
            .as_deref()
            .map(|d| format!(", due {}", d))
            .unwrap_or_default();
        lines.push(format!(
            "  - [ ] @{}: {}{}",
            item.assignee, item.task, due_suffix
        ));
    }

    if lines.is_empty() {
        String::new()
    } else {
        let mut out = String::new();
        for line in lines {
            out.push_str(&line);
            out.push('\n');
        }
        out
    }
}

fn section_insert_position(section_text: &str) -> Option<usize> {
    let body = section_text.find('\n').map(|idx| idx + 1)?;
    let remainder = &section_text[body..];
    remainder.find("\n## ").map(|idx| body + idx)
}

fn summary_excerpt(summary: Option<&str>) -> Option<String> {
    let summary = summary?;
    let line = summary
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("## "))?;
    let cleaned = line.trim_start_matches("- ").trim();
    if cleaned.is_empty() {
        return None;
    }

    let excerpt: String = cleaned.chars().take(140).collect();
    if cleaned.chars().count() > 140 {
        Some(format!("{}...", excerpt))
    } else {
        Some(excerpt)
    }
}

fn relative_or_absolute_link(from_dir: &Path, target: &Path) -> String {
    relative_path(from_dir, target)
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_else(|| target.display().to_string())
}

fn relative_path(from_dir: &Path, target: &Path) -> Option<PathBuf> {
    let from = fs::canonicalize(from_dir).ok()?;
    let to = fs::canonicalize(target).ok()?;

    let from_components: Vec<_> = from.components().collect();
    let to_components: Vec<_> = to.components().collect();

    let mut common = 0usize;
    while common < from_components.len()
        && common < to_components.len()
        && from_components[common] == to_components[common]
    {
        common += 1;
    }

    let mut relative = PathBuf::new();
    for component in &from_components[common..] {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &to_components[common..] {
        relative.push(component.as_os_str());
    }

    Some(relative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::markdown::{ActionItem, Decision};
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn write_result(path: PathBuf, title: &str, content_type: ContentType) -> WriteResult {
        WriteResult {
            path,
            title: title.to_string(),
            word_count: 10,
            content_type,
        }
    }

    #[test]
    fn append_backlink_creates_daily_note_sections() {
        let dir = TempDir::new().unwrap();
        let meetings_dir = dir.path().join("meetings");
        let daily_dir = dir.path().join("daily");
        fs::create_dir_all(&meetings_dir).unwrap();
        let meeting_path = meetings_dir.join("2026-03-19-pricing-review.md");
        fs::write(&meeting_path, "# Pricing Review\n").unwrap();

        let config = Config {
            output_dir: meetings_dir.clone(),
            daily_notes: crate::config::DailyNotesConfig {
                enabled: true,
                path: daily_dir.clone(),
            },
            ..Config::default()
        };

        let result = write_result(meeting_path, "Pricing Review", ContentType::Meeting);
        let note_path = append_backlink(
            &result,
            Local.with_ymd_and_hms(2026, 3, 19, 9, 0, 0).unwrap(),
            Some("## Summary\n\n- Locked pricing at monthly billing.\n"),
            None,
            &config,
        )
        .unwrap()
        .unwrap();

        let note = fs::read_to_string(note_path).unwrap();
        assert!(note.contains("# 2026-03-19"));
        assert!(note.contains("## Meetings"));
        assert!(note.contains("[Pricing Review]("));
        assert!(note.contains("Locked pricing at monthly billing."));
    }

    #[test]
    fn append_backlink_is_idempotent_for_same_artifact() {
        let dir = TempDir::new().unwrap();
        let meetings_dir = dir.path().join("meetings");
        let daily_dir = dir.path().join("daily");
        fs::create_dir_all(&meetings_dir).unwrap();
        let memo_path = meetings_dir.join("memos").join("2026-03-19-onboarding.md");
        fs::create_dir_all(memo_path.parent().unwrap()).unwrap();
        fs::write(&memo_path, "# Onboarding Idea\n").unwrap();

        let config = Config {
            output_dir: meetings_dir.clone(),
            daily_notes: crate::config::DailyNotesConfig {
                enabled: true,
                path: daily_dir.clone(),
            },
            ..Config::default()
        };

        let result = write_result(memo_path, "Onboarding Idea", ContentType::Memo);
        let date = Local.with_ymd_and_hms(2026, 3, 19, 9, 0, 0).unwrap();

        append_backlink(&result, date, Some("Short memo summary"), None, &config).unwrap();
        append_backlink(&result, date, Some("Short memo summary"), None, &config).unwrap();

        let note = fs::read_to_string(daily_dir.join("2026-03-19.md")).unwrap();
        assert_eq!(note.matches("[Onboarding Idea](").count(), 1);
        assert!(note.contains("## Voice Memos"));
    }

    #[test]
    fn append_backlink_includes_decisions_and_action_items() {
        let dir = TempDir::new().unwrap();
        let meetings_dir = dir.path().join("meetings");
        let daily_dir = dir.path().join("daily");
        fs::create_dir_all(&meetings_dir).unwrap();
        let meeting_path = meetings_dir.join("2026-03-19-strategy-call.md");
        fs::write(&meeting_path, "# Strategy Call\n").unwrap();

        let config = Config {
            output_dir: meetings_dir.clone(),
            daily_notes: crate::config::DailyNotesConfig {
                enabled: true,
                path: daily_dir.clone(),
            },
            ..Config::default()
        };

        let mut fm = Frontmatter {
            title: "Strategy Call".into(),
            r#type: ContentType::Meeting,
            date: Local.with_ymd_and_hms(2026, 3, 19, 9, 0, 0).unwrap(),
            duration: "30m".into(),
            source: None,
            status: None,
            tags: vec![],
            attendees: vec![],
            attendees_raw: None,
            calendar_event: None,
            people: vec![],
            entities: Default::default(),
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
        };
        fm.decisions = vec![Decision {
            text: "Switch to monthly billing".into(),
            topic: Some("pricing".into()),
            authority: None,
            supersedes: None,
        }];
        fm.action_items = vec![
            ActionItem {
                assignee: "mat".into(),
                task: "Send pricing doc".into(),
                due: Some("2026-03-22".into()),
                status: "open".into(),
            },
            ActionItem {
                assignee: "dan".into(),
                task: "Already done task".into(),
                due: None,
                status: "done".into(),
            },
        ];

        let result = write_result(meeting_path, "Strategy Call", ContentType::Meeting);
        let note_path = append_backlink(
            &result,
            Local.with_ymd_and_hms(2026, 3, 19, 9, 0, 0).unwrap(),
            Some("Strategy discussion"),
            Some(&fm),
            &config,
        )
        .unwrap()
        .unwrap();

        let note = fs::read_to_string(note_path).unwrap();
        assert!(note.contains("**Decision**: Switch to monthly billing (pricing)"));
        assert!(note.contains("- [ ] @mat: Send pricing doc, due 2026-03-22"));
        // "done" items should be excluded
        assert!(!note.contains("Already done task"));
    }
}
