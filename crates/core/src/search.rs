use crate::config::Config;
use crate::error::SearchError;
use crate::markdown::{extract_field, split_frontmatter, Frontmatter, IntentKind};
use crate::overlays;
use chrono::Local;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directories within `output_dir` that should be excluded from search results.
/// These contain archived, processed, or failed files that are not active meetings.
const EXCLUDED_DIRS: &[&str] = &["archive", "processed", "failed", "failed-captures"];

/// Walk `dir` for `.md` files, skipping excluded subdirectories.
fn walk_meeting_files(dir: &Path) -> impl Iterator<Item = walkdir::DirEntry> {
    WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                !EXCLUDED_DIRS.contains(&name.as_ref())
            } else {
                true
            }
        })
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
}

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

#[derive(Debug, Clone, Serialize)]
pub struct IntentResult {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
    pub kind: IntentKind,
    pub what: String,
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_provenance: Option<String>,
    pub status: String,
    pub by_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ReportEntry {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub what: String,
    pub who: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_original: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub who_provenance: Option<String>,
    pub by_date: Option<String>,
    /// Frontmatter v2: optional authority grade ("high" | "medium" | "low").
    /// Propagated from the source decision when present. None for pre-v2
    /// frontmatter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DecisionConflict {
    pub topic: String,
    pub latest: ReportEntry,
    pub previous: Vec<ReportEntry>,
    /// Frontmatter v2: when the latest decision explicitly `supersedes` an
    /// earlier one, this carries the supersession rationale. Consumers like
    /// `/minutes-lint` should treat resolved conflicts as informational
    /// rather than red flags. None means this is an unresolved contradiction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct OwnerResolution {
    who: Option<String>,
    who_original: Option<String>,
    who_provenance: Option<String>,
}

#[derive(Debug, Clone)]
struct SpeakerOwner {
    name: String,
    provenance: String,
}

fn speaker_overlay_map(
    frontmatter: &Frontmatter,
    overlay_db_path: &Path,
    meeting_path: &Path,
) -> HashMap<String, SpeakerOwner> {
    let mut speakers = frontmatter
        .speaker_map
        .iter()
        .filter(|attr| attr.confidence == crate::diarize::Confidence::High)
        .map(|attr| {
            (
                attr.speaker_label.clone(),
                SpeakerOwner {
                    name: attr.name.clone(),
                    provenance: "speaker_map".to_string(),
                },
            )
        })
        .collect::<HashMap<_, _>>();

    match overlays::load_speaker_confirmations_for_meeting_at(overlay_db_path, meeting_path) {
        Ok(confirmations) => {
            for confirmation in confirmations {
                speakers.insert(
                    confirmation.speaker_label,
                    SpeakerOwner {
                        name: confirmation.name,
                        provenance: "speaker overlay".to_string(),
                    },
                );
            }
        }
        Err(error) => {
            tracing::warn!(
                path = %meeting_path.display(),
                error = %error,
                "failed to load speaker overlays for reporting"
            );
        }
    }

    speakers
}

fn resolve_owner_with_speaker_overlays(
    who: Option<&str>,
    speaker_overlays: &HashMap<String, SpeakerOwner>,
) -> OwnerResolution {
    let Some(raw) = who.map(str::trim).filter(|value| !value.is_empty()) else {
        return OwnerResolution::default();
    };

    if let Some(speaker) = speaker_overlays.get(raw) {
        return OwnerResolution {
            who: Some(speaker.name.clone()),
            who_original: Some(raw.to_string()),
            who_provenance: Some(speaker.provenance.clone()),
        };
    }

    OwnerResolution {
        who: Some(raw.to_string()),
        who_original: None,
        who_provenance: None,
    }
}

fn owner_matches(resolution: &OwnerResolution, owner_lower: &str) -> bool {
    resolution
        .who
        .as_ref()
        .is_some_and(|who| who.to_lowercase().contains(owner_lower))
        || resolution
            .who_original
            .as_ref()
            .is_some_and(|who| who.to_lowercase().contains(owner_lower))
}

fn explicit_supersedes_resolution(
    latest_supersedes: Option<&str>,
    conflicting_previous: &[ReportEntry],
) -> Option<String> {
    let value = latest_supersedes
        .map(str::trim)
        .filter(|value| !value.is_empty())?;

    // `supersedes` is a free-text pointer. It's reliable for a simple
    // one-new-decision-replaces-one-prior-decision case, but not strong enough
    // to auto-resolve an entire topic arc when multiple contradictory prior
    // decisions remain. Stay conservative so `/minutes-lint` doesn't hide a
    // still-live conflict as "resolved".
    if conflicting_previous.len() != 1 {
        return None;
    }

    if !supersedes_references_previous_decision(value, &conflicting_previous[0]) {
        return None;
    }

    Some(format!("Resolved by explicit supersedes: {}", value))
}

fn supersedes_references_previous_decision(supersedes: &str, previous: &ReportEntry) -> bool {
    let supersedes_norm = normalize_decision_value(supersedes);
    if supersedes_norm.is_empty() {
        return false;
    }

    let previous_date = previous
        .date
        .split('T')
        .next()
        .unwrap_or(previous.date.as_str());
    let previous_date_norm = normalize_decision_value(previous_date);
    if !previous_date_norm.is_empty() && supersedes_norm.contains(&previous_date_norm) {
        return true;
    }

    let previous_title_norm = normalize_decision_value(&previous.title);
    if previous_title_norm.len() >= 4 && supersedes_norm.contains(&previous_title_norm) {
        return true;
    }

    let previous_what_norm = normalize_decision_value(&previous.what);
    if previous_what_norm.is_empty() {
        return false;
    }

    let supersedes_tokens = supersedes_norm
        .split_whitespace()
        .filter(|token| token.len() >= 4)
        .collect::<std::collections::HashSet<_>>();
    let previous_tokens = previous_what_norm
        .split_whitespace()
        .filter(|token| token.len() >= 4)
        .collect::<std::collections::HashSet<_>>();

    supersedes_tokens
        .intersection(&previous_tokens)
        .take(2)
        .count()
        >= 2
}

#[derive(Debug, Clone, Serialize)]
pub struct StaleCommitment {
    pub kind: IntentKind,
    pub entry: ReportEntry,
    pub meetings_since: usize,
    pub age_days: i64,
    pub reasons: Vec<String>,
    pub latest_follow_up: Option<MeetingReference>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConsistencyReport {
    pub decision_conflicts: Vec<DecisionConflict>,
    pub stale_commitments: Vec<StaleCommitment>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicSummary {
    pub topic: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MeetingReference {
    pub path: PathBuf,
    pub title: String,
    pub date: String,
    pub content_type: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PersonProfile {
    pub name: String,
    pub recent_meetings: Vec<MeetingReference>,
    pub open_intents: Vec<IntentResult>,
    pub recent_decisions: Vec<ReportEntry>,
    pub top_topics: Vec<TopicSummary>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CrossMeetingResearch {
    pub query: String,
    pub related_decisions: Vec<ReportEntry>,
    pub related_open_intents: Vec<IntentResult>,
    pub recent_meetings: Vec<MeetingReference>,
    pub related_topics: Vec<TopicSummary>,
}

pub struct SearchFilters {
    pub content_type: Option<String>,
    pub since: Option<String>,
    pub attendee: Option<String>,
    pub intent_kind: Option<IntentKind>,
    pub owner: Option<String>,
    pub recorded_by: Option<String>,
}

/// Resolve a meeting file by slug prefix (date-title pattern).
/// Returns the first match found in the output directory.
pub fn resolve_slug(slug: &str, config: &Config) -> Option<PathBuf> {
    if slug.is_empty() {
        return None;
    }

    let dir = &config.output_dir;
    if !dir.exists() {
        return None;
    }

    for entry in walk_meeting_files(dir) {
        let filename = entry
            .path()
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy();
        if filename.to_lowercase().contains(&slug.to_lowercase()) {
            return Some(entry.path().to_path_buf());
        }
    }

    None
}

pub fn cross_meeting_research(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<CrossMeetingResearch, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let query_lower = query.to_lowercase();
    let mut related_decisions = Vec::new();
    let mut related_open_intents = Vec::new();
    let mut recent_meetings = Vec::new();
    let mut topic_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    let overlay_db_path = overlays::default_db_path();

    for entry in walk_meeting_files(dir) {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping file in cross-meeting research");
                continue;
            }
        };

        let (frontmatter_str, _) = split_frontmatter(&content);
        if frontmatter_str.is_empty() {
            continue;
        }

        let frontmatter: Frontmatter = match serde_yaml::from_str(frontmatter_str) {
            Ok(frontmatter) => frontmatter,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping malformed frontmatter in cross-meeting research");
                continue;
            }
        };

        let content_type = match frontmatter.r#type {
            crate::markdown::ContentType::Meeting => "meeting".to_string(),
            crate::markdown::ContentType::Memo => "memo".to_string(),
            crate::markdown::ContentType::Dictation => "dictation".to_string(),
        };
        let speaker_overlays = speaker_overlay_map(&frontmatter, &overlay_db_path, path);
        if let Some(ref type_filter) = filters.content_type {
            if content_type != *type_filter {
                continue;
            }
        }

        let date = frontmatter.date.to_rfc3339();
        if let Some(ref since) = filters.since {
            if date < *since {
                continue;
            }
        }
        if let Some(ref attendee) = filters.attendee {
            let attendee_lower = attendee.to_lowercase();
            let attendee_match = frontmatter
                .attendees
                .iter()
                .any(|name| name.to_lowercase().contains(&attendee_lower))
                || frontmatter
                    .people
                    .iter()
                    .any(|person| person.to_lowercase().contains(&attendee_lower));
            if !attendee_match {
                continue;
            }
        }

        let meeting_matches = frontmatter.title.to_lowercase().contains(&query_lower)
            || frontmatter
                .context
                .as_ref()
                .map(|context| context.to_lowercase().contains(&query_lower))
                .unwrap_or(false);

        let mut matched_this_meeting = meeting_matches;

        for decision in &frontmatter.decisions {
            let topic = decision
                .topic
                .clone()
                .unwrap_or_else(|| normalize_topic(&decision.text));
            let haystack = format!("{} {}", topic, decision.text).to_lowercase();
            if haystack.contains(&query_lower) {
                matched_this_meeting = true;
                if !topic.is_empty() {
                    *topic_counts.entry(topic).or_insert(0) += 1;
                }
                related_decisions.push(ReportEntry {
                    path: path.to_path_buf(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    what: decision.text.clone(),
                    who: None,
                    who_original: None,
                    who_provenance: None,
                    by_date: None,
                    authority: decision.authority.clone(),
                });
            }
        }

        for intent in &frontmatter.intents {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            let haystack = format!(
                "{} {} {} {} {}",
                intent.what,
                owner_resolution.who.clone().unwrap_or_default(),
                owner_resolution.who_original.clone().unwrap_or_default(),
                intent.status,
                intent.by_date.clone().unwrap_or_default()
            )
            .to_lowercase();
            if !haystack.contains(&query_lower) {
                continue;
            }

            matched_this_meeting = true;
            let topic = normalize_topic(&intent.what);
            if !topic.is_empty() {
                *topic_counts.entry(topic).or_insert(0) += 1;
            }

            if intent.status == "open" {
                related_open_intents.push(IntentResult {
                    path: path.to_path_buf(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    content_type: content_type.clone(),
                    kind: intent.kind,
                    what: intent.what.clone(),
                    who: owner_resolution.who.clone(),
                    who_original: owner_resolution.who_original.clone(),
                    who_provenance: owner_resolution.who_provenance.clone(),
                    status: intent.status.clone(),
                    by_date: intent.by_date.clone(),
                });
            }
        }

        if matched_this_meeting {
            recent_meetings.push(MeetingReference {
                path: path.to_path_buf(),
                title: frontmatter.title.clone(),
                date,
                content_type,
            });
        }
    }

    related_decisions.sort_by(|a, b| b.date.cmp(&a.date));
    related_open_intents.sort_by(|a, b| b.date.cmp(&a.date));
    recent_meetings.sort_by(|a, b| b.date.cmp(&a.date));

    let mut related_topics: Vec<TopicSummary> = topic_counts
        .into_iter()
        .map(|(topic, count)| TopicSummary { topic, count })
        .collect();
    related_topics.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.topic.cmp(&b.topic)));

    related_decisions.truncate(10);
    related_open_intents.truncate(10);
    recent_meetings.truncate(10);
    related_topics.truncate(5);

    Ok(CrossMeetingResearch {
        query: query.to_string(),
        related_decisions,
        related_open_intents,
        recent_meetings,
        related_topics,
    })
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

    for entry in walk_meeting_files(dir) {
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

/// Search structured intents across all markdown files in the meetings directory.
pub fn search_intents(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
) -> Result<Vec<IntentResult>, SearchError> {
    search_intents_at(query, config, filters, &overlays::default_db_path())
}

fn search_intents_at(
    query: &str,
    config: &Config,
    filters: &SearchFilters,
    overlay_db_path: &Path,
) -> Result<Vec<IntentResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for entry in walk_meeting_files(dir) {
        let path = entry.path();
        match process_intent_file(path, &query_lower, filters, overlay_db_path) {
            Ok(mut file_results) => results.append(&mut file_results),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping file in intent search");
            }
        }
    }

    results.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(results)
}

pub fn consistency_report(
    config: &Config,
    owner: Option<&str>,
    stale_after_days: i64,
) -> Result<ConsistencyReport, SearchError> {
    consistency_report_at(
        config,
        owner,
        stale_after_days,
        &overlays::default_db_path(),
    )
}

fn consistency_report_at(
    config: &Config,
    owner: Option<&str>,
    stale_after_days: i64,
    overlay_db_path: &Path,
) -> Result<ConsistencyReport, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let mut parsed_frontmatters = Vec::new();
    for entry in walk_meeting_files(dir) {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping file in consistency report");
                continue;
            }
        };

