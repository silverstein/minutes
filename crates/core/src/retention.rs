use crate::config::Config;
use crate::markdown::{split_frontmatter, Frontmatter, OutputStatus};
use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionAudioClass {
    Successful,
    FailedOrNeedsReview,
    RuntimeScratch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RetentionAction {
    Keep,
    DeleteCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionAudioItem {
    pub path: PathBuf,
    pub bytes: u64,
    pub class: RetentionAudioClass,
    pub action: RetentionAction,
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub age_days: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RetentionTotals {
    pub raw_audio_bytes: u64,
    pub delete_candidate_bytes: u64,
    pub raw_audio_files: usize,
    pub delete_candidate_files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetentionPlan {
    pub generated_at: DateTime<Local>,
    pub output_dir: PathBuf,
    pub items: Vec<RetentionAudioItem>,
    pub totals: RetentionTotals,
}

pub fn preview_audio_retention(config: &Config, now: DateTime<Local>) -> RetentionPlan {
    let mut items = Vec::new();
    let mut seen_audio = HashSet::new();

    collect_library_audio(config, now, &mut seen_audio, &mut items);
    collect_runtime_scratch(config, &seen_audio, &mut items);

    let totals = totals_for(&items);
    RetentionPlan {
        generated_at: now,
        output_dir: config.output_dir.clone(),
        items,
        totals,
    }
}

fn collect_library_audio(
    config: &Config,
    now: DateTime<Local>,
    seen_audio: &mut HashSet<PathBuf>,
    items: &mut Vec<RetentionAudioItem>,
) {
    for entry in walkdir::WalkDir::new(&config.output_dir)
        .into_iter()
        .flatten()
        .filter(|entry| entry.file_type().is_file())
    {
        let md_path = entry.path();
        if md_path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = fs::read_to_string(md_path) else {
            continue;
        };
        let (frontmatter_str, _) = split_frontmatter(&content);
        let Ok(frontmatter) = serde_yaml::from_str::<Frontmatter>(frontmatter_str) else {
            continue;
        };

        let pinned = config.retention.keep_pinned_audio && audio_is_pinned(frontmatter_str);
        let class = if frontmatter.status == Some(OutputStatus::NoSpeech) {
            RetentionAudioClass::FailedOrNeedsReview
        } else {
            RetentionAudioClass::Successful
        };
        let age_days = now
            .signed_duration_since(frontmatter.date)
            .num_days()
            .max(0);
        let retention_days = match class {
            RetentionAudioClass::Successful => config.retention.successful_audio_days,
            RetentionAudioClass::FailedOrNeedsReview => config.retention.failed_audio_days,
            RetentionAudioClass::RuntimeScratch => 0,
        } as i64;

        let (action, reason) = library_action_and_reason(pinned, age_days, retention_days, &class);

        for path in library_audio_paths(md_path) {
            let Ok(metadata) = fs::metadata(&path) else {
                continue;
            };
            seen_audio.insert(path.clone());
            items.push(RetentionAudioItem {
                path,
                bytes: metadata.len(),
                class: class.clone(),
                action: action.clone(),
                reason: reason.clone(),
                markdown_path: Some(md_path.to_path_buf()),
                age_days: Some(age_days),
            });
        }
    }
}

fn library_audio_paths(markdown_path: &Path) -> Vec<PathBuf> {
    crate::capture::meeting_audio_artifact_paths(markdown_path)
        .into_iter()
        .filter(|path| is_audio_path(path))
        .collect()
}

fn library_action_and_reason(
    pinned: bool,
    age_days: i64,
    retention_days: i64,
    class: &RetentionAudioClass,
) -> (RetentionAction, String) {
    if pinned {
        return (
            RetentionAction::Keep,
            "audio retention pinned in frontmatter".into(),
        );
    }
    if age_days <= retention_days {
        return (
            RetentionAction::Keep,
            format!("within {} day retention window", retention_days),
        );
    }

    let reason = match class {
        RetentionAudioClass::Successful => {
            format!(
                "successful recording audio older than {} days",
                retention_days
            )
        }
        RetentionAudioClass::FailedOrNeedsReview => {
            format!(
                "failed/needs-review audio older than {} days",
                retention_days
            )
        }
        RetentionAudioClass::RuntimeScratch => "runtime scratch audio".into(),
    };
    (RetentionAction::DeleteCandidate, reason)
}

fn collect_runtime_scratch(
    config: &Config,
    seen_audio: &HashSet<PathBuf>,
    items: &mut Vec<RetentionAudioItem>,
) {
    for root in [
        Config::minutes_dir().join("native-captures"),
        Config::minutes_dir().join("jobs"),
        config.output_dir.join("failed-captures"),
    ] {
        if !root.exists() {
            continue;
        }
        for entry in walkdir::WalkDir::new(root)
            .max_depth(2)
            .into_iter()
            .flatten()
            .filter(|entry| entry.file_type().is_file())
        {
            let path = entry.path().to_path_buf();
            if !is_audio_path(&path) || seen_audio.contains(&path) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            items.push(RetentionAudioItem {
                path,
                bytes: metadata.len(),
                class: RetentionAudioClass::RuntimeScratch,
                action: RetentionAction::Keep,
                reason: "runtime/recovery scratch is inventoried but not auto-deleted yet".into(),
                markdown_path: None,
                age_days: None,
            });
        }
    }
}

fn totals_for(items: &[RetentionAudioItem]) -> RetentionTotals {
    let mut totals = RetentionTotals::default();
    for item in items {
        totals.raw_audio_bytes += item.bytes;
        totals.raw_audio_files += 1;
        if item.action == RetentionAction::DeleteCandidate {
            totals.delete_candidate_bytes += item.bytes;
            totals.delete_candidate_files += 1;
        }
    }
    totals
}

fn is_audio_path(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase()),
        Some(ext) if matches!(ext.as_str(), "wav" | "m4a" | "mp3" | "ogg" | "webm" | "mov")
    )
}

fn audio_is_pinned(frontmatter: &str) -> bool {
    let Ok(value) = serde_yaml::from_str::<serde_yaml::Value>(frontmatter) else {
        return false;
    };
    yaml_string_field(&value, "audio_retention")
        .is_some_and(|value| value.eq_ignore_ascii_case("pinned"))
        || yaml_bool_field(&value, "keep_audio").unwrap_or(false)
}

fn yaml_string_field<'a>(value: &'a serde_yaml::Value, key: &str) -> Option<&'a str> {
    value
        .as_mapping()?
        .get(serde_yaml::Value::String(key.into()))?
        .as_str()
}