        let (frontmatter_str, _) = split_frontmatter(&content);
        if frontmatter_str.is_empty() {
            continue;
        }

        match serde_yaml::from_str::<Frontmatter>(frontmatter_str) {
            Ok(frontmatter) => parsed_frontmatters.push((path.to_path_buf(), frontmatter)),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping malformed frontmatter in consistency report");
            }
        }
    }

    parsed_frontmatters.sort_by_key(|entry| entry.1.date);

    let owner_lower = owner.map(|value| value.to_lowercase());
    let now = Local::now();
    // Each entry carries its source decision's `supersedes` value alongside the
    // ReportEntry so we can detect documented supersessions when the topic
    // group has conflicting decisions.
    let mut decision_groups: std::collections::HashMap<String, Vec<(ReportEntry, Option<String>)>> =
        std::collections::HashMap::new();
    let mut stale_commitments = Vec::new();

    for (path, frontmatter) in &parsed_frontmatters {
        let speaker_overlays = speaker_overlay_map(frontmatter, overlay_db_path, path);

        for decision in &frontmatter.decisions {
            let topic = decision
                .topic
                .as_deref()
                .map(normalize_topic)
                .filter(|topic| !topic.is_empty())
                .unwrap_or_else(|| normalize_topic(&decision.text));
            if topic.is_empty() {
                continue;
            }

            decision_groups.entry(topic).or_default().push((
                ReportEntry {
                    path: path.clone(),
                    title: frontmatter.title.clone(),
                    date: frontmatter.date.to_rfc3339(),
                    what: decision.text.clone(),
                    who: None,
                    who_original: None,
                    who_provenance: None,
                    by_date: None,
                    authority: decision.authority.clone(),
                },
                decision.supersedes.clone(),
            ));
        }

        for intent in &frontmatter.intents {
            if !matches!(intent.kind, IntentKind::Commitment | IntentKind::ActionItem) {
                continue;
            }
            if intent.status != "open" {
                continue;
            }

            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            if let Some(ref owner_lower) = owner_lower {
                if !owner_matches(&owner_resolution, owner_lower) {
                    continue;
                }
            }

            let newer_meetings: Vec<_> = parsed_frontmatters
                .iter()
                .filter(|(_, newer)| newer.date > frontmatter.date)
                .collect();
            let meetings_since = newer_meetings.len();
            let age_days = now.signed_duration_since(frontmatter.date).num_days();
            let latest_follow_up =
                newer_meetings
                    .last()
                    .map(|(path, frontmatter)| MeetingReference {
                        path: path.clone(),
                        title: frontmatter.title.clone(),
                        date: frontmatter.date.to_rfc3339(),
                        content_type: match frontmatter.r#type {
                            crate::markdown::ContentType::Meeting => "meeting".to_string(),
                            crate::markdown::ContentType::Memo => "memo".to_string(),
                            crate::markdown::ContentType::Dictation => "dictation".to_string(),
                        },
                    });

            let mut reasons = Vec::new();
            if age_days >= stale_after_days {
                reasons.push(format!("{} days old", age_days));
            }
            if meetings_since >= 3 {
                reasons.push(format!("{} newer meetings since", meetings_since));
            }
            if let Some(by_date) = &intent.by_date {
                if meetings_since >= 1 || age_days >= 1 {
                    reasons.push(format!("still open with due date {}", by_date));
                }
            }
            if intent
                .who
                .as_deref()
                .is_none_or(|who| who.trim().is_empty())
            {
                reasons.push("still open without an owner".to_string());
            }

            if !reasons.is_empty() {
                stale_commitments.push(StaleCommitment {
                    kind: intent.kind,
                    entry: ReportEntry {
                        path: path.clone(),
                        title: frontmatter.title.clone(),
                        date: frontmatter.date.to_rfc3339(),
                        what: intent.what.clone(),
                        who: owner_resolution.who.clone(),
                        who_original: owner_resolution.who_original.clone(),
                        who_provenance: owner_resolution.who_provenance.clone(),
                        by_date: intent.by_date.clone(),
                        authority: None,
                    },
                    meetings_since,
                    age_days,
                    reasons,
                    latest_follow_up,
                });
            }
        }
    }

    let mut decision_conflicts = Vec::new();
    for (topic, mut entries) in decision_groups {
        entries.sort_by(|a, b| a.0.date.cmp(&b.0.date));
        let mut unique_values = std::collections::HashSet::new();
        for (entry, _) in &entries {
            unique_values.insert(normalize_decision_value(&entry.what));
        }

        if unique_values.len() > 1 {
            let (latest_entry, latest_supersedes) = entries.pop().expect("entries not empty");
            let previous_entries: Vec<ReportEntry> =
                entries.into_iter().map(|(entry, _)| entry).collect();
            let resolution =
                explicit_supersedes_resolution(latest_supersedes.as_deref(), &previous_entries);
            decision_conflicts.push(DecisionConflict {
                topic,
                latest: latest_entry,
                previous: previous_entries,
                resolution,
            });
        }
    }

    decision_conflicts.sort_by(|a, b| b.latest.date.cmp(&a.latest.date));
    stale_commitments.sort_by(|a, b| b.entry.date.cmp(&a.entry.date));

    Ok(ConsistencyReport {
        decision_conflicts,
        stale_commitments,
    })
}