fn yaml_bool_field(value: &serde_yaml::Value, key: &str) -> Option<bool> {
    value
        .as_mapping()?
        .get(serde_yaml::Value::String(key.into()))?
        .as_bool()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn with_temp_home<T>(f: impl FnOnce(&TempDir) -> T) -> T {
        let _guard = crate::test_home_env_lock();
        let dir = tempfile::tempdir().unwrap();
        let original_home = std::env::var_os("HOME");
        let original_userprofile = std::env::var_os("USERPROFILE");
        std::env::set_var("HOME", dir.path());
        std::env::set_var("USERPROFILE", dir.path());
        let result = f(&dir);
        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(up) = original_userprofile {
            std::env::set_var("USERPROFILE", up);
        } else {
            std::env::remove_var("USERPROFILE");
        }
        result
    }

    fn write_meeting(path: &Path, date: &str, extra: &str) {
        fs::write(
            path,
            format!(
                "---\ntitle: Test\ntype: meeting\ndate: {}\nduration: 5m\n{}---\n\nBody\n",
                date, extra
            ),
        )
        .unwrap();
    }

    #[test]
    fn preview_marks_expired_successful_audio_candidates() {
        with_temp_home(|tmp| {
            let mut config = Config {
                output_dir: tmp.path().join("meetings"),
                ..Config::default()
            };
            config.retention.successful_audio_days = 30;
            fs::create_dir_all(&config.output_dir).unwrap();
            let md = config.output_dir.join("old.md");
            write_meeting(&md, "2026-04-01T09:00:00-07:00", "");
            fs::write(config.output_dir.join("old.wav"), b"mixed").unwrap();
            fs::write(config.output_dir.join("old.voice.wav"), b"voice").unwrap();
            fs::write(config.output_dir.join("old.system.wav"), b"system").unwrap();
            fs::write(config.output_dir.join(".old.embeddings"), b"embeddings").unwrap();

            let now = Local.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
            let plan = preview_audio_retention(&config, now);

            assert_eq!(plan.items.len(), 3);
            assert_eq!(plan.totals.delete_candidate_files, 3);
            assert_eq!(
                plan.totals.delete_candidate_bytes,
                "mixedvoicesystem".len() as u64
            );
            assert!(plan.items.iter().all(|item| {
                item.action == RetentionAction::DeleteCandidate
                    && item.class == RetentionAudioClass::Successful
            }));
            assert!(!plan
                .items
                .iter()
                .any(|item| item.path.file_name().unwrap() == ".old.embeddings"));
        });
    }

    #[test]
    fn preview_keeps_pinned_audio_even_when_expired() {
        with_temp_home(|tmp| {
            let config = Config {
                output_dir: tmp.path().join("meetings"),
                ..Config::default()
            };
            fs::create_dir_all(&config.output_dir).unwrap();
            let md = config.output_dir.join("pinned.md");
            write_meeting(
                &md,
                "2026-04-01T09:00:00-07:00",
                "audio_retention: pinned\n",
            );
            fs::write(config.output_dir.join("pinned.wav"), b"mixed").unwrap();

            let now = Local.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
            let plan = preview_audio_retention(&config, now);

            assert_eq!(plan.items.len(), 1);
            assert_eq!(plan.items[0].action, RetentionAction::Keep);
            assert_eq!(
                plan.items[0].reason,
                "audio retention pinned in frontmatter"
            );
        });
    }

    #[test]
    fn failed_or_needs_review_audio_uses_longer_window() {
        with_temp_home(|tmp| {
            let mut config = Config {
                output_dir: tmp.path().join("meetings"),
                ..Config::default()
            };
            config.retention.successful_audio_days = 30;
            config.retention.failed_audio_days = 90;
            fs::create_dir_all(&config.output_dir).unwrap();
            let md = config.output_dir.join("review.md");
            write_meeting(&md, "2026-04-01T09:00:00-07:00", "status: no-speech\n");
            fs::write(config.output_dir.join("review.wav"), b"mixed").unwrap();

            let now = Local.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
            let plan = preview_audio_retention(&config, now);

            assert_eq!(
                plan.items[0].class,
                RetentionAudioClass::FailedOrNeedsReview
            );
            assert_eq!(plan.items[0].action, RetentionAction::Keep);
            assert_eq!(plan.items[0].reason, "within 90 day retention window");
        });
    }

    #[test]
    fn runtime_scratch_is_inventoried_but_not_delete_candidate() {
        with_temp_home(|tmp| {
            let config = Config {
                output_dir: tmp.path().join("meetings"),
                ..Config::default()
            };
            fs::create_dir_all(Config::minutes_dir().join("native-captures")).unwrap();
            fs::write(
                Config::minutes_dir().join("native-captures/call.voice.wav"),
                b"voice",
            )
            .unwrap();

            let now = Local.with_ymd_and_hms(2026, 5, 12, 9, 0, 0).unwrap();
            let plan = preview_audio_retention(&config, now);

            assert_eq!(plan.items.len(), 1);
            assert_eq!(plan.items[0].class, RetentionAudioClass::RuntimeScratch);
            assert_eq!(plan.items[0].action, RetentionAction::Keep);
            assert_eq!(plan.totals.delete_candidate_files, 0);
        });
    }
}