pub fn person_profile(config: &Config, person: &str) -> Result<PersonProfile, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(SearchError::DirNotFound(dir.display().to_string()));
    }

    let person_lower = person.to_lowercase();
    let mut parsed_frontmatters = Vec::new();
    let overlay_db_path = overlays::default_db_path();
    for entry in walk_meeting_files(dir) {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping file in person profile");
                continue;
            }
        };

        let (frontmatter_str, _) = split_frontmatter(&content);
        if frontmatter_str.is_empty() {
            continue;
        }

        match serde_yaml::from_str::<Frontmatter>(frontmatter_str) {
            Ok(frontmatter) => parsed_frontmatters.push((path.to_path_buf(), frontmatter)),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping malformed frontmatter in person profile");
            }
        }
    }

    parsed_frontmatters.sort_by_key(|entry| std::cmp::Reverse(entry.1.date));

    let mut recent_meetings = Vec::new();
    let mut open_intents = Vec::new();
    let mut recent_decisions = Vec::new();
    let mut topic_counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();

    for (path, frontmatter) in parsed_frontmatters {
        let content_type = match frontmatter.r#type {
            crate::markdown::ContentType::Meeting => "meeting".to_string(),
            crate::markdown::ContentType::Memo => "memo".to_string(),
            crate::markdown::ContentType::Dictation => "dictation".to_string(),
        };
        let date = frontmatter.date.to_rfc3339();
        let speaker_overlays = speaker_overlay_map(&frontmatter, &overlay_db_path, &path);

        let attendee_match = frontmatter
            .attendees
            .iter()
            .any(|attendee| attendee.to_lowercase().contains(&person_lower));
        let linked_person_match = frontmatter
            .people
            .iter()
            .any(|person| person.to_lowercase().contains(&person_lower))
            || frontmatter.entities.people.iter().any(|entity| {
                entity.label.to_lowercase().contains(&person_lower)
                    || entity
                        .aliases
                        .iter()
                        .any(|alias| alias.to_lowercase().contains(&person_lower))
            });
        let owned_intent_match = frontmatter.intents.iter().any(|intent| {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            owner_matches(&owner_resolution, &person_lower)
        });

        if !(attendee_match || linked_person_match || owned_intent_match) {
            continue;
        }

        recent_meetings.push(MeetingReference {
            path: path.clone(),
            title: frontmatter.title.clone(),
            date: date.clone(),
            content_type: content_type.clone(),
        });

        for decision in &frontmatter.decisions {
            recent_decisions.push(ReportEntry {
                path: path.clone(),
                title: frontmatter.title.clone(),
                date: date.clone(),
                what: decision.text.clone(),
                who: None,
                who_original: None,
                who_provenance: None,
                by_date: None,
                authority: decision.authority.clone(),
            });

            let topic = decision
                .topic
                .clone()
                .unwrap_or_else(|| normalize_topic(&decision.text));
            if !topic.is_empty() {
                *topic_counts.entry(topic).or_insert(0) += 1;
            }
        }

        for intent in &frontmatter.intents {
            let owner_resolution =
                resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
            let owned_by_person = owner_matches(&owner_resolution, &person_lower);

            if owned_by_person
                && intent.status == "open"
                && matches!(intent.kind, IntentKind::ActionItem | IntentKind::Commitment)
            {
                open_intents.push(IntentResult {
                    path: path.clone(),
                    title: frontmatter.title.clone(),
                    date: date.clone(),
                    content_type: content_type.clone(),
                    kind: intent.kind,
                    what: intent.what.clone(),
                    who: owner_resolution.who.clone(),
                    who_original: owner_resolution.who_original.clone(),
                    who_provenance: owner_resolution.who_provenance.clone(),
                    status: intent.status.clone(),
                    by_date: intent.by_date.clone(),
                });
            }

            if attendee_match || owned_by_person {
                let topic = normalize_topic(&intent.what);
                if !topic.is_empty() {
                    *topic_counts.entry(topic).or_insert(0) += 1;
                }
            }
        }
    }

    recent_meetings.truncate(5);
    recent_decisions.truncate(5);
    open_intents.truncate(10);

    let mut top_topics: Vec<TopicSummary> = topic_counts
        .into_iter()
        .map(|(topic, count)| TopicSummary { topic, count })
        .collect();
    top_topics.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.topic.cmp(&b.topic)));
    top_topics.truncate(5);

    Ok(PersonProfile {
        name: person.to_string(),
        recent_meetings,
        open_intents,
        recent_decisions,
        top_topics,
    })
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
    if let Some(ref recorded_by) = filters.recorded_by {
        let recorded = extract_field(frontmatter_str, "recorded_by").unwrap_or_default();
        if !recorded
            .to_lowercase()
            .contains(&recorded_by.to_lowercase())
        {
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

fn process_intent_file(
    path: &Path,
    query: &str,
    filters: &SearchFilters,
    overlay_db_path: &Path,
) -> Result<Vec<IntentResult>, SearchError> {
    let content = std::fs::read_to_string(path)?;
    let (frontmatter_str, _) = split_frontmatter(&content);
    if frontmatter_str.is_empty() {
        return Ok(vec![]);
    }

    let frontmatter: Frontmatter = serde_yaml::from_str(frontmatter_str)
        .map_err(|e| SearchError::Io(std::io::Error::other(e.to_string())))?;

    let date = frontmatter.date.to_rfc3339();
    let content_type = match frontmatter.r#type {
        crate::markdown::ContentType::Meeting => "meeting".to_string(),
        crate::markdown::ContentType::Memo => "memo".to_string(),
        crate::markdown::ContentType::Dictation => "dictation".to_string(),
    };

    if let Some(ref type_filter) = filters.content_type {
        if content_type != *type_filter {
            return Ok(vec![]);
        }
    }
    if let Some(ref since) = filters.since {
        if date < *since {
            return Ok(vec![]);
        }
    }
    if let Some(ref attendee) = filters.attendee {
        let attendee_lower = attendee.to_lowercase();
        let attendee_match = frontmatter
            .attendees
            .iter()
            .any(|name| name.to_lowercase().contains(&attendee_lower));
        if !attendee_match {
            return Ok(vec![]);
        }
    }
    if let Some(ref recorded_by) = filters.recorded_by {
        let matches = frontmatter
            .recorded_by
            .as_ref()
            .is_some_and(|r| r.to_lowercase().contains(&recorded_by.to_lowercase()));
        if !matches {
            return Ok(vec![]);
        }
    }

    let speaker_overlays = speaker_overlay_map(&frontmatter, overlay_db_path, path);
    let mut results = Vec::new();
    for intent in frontmatter.intents {
        if let Some(kind) = filters.intent_kind {
            if intent.kind != kind {
                continue;
            }
        }
        let owner_resolution =
            resolve_owner_with_speaker_overlays(intent.who.as_deref(), &speaker_overlays);
        if let Some(ref owner) = filters.owner {
            let owner_lower = owner.to_lowercase();
            if !owner_matches(&owner_resolution, &owner_lower) {
                continue;
            }
        }

        let haystack = format!(
            "{} {} {} {} {} {}",
            frontmatter.title,
            intent.what,
            owner_resolution.who.clone().unwrap_or_default(),
            owner_resolution.who_original.clone().unwrap_or_default(),
            intent.status,
            intent.by_date.clone().unwrap_or_default()
        )
        .to_lowercase();

        if !query.is_empty() && !haystack.contains(query) {
            continue;
        }

        results.push(IntentResult {
            path: path.to_path_buf(),
            title: frontmatter.title.clone(),
            date: date.clone(),
            content_type: content_type.clone(),
            kind: intent.kind,
            what: intent.what,
            who: owner_resolution.who,
            who_original: owner_resolution.who_original,
            who_provenance: owner_resolution.who_provenance,
            status: intent.status,
            by_date: intent.by_date,
        });
    }

    Ok(results)
}

// split_frontmatter and extract_field are in markdown.rs (shared)

/// Find meetings with open action items, optionally filtered by assignee.
/// Parses YAML frontmatter for the structured action_items field.
pub fn find_open_actions(
    config: &Config,
    assignee: Option<&str>,
) -> Result<Vec<ActionResult>, SearchError> {
    let dir = &config.output_dir;
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut results = Vec::new();

    for entry in walk_meeting_files(dir) {
        let path = entry.path();
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let (fm_str, _) = split_frontmatter(&content);
        let title = extract_field(fm_str, "title").unwrap_or_default();
        let date = extract_field(fm_str, "date").unwrap_or_default();

        // Parse action_items from frontmatter (YAML list)
        // Look for lines like "  - assignee: mat" within the action_items block
        if !content.contains("action_items:") {
            continue;
        }

        // Simple parse: find action_items section in frontmatter YAML
        // Note: fm_str is already stripped of --- markers by split_frontmatter,
        // so pass it directly — wrapping with --- would create a multi-document
        // YAML that serde_yaml rejects.
        let parsed: Result<serde_yaml::Value, _> = serde_yaml::from_str(fm_str);
        if let Ok(yaml) = parsed {
            if let Some(items) = yaml.get("action_items").and_then(|v| v.as_sequence()) {
                for item in items {
                    let item_assignee = item
                        .get("assignee")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unassigned");
                    let item_status = item
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("open");
                    let item_task = item.get("task").and_then(|v| v.as_str()).unwrap_or("");
                    let item_due = item
                        .get("due")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());

                    if item_status != "open" {
                        continue;
                    }
                    if let Some(filter) = assignee {
                        let a = item_assignee.to_lowercase();
                        let f = filter.to_lowercase();
                        if a != f && !a.contains(&f) {
                            continue;
                        }
                    }

                    results.push(ActionResult {
                        meeting_path: path.to_path_buf(),
                        meeting_title: title.clone(),
                        meeting_date: date.clone(),
                        assignee: item_assignee.to_string(),
                        task: item_task.to_string(),
                        due: item_due,
                    });
                }
            }
        }
    }

    results.sort_by(|a, b| b.meeting_date.cmp(&a.meeting_date));
    Ok(results)
}

/// A structured action item result from cross-meeting search.
#[derive(Debug, Clone, Serialize)]
pub struct ActionResult {
    pub meeting_path: PathBuf,
    pub meeting_title: String,
    pub meeting_date: String,
    pub assignee: String,
    pub task: String,
    pub due: Option<String>,
}

/// Extract a snippet around the first match of the query.
fn extract_snippet(body: &str, query: &str) -> String {
    // Find the query in the body case-insensitively.
    // We search the original body to avoid byte-offset mismatch from to_lowercase().
    let pos = body
        .char_indices()
        .position(|(i, _)| body[i..].to_lowercase().starts_with(query))
        .and_then(|char_idx| body.char_indices().nth(char_idx).map(|(i, _)| i));

    if let Some(pos) = pos {
        let start = body[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
        let end = body[pos..]
            .find('\n')
            .map(|i| pos + i)
            .unwrap_or(body.len());

        let line = body[start..end].trim();
        if line.chars().count() > 200 {
            let truncated: String = line.chars().take(200).collect();
            format!("{}...", truncated)
        } else {
            line.to_string()
        }
    } else {
        String::new()
    }
}

fn normalize_topic(text: &str) -> String {
    let stopwords = [
        "a", "an", "and", "as", "at", "by", "for", "from", "in", "of", "on", "or", "the", "to",
        "with", "we", "should", "will", "be", "is", "are", "use", "using",
    ];

    text.split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|word| !word.is_empty())
        .filter(|word| !stopwords.contains(&word.to_lowercase().as_str()))
        .take(4)
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalize_decision_value(text: &str) -> String {
    text.chars()
        .map(|ch| {
            if ch.is_alphanumeric() || ch.is_whitespace() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
            intent_kind: None,
            owner: None,
            recorded_by: None,
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
            intent_kind: None,
            owner: None,
            recorded_by: None,
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
            intent_kind: None,
            owner: None,
            recorded_by: None,
        };

        let results = search("pricing", &config, &filters).unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_filters_by_recorded_by() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "test.md",
            "---\ntitle: Test\ndate: 2026-03-17\nrecorded_by: Mat Silver\ntype: meeting\n---\n\nPricing discussion",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("mat".into()),
        };
        let non_matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("sarah".into()),
        };

        let matching_results = search("pricing", &config, &matching_filters).unwrap();
        let non_matching_results = search("pricing", &config, &non_matching_filters).unwrap();

        assert_eq!(matching_results.len(), 1);
        assert!(non_matching_results.is_empty());
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
            intent_kind: None,
            owner: None,
            recorded_by: None,
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

    #[test]
    fn search_intents_returns_matching_structured_records() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n  - kind: commitment\n    what: Share revised pricing model\n    who: sarah\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
        };

        let overlay_db = dir.path().join("overlays.db");
        let results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            "pricing",
            &filters,
            &overlay_db,
        )
        .unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Pricing Review");
        assert!(results
            .iter()
            .any(|item| item.kind == IntentKind::ActionItem));
        assert!(results
            .iter()
            .any(|item| item.kind == IntentKind::Commitment));
    }

    #[test]
    fn search_intents_filters_by_kind_and_owner() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n  - kind: commitment\n    what: Share revised pricing model\n    who: sarah\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: Some(IntentKind::Commitment),
            owner: Some("sarah".into()),
            recorded_by: None,
        };

        let overlay_db = dir.path().join("overlays.db");
        let results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            "",
            &filters,
            &overlay_db,
        )
        .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].kind, IntentKind::Commitment);
        assert_eq!(results[0].who.as_deref(), Some("sarah"));
    }

    #[test]
    fn search_intents_filters_owner_through_speaker_overlay() {
        let dir = TempDir::new().unwrap();
        let meeting = dir.path().join("2026-03-17-test.md");
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nspeaker_map:\n  - speaker_label: SPEAKER_0\n    name: Unknown Speaker\n    confidence: medium\n    source: llm\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: SPEAKER_0\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\n[SPEAKER_0 0:00] I'll send pricing.\n",
        );

        let overlay_db = dir.path().join("overlays.db");
        crate::overlays::write_speaker_confirmation_at(
            &overlay_db,
            &meeting,
            "SPEAKER_0",
            "Alex Kim",
            Some("Unknown Speaker"),
            Some("test owner resolution"),
        )
        .unwrap();

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: Some(IntentKind::ActionItem),
            owner: Some("alex".into()),
            recorded_by: None,
        };

        let results = search_intents_at("", &config, &filters, &overlay_db).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].who.as_deref(), Some("Alex Kim"));
        assert_eq!(results[0].who_original.as_deref(), Some("SPEAKER_0"));
        assert_eq!(
            results[0].who_provenance.as_deref(),
            Some("speaker overlay")
        );
    }

    #[test]
    fn search_intents_filter_by_recorded_by() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: []\nrecorded_by: Mat Silver\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Send pricing doc\n    who: mat\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("mat".into()),
        };
        let non_matching_filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: Some("sarah".into()),
        };

        let matching_results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            "",
            &matching_filters,
            &dir.path().join("overlays.db"),
        )
        .unwrap();
        let non_matching_results = process_intent_file(
            &dir.path().join("2026-03-17-test.md"),
            "",
            &non_matching_filters,
            &dir.path().join("overlays.db"),
        )
        .unwrap();

        assert_eq!(matching_results.len(), 1);
        assert!(non_matching_results.is_empty());
    }

    #[test]
    fn consistency_report_flags_conflicts_and_stale_commitments() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: Pricing Decision\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at annual billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Send pricing doc\n    who: case\n    status: open\n    by_date: March 8\n---\n\n## Transcript\n\nPricing discussion.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: Pricing Revisit\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nPricing changed.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-c.md",
            "---\ntitle: Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nFollow-up.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-d.md",
            "---\ntitle: Another Follow-up\ntype: meeting\ndate: 2026-03-25T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nAnother follow-up.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert_eq!(report.decision_conflicts[0].topic, "pricing");
        assert_eq!(report.decision_conflicts[0].previous.len(), 1);
        assert_eq!(report.stale_commitments.len(), 1);
        assert_eq!(
            report.stale_commitments[0].entry.who.as_deref(),
            Some("case")
        );
        assert!(report.stale_commitments[0].meetings_since >= 3);
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("days old")));
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("newer meetings since")));
        assert!(report.stale_commitments[0]
            .reasons
            .iter()
            .any(|reason| reason.contains("still open with due date March 8")));
        assert_eq!(
            report.stale_commitments[0]
                .latest_follow_up
                .as_ref()
                .map(|meeting| meeting.title.as_str()),
            Some("Another Follow-up")
        );
    }

    #[test]
    fn consistency_report_resolves_stale_owner_through_speaker_overlay() {
        let dir = TempDir::new().unwrap();
        let meeting = dir.path().join("2020-03-01-a.md");
        create_test_file(
            dir.path(),
            "2020-03-01-a.md",
            "---\ntitle: Follow-up Owner\ntype: meeting\ndate: 2020-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nspeaker_map:\n  - speaker_label: SPEAKER_0\n    name: Unknown Speaker\n    confidence: medium\n    source: llm\nintents:\n  - kind: commitment\n    what: Send the rollout memo\n    who: SPEAKER_0\n    status: open\n    by_date: March 8\n---\n\n## Transcript\n\n[SPEAKER_0 0:00] I'll send it.\n",
        );

        let overlay_db = dir.path().join("overlays.db");
        crate::overlays::write_speaker_confirmation_at(
            &overlay_db,
            &meeting,
            "SPEAKER_0",
            "Alex Kim",
            Some("Unknown Speaker"),
            Some("test consistency owner resolution"),
        )
        .unwrap();

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };
        let report = consistency_report_at(&config, Some("alex"), 7, &overlay_db).unwrap();

        assert_eq!(report.stale_commitments.len(), 1);
        let entry = &report.stale_commitments[0].entry;
        assert_eq!(entry.who.as_deref(), Some("Alex Kim"));
        assert_eq!(entry.who_original.as_deref(), Some("SPEAKER_0"));
        assert_eq!(entry.who_provenance.as_deref(), Some("speaker overlay"));
    }

    #[test]
    fn consistency_report_marks_conflict_resolved_when_supersedes_is_set() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-02-28-a.md",
            "---\ntitle: Pricing Strategy\ntype: meeting\ndate: 2026-02-28T10:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing for consultants\n    topic: pricing\n    authority: high\nintents: []\n---\n\n## Transcript\n\nDecision A.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-b.md",
            "---\ntitle: Pricing Reversal\ntype: meeting\ndate: 2026-03-25T10:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Revert to annual-only billing across all segments\n    topic: pricing\n    authority: high\n    supersedes: \"2026-02-28 monthly billing decision\"\nintents: []\n---\n\n## Transcript\n\nDecision B reverses A.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        let conflict = &report.decision_conflicts[0];
        assert_eq!(conflict.topic, "pricing");
        assert!(conflict.resolution.is_some());
        assert!(conflict.resolution.as_ref().unwrap().contains("2026-02-28"));
        assert_eq!(conflict.latest.authority.as_deref(), Some("high"));
        assert_eq!(conflict.previous[0].authority.as_deref(), Some("high"));
    }

    #[test]
    fn consistency_report_leaves_resolution_none_without_supersedes() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay on annual billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nB without supersedes.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert!(report.decision_conflicts[0].resolution.is_none());
        assert!(report.decision_conflicts[0].latest.authority.is_none());
    }

    #[test]
    fn consistency_report_does_not_mark_resolution_when_other_conflicts_remain() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay annual only\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nB.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-25-c.md",
            "---\ntitle: C\ntype: meeting\ndate: 2026-03-25T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Test monthly billing for consultants only\n    topic: pricing\n    supersedes: \"2026-03-01 monthly billing decision\"\nintents: []\n---\n\n## Transcript\n\nC.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        let conflict = &report.decision_conflicts[0];
        assert_eq!(conflict.previous.len(), 2);
        assert!(conflict.resolution.is_none());
    }

    #[test]
    fn consistency_report_requires_supersedes_to_reference_the_prior_decision() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: A\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch monthly billing\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nA.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: B\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Stay annual only\n    topic: pricing\n    supersedes: \"some old plan\"\nintents: []\n---\n\n## Transcript\n\nB.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert_eq!(report.decision_conflicts.len(), 1);
        assert!(report.decision_conflicts[0].resolution.is_none());
    }

    #[test]
    fn consistency_report_ignores_near_duplicate_decisions() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-01-a.md",
            "---\ntitle: Pricing Decision\ntype: meeting\ndate: 2026-03-01T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at 399 per month\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nPricing discussion.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-12-b.md",
            "---\ntitle: Pricing Follow-up\ntype: meeting\ndate: 2026-03-12T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at 399 per month.\n    topic: pricing strategy\nintents: []\n---\n\n## Transcript\n\nPricing repeated.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let report = consistency_report(&config, None, 7).unwrap();
        assert!(report.decision_conflicts.is_empty());
    }

    #[test]
    fn person_profile_aggregates_recent_meetings_topics_and_open_intents() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Share revised pricing model\n    who: Alex\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-b.md",
            "---\ntitle: Onboarding Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: []\naction_items: []\ndecisions: []\nintents:\n  - kind: action-item\n    what: Review onboarding copy\n    who: Alex\n    status: open\n    by_date: Friday\n---\n\n## Transcript\n\nWe discussed onboarding.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let profile = person_profile(&config, "alex").unwrap();
        assert_eq!(profile.name, "alex");
        assert_eq!(profile.recent_meetings.len(), 2);
        assert_eq!(profile.open_intents.len(), 2);
        assert_eq!(profile.recent_decisions.len(), 1);
        assert!(profile
            .top_topics
            .iter()
            .any(|topic| topic.topic == "pricing"));
    }

    #[test]
    fn person_profile_matches_linked_people_entities() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: []\npeople: [Alex Chen]\nentities:\n  people:\n    - slug: sarah-chen\n      label: Alex Chen\n      aliases: [sarah]\n  projects:\n    - slug: pricing-review\n      label: Pricing Review\n      aliases: [pricing]\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents: []\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let profile = person_profile(&config, "sarah").unwrap();
        assert_eq!(profile.recent_meetings.len(), 1);
        assert_eq!(profile.recent_meetings[0].title, "Pricing Review");
    }

    #[test]
    fn cross_meeting_research_collects_decisions_intents_and_meetings() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-a.md",
            "---\ntitle: Pricing Review\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 42m\nstatus: complete\ntags: []\nattendees: [Alex]\npeople: [Alex]\nentities:\n  people:\n    - slug: sarah\n      label: Alex\n      aliases: []\n  projects:\n    - slug: pricing\n      label: Pricing\n      aliases: []\ncontext: pricing review\naction_items: []\ndecisions:\n  - text: Launch pricing at monthly billing per month\n    topic: pricing\nintents:\n  - kind: commitment\n    what: Share revised pricing model\n    who: Alex\n    status: open\n    by_date: Tuesday\n---\n\n## Transcript\n\nWe discussed pricing.\n",
        );
        create_test_file(
            dir.path(),
            "2026-03-20-b.md",
            "---\ntitle: Onboarding Follow-up\ntype: meeting\ndate: 2026-03-20T12:00:00-07:00\nduration: 30m\nstatus: complete\ntags: []\nattendees: []\npeople: []\naction_items: []\ndecisions: []\nintents: []\n---\n\n## Transcript\n\nWe discussed onboarding.\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let filters = SearchFilters {
            content_type: None,
            since: None,
            attendee: None,
            intent_kind: None,
            owner: None,
            recorded_by: None,
        };
        let report = cross_meeting_research("pricing", &config, &filters).unwrap();

        assert_eq!(report.related_decisions.len(), 1);
        assert_eq!(report.related_open_intents.len(), 1);
        assert_eq!(report.recent_meetings.len(), 1);
        assert_eq!(report.recent_meetings[0].title, "Pricing Review");
        assert!(report
            .related_topics
            .iter()
            .any(|topic| topic.topic == "pricing"));
    }

    #[test]
    fn find_open_actions_parses_frontmatter() {
        let dir = TempDir::new().unwrap();
        create_test_file(
            dir.path(),
            "2026-03-17-test.md",
            "---\ntitle: Test\ntype: meeting\ndate: 2026-03-17T12:00:00-07:00\nduration: 5m\nstatus: complete\naction_items:\n  - assignee: mat\n    task: Send doc\n    status: open\n  - assignee: alex\n    task: Review PR\n    status: done\ndecisions: []\nintents: []\n---\n\nTranscript\n",
        );

        let config = Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        };

        let results = find_open_actions(&config, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].assignee, "mat");
        assert_eq!(results[0].task, "Send doc");

        // Filter by assignee
        let filtered = find_open_actions(&config, Some("nobody")).unwrap();
        assert!(filtered.is_empty());
    }
}
