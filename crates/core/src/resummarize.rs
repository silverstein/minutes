//! Re-run the AI pass on an existing meeting/memo artifact (#523).
//!
//! The pipeline's summarization is one self-contained stage
//! ([`crate::summarize::summarize_with_template`]) plus purely local
//! derivations (action items, decisions, intents, entity links). This module
//! re-runs exactly that stage against the *current* transcript text of an
//! edited artifact and splices the regenerated AI-owned content back in,
//! under a strict safety contract:
//!
//! - **Never destroy on failure.** Engine `none`, provider errors, and empty
//!   summaries are hard no-write failures; the file is untouched.
//! - **Concurrent-edit guard.** The file's full content is captured before
//!   inference and re-compared immediately before the write; any change
//!   aborts with [`ResummarizeError::ConcurrentEdit`].
//! - **Splice, don't rewrite.** Only the AI-owned body sections and derived
//!   frontmatter fields change. `## Notes`, `## Transcript`, `speaker_map`,
//!   and capture/consent metadata are preserved byte-for-byte or
//!   field-for-field.
//! - **Status-preserving merge.** Action items and decisions carry
//!   user-curated state forward by exact normalized identity; everything
//!   ambiguous is surfaced in the report, never silently resolved.
//!
//! See [`resummarize_meeting`] for the frontmatter field contract.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::Config;
use crate::error::ResummarizeError;
use crate::markdown::{
    self, ActionItem, CapturePolicy, ContentType, Decision, Frontmatter, Intent, OutputStatus,
    SummarizationHealth,
};
use crate::summarize::{self, Summary};
use crate::template::{Template, TemplateResolver};

/// The AI-owned H2 sections that a resummarize run replaces. Everything else
/// in the body is preserved byte-for-byte.
pub const AI_SECTIONS: [&str; 5] = [
    "Summary",
    "Decisions",
    "Action Items",
    "Open Questions",
    "Commitments",
];

/// Options for a resummarize run.
#[derive(Debug, Clone, Default)]
pub struct ResummarizeOptions {
    /// Write the result back. `false` is preview mode — **the model is still
    /// invoked** (cost/privacy: on cloud engines the transcript and notes
    /// leave the machine), only the write is skipped.
    pub apply: bool,
    /// Explicit template slug override. `None` uses the template recorded in
    /// the artifact's frontmatter; if that recorded template no longer
    /// resolves, the run fails visibly rather than silently switching shape.
    pub template_override: Option<String>,
}

/// How a previous action item / decision fared in the merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeDisposition {
    /// Exact-identity match; user-owned fields carried onto the new item.
    Carried,
    /// Exact-identity match, but a user-visible field conflicted; the old
    /// value won and the conflict is reported here.
    CarriedWithConflict(String),
    /// No match in the regenerated set, but the old item holds user-curated
    /// state — it was kept rather than destroyed.
    KeptUnmatched,
    /// No match and no user-curated state; the old item was dropped in favor
    /// of the regenerated set.
    Dropped,
    /// The normalized identity was not unique (on either side); no automatic
    /// merge was attempted. Old items with user state were kept.
    Ambiguous,
}

/// One merge decision, reported so a preview can surface every case that was
/// not a clean carry-forward.
#[derive(Debug, Clone)]
pub struct MergeNote {
    /// `action_item` or `decision`.
    pub kind: &'static str,
    /// The previous item's display text.
    pub previous: String,
    /// What happened to it.
    pub disposition: MergeDisposition,
}

/// Result of one resummarize run. Returned for both preview and apply.
#[derive(Debug)]
pub struct ResummarizeReport {
    /// The artifact operated on.
    pub path: PathBuf,
    /// Whether the file was rewritten (`false` = preview; the model still ran).
    pub applied: bool,
    /// The timestamped backup written by this run, present only after a
    /// successful apply.
    pub backup: Option<PathBuf>,
    /// Engine string the run used (`config.summarization.engine` after any
    /// caller-side override; resolution itself happens inside the summarizer —
    /// the same choke point the pipeline uses).
    pub engine: String,
    /// Model hint recorded for the run.
    pub model: String,
    /// Template slug applied, if any.
    pub template: Option<String>,
    /// The regenerated AI-owned body content (what replaces `## Summary`
    /// through `## Commitments`).
    pub new_ai_body: String,
    /// Which AI-owned sections existed before and were replaced (by name).
    pub sections_replaced: Vec<String>,
    /// Merge outcomes that were not clean carries (conflicts, keeps, drops,
    /// ambiguities) — the preview must surface these.
    pub merge_notes: Vec<MergeNote>,
    /// Merged structured action items (what frontmatter gets on apply).
    pub action_items: Vec<ActionItem>,
    /// Merged structured decisions.
    pub decisions: Vec<Decision>,
    /// Wall-clock of the summarize stage in milliseconds.
    pub duration_ms: u64,
}

/// Normalized identity for exact matching: lowercase, whitespace collapsed,
/// and *terminal* punctuation stripped from each word. Case, spacing, and
/// trailing `.,;:!?` / quote / bracket differences do not break a match; any
/// word change does. Intra-token symbols stay significant on purpose —
/// `Migrate C++ service` and `Migrate C service` must NOT collide (a wrong
/// merge is worse than none).
fn identity_key(text: &str) -> String {
    let trim_set = |c: char| {
        matches!(
            c,
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '`' | '(' | ')' | '[' | ']'
        )
    };
    text.split_whitespace()
        .map(|word| word.trim_matches(trim_set).to_lowercase())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn normalized_scaffold_label(line: &str) -> &str {
    line.trim()
        .trim_start_matches(|c: char| matches!(c, '#' | '*' | '-' | '_' | '>') || c.is_whitespace())
        .trim_end_matches(|c: char| matches!(c, ':' | '*') || c.is_whitespace())
}

fn is_known_scaffold_header(line: &str) -> bool {
    matches!(
        line.to_ascii_uppercase().as_str(),
        "SUMMARY"
            | "KEY POINTS"
            | "DECISIONS"
            | "ACTION ITEMS"
            | "OPEN QUESTIONS"
            | "COMMITMENTS"
            | "PARTICIPANTS"
    )
}

/// Whether a line is empty scaffolding or a decorated known summary header.
fn is_scaffold_line(line: &str) -> bool {
    let label = normalized_scaffold_label(line);
    label.is_empty() || is_known_scaffold_header(label)
}

fn summary_has_no_structured_content(summary: &Summary) -> bool {
    summary.key_points.is_empty()
        && summary.decisions.is_empty()
        && summary.action_items.is_empty()
        && summary.open_questions.is_empty()
        && summary.commitments.is_empty()
}

/// Is this summary semantically empty — nothing worth splicing?
///
/// When every structured list is empty, the response parser dumps the raw
/// model output into `text`, so a degenerate response of bare section
/// headers (including markdown-decorated forms such as `## KEY POINTS:`)
/// would otherwise pass a plain is-empty check and `--apply` would replace a
/// good summary with scaffolding. Any real prose line counts as content.
fn summary_is_semantically_empty(summary: &Summary) -> bool {
    if !summary_has_no_structured_content(summary) {
        return false;
    }
    summary.text.lines().all(is_scaffold_line)
}

/// Detect the conservative unstructured-output regression heuristic.
///
/// It rejects only when the old artifact has action items or decisions, the
/// new summary parsed no structured content, and its raw text contains no
/// recognizable section header. It intentionally does not reject prose-only
/// summaries from artifacts that had no structured items.
fn summary_is_unstructured_regression(old: &Frontmatter, summary: &Summary) -> bool {
    (!old.action_items.is_empty() || !old.decisions.is_empty())
        && summary_has_no_structured_content(summary)
        && !summary
            .text
            .lines()
            .any(|line| is_known_scaffold_header(normalized_scaffold_label(line)))
}

/// Does this action item carry user-curated state worth preserving?
///
/// Deliberately narrow: `status != "open"` is the only signal that is
/// necessarily user-set (the extractor always emits `open`); a due date is
/// kept as a state signal because losing one silently is costly. Assignee is
/// NOT counted — the extractor derives it from `@name:` prefixes on every
/// fresh pass, so treating it as user state would make items effectively
/// undroppable. Every drop is surfaced in the report either way.
fn action_item_has_user_state(item: &ActionItem) -> bool {
    item.status != "open" || item.due.is_some()
}

/// Does this decision carry user-curated state worth preserving?
fn decision_has_user_state(decision: &Decision) -> bool {
    decision.authority.is_some() || decision.supersedes.is_some()
}

fn count_keys<'a, I: Iterator<Item = &'a str>>(
    texts: I,
) -> std::collections::HashMap<String, usize> {
    let mut counts = std::collections::HashMap::new();
    for t in texts {
        *counts.entry(identity_key(t)).or_insert(0) += 1;
    }
    counts
}

/// Merge regenerated action items with the previous set, carrying user-owned
/// state forward by **exact normalized identity** (never fuzzy — a wrong
/// merge is worse than none).
///
/// Rules:
/// - identity match (unique on both sides): new item text wins; `status` and
///   `due` carry from the old item (never downgrade `done` → `open`); a
///   conflicting old assignee wins and is reported.
/// - old item unmatched **with** user state: kept (appended after the
///   regenerated items) and reported — user-curated state is never destroyed.
/// - old item unmatched, pristine: dropped (reported) — regenerable content
///   follows the new pass; that is the feature's purpose.
/// - identity not unique on either side: no automatic merge for that
///   identity; old items with user state are kept, and the case is reported
///   as ambiguous for explicit resolution.
pub fn merge_action_items(
    old: &[ActionItem],
    new: &[ActionItem],
) -> (Vec<ActionItem>, Vec<MergeNote>) {
    let old_counts = count_keys(old.iter().map(|i| i.task.as_str()));
    let new_counts = count_keys(new.iter().map(|i| i.task.as_str()));
    let mut notes = Vec::new();

    let mut merged: Vec<ActionItem> = Vec::with_capacity(new.len());
    for new_item in new {
        let key = identity_key(&new_item.task);
        let unique = old_counts.get(&key).copied().unwrap_or(0) <= 1
            && new_counts.get(&key).copied() == Some(1);
        let old_match = unique
            .then(|| old.iter().find(|o| identity_key(&o.task) == key))
            .flatten();
        match old_match {
            Some(old_item) => {
                let mut item = new_item.clone();
                let mut conflicts: Vec<String> = Vec::new();
                if old_item.status != "open" {
                    item.status = old_item.status.clone();
                }
                if old_item.due.is_some() {
                    if new_item.due.is_some() && new_item.due != old_item.due {
                        conflicts.push(format!(
                            "due kept as {:?} (regenerated pass said {:?})",
                            old_item.due, new_item.due
                        ));
                    }
                    item.due = old_item.due.clone();
                }
                if old_item.assignee != "unassigned" && old_item.assignee != new_item.assignee {
                    if new_item.assignee != "unassigned" {
                        conflicts.push(format!(
                            "assignee kept as '{}' (regenerated pass said '{}')",
                            old_item.assignee, new_item.assignee
                        ));
                    }
                    item.assignee = old_item.assignee.clone();
                }
                notes.push(MergeNote {
                    kind: "action_item",
                    previous: old_item.task.clone(),
                    disposition: if conflicts.is_empty() {
                        MergeDisposition::Carried
                    } else {
                        MergeDisposition::CarriedWithConflict(conflicts.join("; "))
                    },
                });
                merged.push(item);
            }
            None => merged.push(new_item.clone()),
        }
    }

    for old_item in old {
        let key = identity_key(&old_item.task);
        let ambiguous = old_counts.get(&key).copied().unwrap_or(0) > 1
            || new_counts.get(&key).copied().unwrap_or(0) > 1;
        let matched = !ambiguous && new_counts.contains_key(&key);
        if matched {
            continue; // handled above
        }
        if action_item_has_user_state(old_item) {
            merged.push(old_item.clone());
            notes.push(MergeNote {
                kind: "action_item",
                previous: old_item.task.clone(),
                disposition: if ambiguous {
                    MergeDisposition::Ambiguous
                } else {
                    MergeDisposition::KeptUnmatched
                },
            });
        } else {
            notes.push(MergeNote {
                kind: "action_item",
                previous: old_item.task.clone(),
                disposition: if ambiguous {
                    MergeDisposition::Ambiguous
                } else {
                    MergeDisposition::Dropped
                },
            });
        }
    }

    (merged, notes)
}

/// Merge regenerated decisions with the previous set. Same doctrine as
/// [`merge_action_items`]: exact normalized identity, carry `authority` /
/// `supersedes` / a user-set `topic`, keep unmatched decisions that hold
/// user-curated v2 fields, drop pristine unmatched ones, report everything.
pub fn merge_decisions(old: &[Decision], new: &[Decision]) -> (Vec<Decision>, Vec<MergeNote>) {
    let old_counts = count_keys(old.iter().map(|d| d.text.as_str()));
    let new_counts = count_keys(new.iter().map(|d| d.text.as_str()));
    let mut notes = Vec::new();

    let mut merged: Vec<Decision> = Vec::with_capacity(new.len());
    for new_decision in new {
        let key = identity_key(&new_decision.text);
        let unique = old_counts.get(&key).copied().unwrap_or(0) <= 1
            && new_counts.get(&key).copied() == Some(1);
        let old_match = unique
            .then(|| old.iter().find(|o| identity_key(&o.text) == key))
            .flatten();
        match old_match {
            Some(old_decision) => {
                let mut decision = new_decision.clone();
                decision.authority = old_decision.authority.clone();
                decision.supersedes = old_decision.supersedes.clone();
                if old_decision.topic.is_some() {
                    decision.topic = old_decision.topic.clone();
                }
                notes.push(MergeNote {
                    kind: "decision",
                    previous: old_decision.text.clone(),
                    disposition: MergeDisposition::Carried,
                });
                merged.push(decision);
            }
            None => merged.push(new_decision.clone()),
        }
    }

    for old_decision in old {
        let key = identity_key(&old_decision.text);
        let ambiguous = old_counts.get(&key).copied().unwrap_or(0) > 1
            || new_counts.get(&key).copied().unwrap_or(0) > 1;
        let matched = !ambiguous && new_counts.contains_key(&key);
        if matched {
            continue;
        }
        if decision_has_user_state(old_decision) {
            merged.push(old_decision.clone());
            notes.push(MergeNote {
                kind: "decision",
                previous: old_decision.text.clone(),
                disposition: if ambiguous {
                    MergeDisposition::Ambiguous
                } else {
                    MergeDisposition::KeptUnmatched
                },
            });
        } else {
            notes.push(MergeNote {
                kind: "decision",
                previous: old_decision.text.clone(),
                disposition: if ambiguous {
                    MergeDisposition::Ambiguous
                } else {
                    MergeDisposition::Dropped
                },
            });
        }
    }

    (merged, notes)
}

/// Render an action item back to its display form (`@assignee: task`), the
/// same shape [`crate::pipeline::extract_action_items`] parses.
fn render_action_item(item: &ActionItem) -> String {
    if item.assignee == "unassigned" {
        item.task.clone()
    } else {
        format!("@{}: {}", item.assignee, item.task)
    }
}

/// Render the AI-owned body block (`## Summary` content through
/// `## Commitments`) from the regenerated summary and the **merged**
/// structured state, so body checkboxes and frontmatter can never diverge.
///
/// Shape matches [`crate::summarize::format_summary`], except action items
/// and decisions come from the merged items (with real checkbox state)
/// rather than the raw summary strings.
pub fn render_ai_body(
    summary: &Summary,
    merged_actions: &[ActionItem],
    merged_decisions: &[Decision],
) -> String {
    let mut output = String::new();

    if !summary.key_points.is_empty() {
        for point in &summary.key_points {
            output.push_str(&format!("- {}\n", point));
        }
    } else if !summary.text.is_empty() {
        output.push_str(&summary.text);
        output.push('\n');
    }

    if !merged_decisions.is_empty() {
        output.push_str("\n## Decisions\n\n");
        for decision in merged_decisions {
            output.push_str(&format!("- [x] {}\n", decision.text));
        }
    }

    if !merged_actions.is_empty() {
        output.push_str("\n## Action Items\n\n");
        for item in merged_actions {
            let marker = if item.status == "done" { "x" } else { " " };
            output.push_str(&format!("- [{}] {}\n", marker, render_action_item(item)));
        }
    }

    if !summary.open_questions.is_empty() {
        output.push_str("\n## Open Questions\n\n");
        for question in &summary.open_questions {
            output.push_str(&format!("- {}\n", question));
        }
    }

    if !summary.commitments.is_empty() {
        output.push_str("\n## Commitments\n\n");
        for commitment in &summary.commitments {
            output.push_str(&format!("- {}\n", commitment));
        }
    }

    output
}

/// Rebuild intents from the merged structured state plus the regenerated
/// open questions / commitments — the same composition as the pipeline's
/// intent extraction, but sourced from merged items so intent status follows
/// carried action-item status.
fn rebuild_intents(
    summary: &Summary,
    merged_actions: &[ActionItem],
    merged_decisions: &[Decision],
) -> Vec<Intent> {
    // Build a summary whose action/decision lists are empty so the pipeline
    // extractor only contributes open questions and commitments; actions and
    // decisions are composed from the merged structured items directly.
    let mut tail_only = summary.clone();
    tail_only.action_items = vec![];
    tail_only.decisions = vec![];
    let mut intents: Vec<Intent> = Vec::new();
    for item in merged_actions {
        intents.push(Intent {
            kind: markdown::IntentKind::ActionItem,
            what: item.task.clone(),
            who: (item.assignee != "unassigned").then(|| item.assignee.clone()),
            status: item.status.clone(),
            by_date: item.due.clone(),
        });
    }
    for decision in merged_decisions {
        intents.push(Intent {
            kind: markdown::IntentKind::Decision,
            what: decision.text.clone(),
            who: None,
            status: "decided".into(),
            by_date: None,
        });
    }
    intents.extend(crate::pipeline::extract_intents(&tail_only));
    intents
}

/// Validate that this artifact is something v1 resummarize may operate on.
fn validate_artifact(fm: &Frontmatter, path: &Path) -> Result<(), ResummarizeError> {
    if fm.r#type == ContentType::Dictation {
        return Err(ResummarizeError::Unsupported(format!(
            "{} is a dictation artifact; resummarize supports meetings and memos",
            path.display()
        )));
    }
    if fm.status == Some(OutputStatus::NoSpeech) {
        return Err(ResummarizeError::Unsupported(format!(
            "{} is a no-speech artifact; there is no transcript to summarize",
            path.display()
        )));
    }
    if fm.capture == Some(CapturePolicy::None) {
        return Err(ResummarizeError::Unsupported(format!(
            "{} has capture: none — its body is a human-authored debrief, not a transcript",
            path.display()
        )));
    }
    Ok(())
}

/// Resolve the template for this run: explicit override first, then the slug
/// recorded in frontmatter. A recorded template that no longer resolves is a
/// visible failure (silently switching to the default template would change
/// the output shape).
fn resolve_template(
    fm: &Frontmatter,
    opts: &ResummarizeOptions,
) -> Result<Option<Template>, ResummarizeError> {
    let slug = opts.template_override.as_deref().or(fm.template.as_deref());
    match slug {
        None => Ok(None),
        Some(slug) => TemplateResolver::new()
            .resolve(slug)
            .map(Some)
            .map_err(|e| ResummarizeError::TemplateUnavailable {
                slug: slug.to_string(),
                reason: e.to_string(),
            }),
    }
}

/// Remove summarize-step warnings that a successful re-run has just
/// invalidated, and re-derive the overall status from what remains. Warnings
/// from other steps (capture, diarization) are untouched.
fn refresh_status_after_success(fm: &mut Frontmatter) {
    fm.processing_warnings.retain(|w| w.step != "summarize");
    match fm.status {
        Some(OutputStatus::TranscriptOnly) => fm.status = Some(OutputStatus::Complete),
        Some(OutputStatus::Degraded) if fm.processing_warnings.is_empty() => {
            fm.status = Some(OutputStatus::Complete)
        }
        _ => {}
    }
}

/// Splice the regenerated AI-owned block into the body, replacing every
/// existing AI-owned section and preserving all other bytes.
///
/// The new block is inserted where `## Summary` previously started, or, if
/// the document had no AI-owned sections at all (e.g. a `minutes import
/// text` artifact, #516), immediately before `## Notes` / `## Transcript` —
/// matching the canonical render order.
///
/// Fails closed on duplicate AI-owned headings ([`MarkdownError::AmbiguousSection`]).
pub fn splice_ai_sections(
    body: &str,
    new_ai_body: &str,
) -> Result<(String, Vec<String>), ResummarizeError> {
    // Resolve every AI-owned section strictly before touching anything.
    let mut ranges: Vec<(String, markdown::SectionRange)> = Vec::new();
    for name in AI_SECTIONS {
        if let Some(range) = markdown::find_unique_section(body, name)? {
            ranges.push((name.to_string(), range));
        }
    }

    let insert_at = match ranges.iter().find(|(name, _)| name == "Summary") {
        Some((_, summary_range)) => summary_range.heading_start,
        None => match ranges.first() {
            Some((_, first)) => first.heading_start,
            None => {
                // No AI sections yet: insert before Notes, else Transcript,
                // else append at the end.
                let notes = markdown::find_unique_section(body, "Notes")?;
                let transcript = markdown::find_unique_section(body, "Transcript")?;
                notes
                    .or(transcript)
                    .map(|r| r.heading_start)
                    .unwrap_or(body.len())
            }
        },
    };

    // Remove existing AI sections back-to-front so earlier offsets stay valid,
    // tracking how much removed content preceded the insertion point.
    let mut result = body.to_string();
    let mut removed_before_insert = 0usize;
    let mut sections_replaced: Vec<String> = ranges.iter().map(|(n, _)| n.clone()).collect();
    sections_replaced.sort();
    let mut sorted: Vec<&(String, markdown::SectionRange)> = ranges.iter().collect();
    sorted.sort_by_key(|(_, r)| std::cmp::Reverse(r.heading_start));
    for (_, range) in sorted {
        result.replace_range(range.heading_start..range.end, "");
        if range.heading_start < insert_at {
            removed_before_insert += range.end - range.heading_start;
        }
    }
    let insert_at = insert_at - removed_before_insert;

    let mut block = format!("## Summary\n\n{}", new_ai_body.trim_end_matches('\n'));
    block.push('\n');
    block.push('\n');
    // Keep exactly one blank line between the block and what follows.
    let after = &result[insert_at..];
    if after.is_empty() {
        block.truncate(block.trim_end_matches('\n').len());
        block.push('\n');
    }
    // Match the document's line endings so a CRLF artifact does not end up
    // with mixed endings (only the inserted block needs converting — every
    // preserved byte already carries its own ending).
    if body.contains("\r\n") {
        block = block.replace('\n', "\r\n");
    }
    result.insert_str(insert_at, &block);

    Ok((result, sections_replaced))
}

/// Re-run the AI pass on the artifact at `path`.
///
/// Engine selection: the caller controls the engine exactly as the pipeline
/// does — via `config.summarization.engine` (clone the config to override per
/// run). All resolution happens inside the summarizer, so resummarize can
/// never bypass engine policy (maintainer note M1 on #523).
///
/// ## Frontmatter field contract
///
/// | Fields | Treatment |
/// |---|---|
/// | `title`, `type`, `date`, `duration`, `source`, `tags`, `attendees`, `attendees_raw`, `calendar_event`, `device`, `captured_at`, `context`, `recorded_by`, `capture`, `sensitivity`, `debrief`, `consent`, `consent_notice`, `visibility`, `speaker_map`, `name_corrections`, `recording_health`, `speaker_mapping` | **preserved** untouched |
/// | `entities`, `people`, `intents` | **recomputed** from the merged state |
/// | `action_items`, `decisions` | **merge-derived** (exact-identity carry-forward, see [`merge_action_items`] / [`merge_decisions`]) |
/// | `template`, `summarization`, `status`, `processing_warnings` | **recorded**: template slug used, run health block, summarize-scoped warnings removed and status re-derived |
pub fn resummarize_meeting(
    path: &Path,
    config: &Config,
    opts: &ResummarizeOptions,
) -> Result<ResummarizeReport, ResummarizeError> {
    resummarize_meeting_with(path, config, opts, |transcript, notes, template| {
        summarize::summarize_with_template(
            transcript,
            notes,
            &[], // v1 is text-only: no screenshot re-feed
            config,
            template,
            None,
        )
    })
}

/// [`resummarize_meeting`] with an injectable summarize stage, so the
/// failure/concurrency/splice contract is testable without a live engine.
///
/// Crate-private on purpose: the public entry point is
/// [`resummarize_meeting`], whose summarizer is always
/// `summarize_with_template` — the same engine-resolution choke point the
/// pipeline uses. A public closure-taking variant would let callers bypass
/// that policy (#523 maintainer note M1).
pub(crate) fn resummarize_meeting_with<F>(
    path: &Path,
    config: &Config,
    opts: &ResummarizeOptions,
    run_summarize: F,
) -> Result<ResummarizeReport, ResummarizeError>
where
    F: FnOnce(&str, Option<&str>, Option<&Template>) -> Option<Summary>,
{
    let engine = config.summarization.engine.clone();
    if engine == "none" {
        return Err(ResummarizeError::SummarizeFailed {
            engine,
            reason: "disabled".into(),
        });
    }

    // Snapshot for the concurrent-edit guard: the feature's own premise is
    // that this file is open in an editor right now.
    let original = std::fs::read_to_string(path)?;
    let (fm_str, body) = markdown::split_frontmatter(&original);
    if fm_str.is_empty() {
        return Err(ResummarizeError::Unsupported(format!(
            "{} is not a minutes artifact (no YAML frontmatter)",
            path.display()
        )));
    }
    let fm: Frontmatter =
        serde_yaml::from_str(fm_str).map_err(|e| ResummarizeError::Frontmatter(e.to_string()))?;
    validate_artifact(&fm, path)?;

    // Inputs: current transcript (required) and user notes (optional).
    let transcript = markdown::find_unique_section(body, "Transcript")?
        .map(|r| markdown::section_text(body, r))
        .filter(|t| !t.trim().is_empty())
        .ok_or_else(|| {
            ResummarizeError::Unsupported(format!(
                "{} has no non-empty '## Transcript' section",
                path.display()
            ))
        })?;
    let notes = markdown::find_unique_section(body, "Notes")?
        .map(|r| markdown::section_text(body, r))
        .filter(|n| !n.trim().is_empty());

    let template = resolve_template(&fm, opts)?;
    let template_slug = template
        .as_ref()
        .map(|t| t.slug().to_string())
        .or_else(|| opts.template_override.clone());

    // The summarize stage — the only model invocation, preview included.
    let started = Instant::now();
    let summary =
        run_summarize(&transcript, notes.as_deref(), template.as_ref()).ok_or_else(|| {
            ResummarizeError::SummarizeFailed {
                engine: engine.clone(),
                reason: "provider_error".into(),
            }
        })?;
    let duration_ms = started.elapsed().as_millis() as u64;

    // Empty output is a hard failure: a splice from it would erase a good
    // summary block.
    if summary_is_semantically_empty(&summary) {
        return Err(ResummarizeError::SummarizeFailed {
            engine,
            reason: "empty_summary".into(),
        });
    }
    if summary_is_unstructured_regression(&fm, &summary) {
        return Err(ResummarizeError::SummarizeFailed {
            engine,
            reason: "unstructured_output".into(),
        });
    }
    let new_actions_raw = crate::pipeline::extract_action_items(&summary);
    let new_decisions_raw = crate::pipeline::extract_decisions(&summary);

    // Status-preserving merge (never fuzzy; conflicts surface in the report).
    let (merged_actions, mut merge_notes) = merge_action_items(&fm.action_items, &new_actions_raw);
    let (merged_decisions, decision_notes) = merge_decisions(&fm.decisions, &new_decisions_raw);
    merge_notes.extend(decision_notes);
    // Clean carries are internal detail; the report surfaces what needs eyes.
    merge_notes.retain(|n| n.disposition != MergeDisposition::Carried);

    let new_ai_body = render_ai_body(&summary, &merged_actions, &merged_decisions);
    let (new_body, sections_replaced) = splice_ai_sections(body, &new_ai_body)?;

    let model = summarize::summarization_model_hint(config, false);
    let mut report = ResummarizeReport {
        path: path.to_path_buf(),
        applied: false,
        backup: None,
        engine: engine.clone(),
        model: model.clone(),
        template: template_slug.clone(),
        new_ai_body,
        sections_replaced,
        merge_notes,
        action_items: merged_actions.clone(),
        decisions: merged_decisions.clone(),
        duration_ms,
    };

    if !opts.apply {
        return Ok(report);
    }

    // Derived frontmatter, recomputed from the merged state.
    let attendees = fm.normalized_attendees();
    let entities = crate::pipeline::build_entity_links(
        &fm.title,
        fm.context.as_deref(),
        &attendees,
        &merged_actions,
        &merged_decisions,
        &rebuild_intents(&summary, &merged_actions, &merged_decisions),
        &fm.tags,
        Some(&config.identity),
    );

    let mut new_fm = fm.clone();
    new_fm.action_items = merged_actions;
    new_fm.decisions = merged_decisions;
    new_fm.intents = rebuild_intents(&summary, &new_fm.action_items, &new_fm.decisions);
    new_fm.people = entities.people.iter().map(|e| e.label.clone()).collect();
    new_fm.entities = entities;
    new_fm.template = template_slug.clone();
    new_fm.summarization = Some(SummarizationHealth {
        status: "ok".into(),
        model,
        template: template_slug,
        duration_ms: Some(duration_ms),
        reason: None,
        last_run: Some(chrono::Local::now().to_rfc3339()),
    });
    refresh_status_after_success(&mut new_fm);

    let mut serialized = serde_yaml::to_string(&new_fm).map_err(|e| {
        ResummarizeError::Markdown(crate::error::MarkdownError::SerializationError(
            e.to_string(),
        ))
    })?;
    // Match the artifact's line endings (the spliced body already does).
    let crlf = original.contains("\r\n");
    if crlf {
        serialized = serialized.replace('\n', "\r\n");
    }
    let eol = if crlf { "\r\n" } else { "\n" };
    let new_content = format!(
        "---{eol}{serialized}---{eol}{eol}{}",
        new_body.trim_start_matches(['\r', '\n'])
    );

    // Concurrent-edit guard + timestamped backup + atomic swap, in that order
    // inside the helper: tmp is staged and validated first, the guard compare
    // runs immediately before the swap, and each successful run writes one
    // hidden sibling carrying the artifact's mode. The newest three backups
    // are retained, so a conflicted run never disturbs an earlier run's backup.
    let unix_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());
    let backup = timestamped_backup_path(path, unix_secs);
    match markdown::atomic_rewrite_preserving_mode_guarded(
        path,
        &new_content,
        Some(&original),
        Some(&backup),
    ) {
        Ok(()) => {}
        Err(crate::error::MarkdownError::ConcurrentModification) => {
            return Err(ResummarizeError::ConcurrentEdit);
        }
        Err(e) => return Err(e.into()),
    }
    report.applied = true;
    report.backup = Some(backup);
    prune_resummarize_backups(path, 3);
    Ok(report)
}

/// Return this run's hidden sibling backup path:
/// `.<filename>.pre-resummarize.<unix-secs>.bak`.
///
/// [`prune_resummarize_backups`] keeps only the newest backups per artifact.
pub fn timestamped_backup_path(path: &Path, unix_secs: u64) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact".into());
    path.with_file_name(format!(".{name}.pre-resummarize.{unix_secs}.bak"))
}

/// Best-effort retention for this artifact's timestamped resummarize backups.
///
/// Files matching only this artifact's backup scheme are ordered by descending
/// numeric timestamp (then name), and every entry beyond `keep` is deleted.
/// Directory-read and deletion failures are deliberately ignored.
pub fn prune_resummarize_backups(path: &Path, keep: usize) {
    let Some(parent) = path.parent() else {
        return;
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "artifact".into());
    let prefix = format!(".{name}.pre-resummarize.");
    let suffix = ".bak";
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };

    let mut backups = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let file_name = file_name.to_string_lossy();
        let Some(timestamp) = file_name
            .strip_prefix(&prefix)
            .and_then(|rest| rest.strip_suffix(suffix))
        else {
            continue;
        };
        if timestamp.is_empty() || !timestamp.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let Ok(timestamp) = timestamp.parse::<u64>() else {
            continue;
        };
        backups.push((timestamp, file_name.into_owned(), entry.path()));
    }

    backups.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
    for (_, _, backup) in backups.into_iter().skip(keep) {
        let _ = std::fs::remove_file(backup);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn item(assignee: &str, task: &str, due: Option<&str>, status: &str) -> ActionItem {
        ActionItem {
            assignee: assignee.into(),
            task: task.into(),
            due: due.map(String::from),
            status: status.into(),
        }
    }

    fn decision(text: &str, authority: Option<&str>, supersedes: Option<&str>) -> Decision {
        Decision {
            text: text.into(),
            topic: None,
            authority: authority.map(String::from),
            supersedes: supersedes.map(String::from),
        }
    }

    fn summary(
        key_points: &[&str],
        decisions: &[&str],
        action_items: &[&str],
        open_questions: &[&str],
        commitments: &[&str],
    ) -> Summary {
        Summary {
            text: String::new(),
            decisions: decisions.iter().map(|s| s.to_string()).collect(),
            action_items: action_items.iter().map(|s| s.to_string()).collect(),
            open_questions: open_questions.iter().map(|s| s.to_string()).collect(),
            commitments: commitments.iter().map(|s| s.to_string()).collect(),
            key_points: key_points.iter().map(|s| s.to_string()).collect(),
            participants: vec![],
        }
    }

    // ── identity_key ──────────────────────────────────────────────

    #[test]
    fn identity_key_normalizes_case_terminal_punctuation_whitespace() {
        assert_eq!(
            identity_key("Send  the pricing doc, by Friday!"),
            identity_key("send the Pricing doc by friday")
        );
        assert_ne!(identity_key("send doc"), identity_key("send docs"));
        // Intra-token symbols are significant: never-fuzzy means these are
        // DIFFERENT tasks (Codex review finding on #523).
        assert_ne!(
            identity_key("Migrate C++ service"),
            identity_key("Migrate C service")
        );
        assert_ne!(
            identity_key("pricing-doc review"),
            identity_key("pricing doc review")
        );
    }

    #[test]
    fn markdown_decorated_scaffolding_is_semantically_empty() {
        for text in ["## KEY POINTS:", "**Decisions:**", "> Summary"] {
            let mut scaffold = summary(&[], &[], &[], &[], &[]);
            scaffold.text = text.into();
            assert!(summary_is_semantically_empty(&scaffold), "{text}");
        }
    }

    #[test]
    fn real_prose_is_not_semantically_empty() {
        let mut prose = summary(&[], &[], &[], &[], &[]);
        prose.text = "The team agreed to ship next week.".into();
        assert!(!summary_is_semantically_empty(&prose));
    }

    // ── merge_action_items ────────────────────────────────────────

    #[test]
    fn merge_carries_done_status_and_due_on_exact_match() {
        let old = vec![item("Ryan", "Send pricing doc", Some("Friday"), "done")];
        let new = vec![item("Ryan", "Send pricing doc", None, "open")];
        let (merged, notes) = merge_action_items(&old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, "done");
        assert_eq!(merged[0].due.as_deref(), Some("Friday"));
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].disposition, MergeDisposition::Carried);
    }

    #[test]
    fn merge_matches_across_case_and_punctuation() {
        let old = vec![item("unassigned", "Update the roadmap!", None, "done")];
        let new = vec![item("unassigned", "update the roadmap", None, "open")];
        let (merged, _) = merge_action_items(&old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].status, "done");
        // The regenerated text is the display text going forward.
        assert_eq!(merged[0].task, "update the roadmap");
    }

    #[test]
    fn merge_reports_assignee_conflict_and_keeps_old() {
        let old = vec![item("Alice", "Ship the fix", None, "open")];
        let new = vec![item("Bob", "Ship the fix", None, "open")];
        let (merged, notes) = merge_action_items(&old, &new);
        assert_eq!(merged[0].assignee, "Alice");
        assert!(matches!(
            notes[0].disposition,
            MergeDisposition::CarriedWithConflict(_)
        ));
    }

    #[test]
    fn merge_keeps_unmatched_old_item_with_user_state() {
        let old = vec![item("Ryan", "Done long ago", None, "done")];
        let new = vec![item("unassigned", "Brand new task", None, "open")];
        let (merged, notes) = merge_action_items(&old, &new);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|i| i.task == "Done long ago"));
        assert!(notes
            .iter()
            .any(|n| n.disposition == MergeDisposition::KeptUnmatched));
    }

    #[test]
    fn merge_drops_unmatched_pristine_old_item_with_report() {
        let old = vec![item("unassigned", "No longer in transcript", None, "open")];
        let new = vec![item("unassigned", "Brand new task", None, "open")];
        let (merged, notes) = merge_action_items(&old, &new);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].task, "Brand new task");
        assert!(notes
            .iter()
            .any(|n| n.disposition == MergeDisposition::Dropped));
    }

    #[test]
    fn merge_never_auto_merges_ambiguous_identities() {
        // Same normalized identity twice in the new set: no carry may happen.
        let old = vec![item("Ryan", "Review the PR", Some("Monday"), "done")];
        let new = vec![
            item("unassigned", "Review the PR", None, "open"),
            item("unassigned", "Review the PR!", None, "open"),
        ];
        let (merged, notes) = merge_action_items(&old, &new);
        // Both new items pass through untouched...
        assert_eq!(
            merged.iter().filter(|i| i.status == "open").count(),
            2,
            "no new item may receive carried state on an ambiguous identity"
        );
        // ...and the old user-stateful item is kept and flagged ambiguous.
        assert!(merged.iter().any(|i| i.status == "done"));
        assert!(notes
            .iter()
            .any(|n| n.disposition == MergeDisposition::Ambiguous));
    }

    #[test]
    fn merge_passes_through_new_items_without_history() {
        let new = vec![item("unassigned", "Fresh", None, "open")];
        let (merged, notes) = merge_action_items(&[], &new);
        assert_eq!(merged.len(), 1);
        assert!(notes.is_empty());
    }

    // ── merge_decisions ───────────────────────────────────────────

    #[test]
    fn merge_decisions_carries_v2_fields() {
        let old = vec![decision("Use Postgres", Some("high"), Some("SQLite call"))];
        let new = vec![decision("use postgres", None, None)];
        let (merged, _) = merge_decisions(&old, &new);
        assert_eq!(merged[0].authority.as_deref(), Some("high"));
        assert_eq!(merged[0].supersedes.as_deref(), Some("SQLite call"));
    }

    #[test]
    fn merge_decisions_keeps_stateful_and_drops_pristine_unmatched() {
        let old = vec![
            decision("Keep me (authority)", Some("high"), None),
            decision("Drop me", None, None),
        ];
        let new = vec![decision("Something else", None, None)];
        let (merged, notes) = merge_decisions(&old, &new);
        assert_eq!(merged.len(), 2);
        assert!(merged.iter().any(|d| d.text == "Keep me (authority)"));
        assert!(!merged.iter().any(|d| d.text == "Drop me"));
        assert!(notes
            .iter()
            .any(|n| n.disposition == MergeDisposition::Dropped));
        assert!(notes
            .iter()
            .any(|n| n.disposition == MergeDisposition::KeptUnmatched));
    }

    // ── render_ai_body ────────────────────────────────────────────

    #[test]
    fn render_ai_body_reflects_merged_checkbox_state() {
        let s = summary(&["point one"], &[], &[], &["open q"], &["a commitment"]);
        let actions = vec![
            item("Ryan", "Done task", None, "done"),
            item("unassigned", "Open task", None, "open"),
        ];
        let decisions = vec![decision("The decision", None, None)];
        let body = render_ai_body(&s, &actions, &decisions);
        assert!(body.contains("- point one"));
        assert!(body.contains("- [x] @Ryan: Done task"));
        assert!(body.contains("- [ ] Open task"));
        assert!(body.contains("## Decisions\n\n- [x] The decision"));
        assert!(body.contains("## Open Questions\n\n- open q"));
        assert!(body.contains("## Commitments\n\n- a commitment"));
    }

    // ── splice_ai_sections ────────────────────────────────────────

    #[test]
    fn splice_replaces_ai_sections_and_preserves_rest() {
        let body = "## Summary\n\nold points\n\n## Decisions\n\n- [x] old d\n\n## Action Items\n\n- [ ] old a\n\n## Notes\n\n- my note\n\n## Transcript\n\n[SPEAKER_00 0:00] hi\n";
        let (out, replaced) = splice_ai_sections(body, "new points\n").unwrap();
        assert!(out.starts_with("## Summary\n\nnew points\n"));
        assert!(!out.contains("old points"));
        assert!(!out.contains("old d"));
        assert!(!out.contains("old a"));
        assert!(out.contains("## Notes\n\n- my note"));
        assert!(out.contains("## Transcript\n\n[SPEAKER_00 0:00] hi\n"));
        assert_eq!(replaced, vec!["Action Items", "Decisions", "Summary"]);
    }

    #[test]
    fn splice_inserts_before_notes_when_no_ai_sections_exist() {
        // The minutes import text shape (#516): transcript only, no AI pass yet.
        let body = "## Notes\n\n- n\n\n## Transcript\n\nimported text\n";
        let (out, replaced) = splice_ai_sections(body, "first summary\n").unwrap();
        assert!(out.starts_with("## Summary\n\nfirst summary\n"));
        assert!(out.contains("## Notes\n\n- n"));
        assert!(out.contains("## Transcript\n\nimported text\n"));
        assert!(replaced.is_empty());
    }

    #[test]
    fn splice_fails_closed_on_duplicate_ai_heading() {
        let body = "## Summary\n\na\n\n## Summary\n\nb\n\n## Transcript\n\nt\n";
        let err = splice_ai_sections(body, "new\n").unwrap_err();
        assert!(matches!(
            err,
            ResummarizeError::Markdown(crate::error::MarkdownError::AmbiguousSection { .. })
        ));
    }

    #[test]
    fn splice_preserves_custom_user_section_between_ai_sections() {
        let body = "## Summary\n\nold\n\n## My Custom Notes\n\nmine\n\n## Action Items\n\n- [ ] old\n\n## Transcript\n\nt\n";
        let (out, _) = splice_ai_sections(body, "new\n").unwrap();
        assert!(out.contains("## My Custom Notes\n\nmine"));
        assert!(!out.contains("- [ ] old"));
        assert!(out.contains("## Transcript\n\nt\n"));
    }

    // ── orchestrator guards (injected summarizer, tempdir) ────────

    const MEETING: &str = "---\ntitle: \"Weekly sync\"\ntype: meeting\ndate: 2026-07-20\nduration: \"10m\"\naction_items:\n  - assignee: \"Ryan\"\n    task: \"Send pricing doc\"\n    due: \"Friday\"\n    status: \"done\"\n---\n\n## Summary\n\n- old point\n\n## Action Items\n\n- [x] @Ryan: Send pricing doc\n\n## Notes\n\n- my note\n\n## Transcript\n\n[SPEAKER_00 0:00] edited hello\n";

    fn write_meeting(dir: &TempDir) -> PathBuf {
        let path = dir.path().join("meeting.md");
        fs::write(&path, MEETING).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        }
        path
    }

    fn test_config() -> Config {
        let mut config = Config::default();
        config.summarization.engine = "test".into();
        config
    }

    fn good_summary() -> Summary {
        summary(
            &["fresh point"],
            &["Ship it"],
            &["@Ryan: Send pricing doc", "New task"],
            &[],
            &[],
        )
    }

    #[test]
    fn engine_none_is_hard_failure_without_write() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let mut config = test_config();
        config.summarization.engine = "none".into();
        let err = resummarize_meeting_with(
            &path,
            &config,
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| Some(good_summary()),
        )
        .unwrap_err();
        assert!(
            matches!(err, ResummarizeError::SummarizeFailed { ref reason, .. } if reason == "disabled")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), MEETING);
    }

    #[test]
    fn provider_error_is_hard_failure_without_write() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| None,
        )
        .unwrap_err();
        assert!(
            matches!(err, ResummarizeError::SummarizeFailed { ref reason, .. } if reason == "provider_error")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), MEETING);
    }

    #[test]
    fn empty_summary_is_hard_failure_without_write() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| Some(summary(&[], &[], &[], &[], &[])),
        )
        .unwrap_err();
        assert!(
            matches!(err, ResummarizeError::SummarizeFailed { ref reason, .. } if reason == "empty_summary")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), MEETING);
    }

    #[test]
    fn header_only_model_response_is_empty_summary_failure() {
        // With no parsed sections, the response parser dumps raw output into
        // `text` — bare scaffolding must still count as empty (Codex finding).
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let mut scaffold = summary(&[], &[], &[], &[], &[]);
        scaffold.text = "SUMMARY:\n\nKEY POINTS:\n\nDECISIONS:\n".into();
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            move |_, _, _| Some(scaffold),
        )
        .unwrap_err();
        assert!(
            matches!(err, ResummarizeError::SummarizeFailed { ref reason, .. } if reason == "empty_summary")
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), MEETING);
    }

    #[test]
    fn unstructured_prose_regression_with_old_action_items_fails() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let mut prose = summary(&[], &[], &[], &[], &[]);
        prose.text = "The service was temporarily unavailable.".into();

        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            move |_, _, _| Some(prose),
        )
        .unwrap_err();

        assert!(
            matches!(err, ResummarizeError::SummarizeFailed { ref reason, .. } if reason == "unstructured_output")
        );
    }

    #[test]
    fn unstructured_prose_is_allowed_without_old_action_items_or_decisions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        let without_structured_items = MEETING.replace(
            "action_items:\n  - assignee: \"Ryan\"\n    task: \"Send pricing doc\"\n    due: \"Friday\"\n    status: \"done\"\n",
            "",
        );
        fs::write(&path, without_structured_items).unwrap();
        let mut prose = summary(&[], &[], &[], &[], &[]);
        prose.text = "The service was temporarily unavailable.".into();

        let report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            move |_, _, _| Some(prose),
        )
        .unwrap();

        assert!(!report.applied);
    }

    #[test]
    fn structured_output_is_not_an_unstructured_regression() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);

        let report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            |_, _, _| Some(good_summary()),
        )
        .unwrap();

        assert!(!report.applied);
    }

    #[test]
    fn timestamped_backup_path_uses_hidden_sibling_scheme() {
        let path = std::path::Path::new("/tmp/meeting.md");
        assert_eq!(
            timestamped_backup_path(path, 1_784_000_001),
            std::path::PathBuf::from("/tmp/.meeting.md.pre-resummarize.1784000001.bak")
        );
    }

    #[test]
    fn prune_resummarize_backups_keeps_three_newest_for_one_artifact() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        let other_path = dir.path().join("other.md");
        for timestamp in 1..=5 {
            fs::write(timestamped_backup_path(&path, timestamp), "backup").unwrap();
        }
        let other_backup = timestamped_backup_path(&other_path, 99);
        fs::write(&other_backup, "other backup").unwrap();

        prune_resummarize_backups(&path, 3);

        for timestamp in 1..=2 {
            assert!(!timestamped_backup_path(&path, timestamp).exists());
        }
        for timestamp in 3..=5 {
            assert!(timestamped_backup_path(&path, timestamp).exists());
        }
        assert!(other_backup.exists());
    }

    #[test]
    fn conflicted_run_preserves_prior_backup() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        // First run succeeds and leaves a backup of the original.
        let first_report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| Some(good_summary()),
        )
        .unwrap();
        let backup = first_report.backup.unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), MEETING);

        // Second run hits a concurrent edit — the good backup must survive.
        let path_clone = path.clone();
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            move |_, _, _| {
                let mutated = fs::read_to_string(&path_clone)
                    .unwrap()
                    .replace("fresh point", "user edit mid-flight");
                fs::write(&path_clone, mutated).unwrap();
                Some(good_summary())
            },
        )
        .unwrap_err();
        assert!(matches!(err, ResummarizeError::ConcurrentEdit));
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            MEETING,
            "a conflicted run must not clobber the prior run's backup"
        );
    }

    #[test]
    fn concurrent_edit_aborts_apply() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let path_clone = path.clone();
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            move |_, _, _| {
                // The user saves their editor mid-inference.
                let mutated = fs::read_to_string(&path_clone)
                    .unwrap()
                    .replace("edited hello", "edited again");
                fs::write(&path_clone, mutated).unwrap();
                Some(good_summary())
            },
        )
        .unwrap_err();
        assert!(matches!(err, ResummarizeError::ConcurrentEdit));
        // The user's mid-flight save is what survives.
        assert!(fs::read_to_string(&path).unwrap().contains("edited again"));
    }

    #[test]
    fn preview_never_writes_but_reports_everything() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(), // apply: false
            |transcript, notes, _| {
                assert!(transcript.contains("edited hello"));
                assert!(notes.unwrap().contains("my note"));
                Some(good_summary())
            },
        )
        .unwrap();
        assert!(!report.applied);
        assert_eq!(fs::read_to_string(&path).unwrap(), MEETING);
        assert!(report.new_ai_body.contains("fresh point"));
        // The carried done-item renders checked in the preview body.
        assert!(report.new_ai_body.contains("- [x] @Ryan: Send pricing doc"));
        assert!(report.action_items.iter().any(|i| i.status == "done"));
    }

    #[test]
    fn apply_rewrites_ai_content_and_preserves_user_content() {
        let dir = TempDir::new().unwrap();
        let path = write_meeting(&dir);
        let report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| Some(good_summary()),
        )
        .unwrap();
        assert!(report.applied);

        let content = fs::read_to_string(&path).unwrap();
        // AI content regenerated.
        assert!(content.contains("- fresh point"));
        assert!(content.contains("- [x] Ship it"));
        assert!(!content.contains("- old point"));
        // User content untouched.
        assert!(content.contains("## Notes\n\n- my note"));
        assert!(content.contains("[SPEAKER_00 0:00] edited hello"));
        // Carried state present in body and frontmatter.
        assert!(content.contains("- [x] @Ryan: Send pricing doc"));
        assert!(content.contains("status: done"));
        // Run record present.
        assert!(content.contains("summarization:"));
        assert!(content.contains("status: ok"));
        // Backup exists with the prior content — and the artifact's mode,
        // not the umask default (a 0644 copy of a 0600 transcript would be
        // a privacy leak).
        let backup = report.backup.as_ref().unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), MEETING);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            let backup_mode = fs::metadata(&backup).unwrap().permissions().mode() & 0o777;
            assert_eq!(backup_mode, 0o600);
        }
        // The result still parses and still resummarizes (idempotent shape).
        let report2 = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            |transcript, _, _| {
                assert!(transcript.contains("edited hello"));
                Some(good_summary())
            },
        )
        .unwrap();
        assert!(!report2.applied);
    }

    #[test]
    fn apply_to_crlf_artifact_keeps_uniform_line_endings() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        fs::write(&path, MEETING.replace('\n', "\r\n")).unwrap();
        let report = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions {
                apply: true,
                ..Default::default()
            },
            |_, _, _| Some(good_summary()),
        )
        .unwrap();
        assert!(report.applied);
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("fresh point"));
        // Every \n in the file must be part of a \r\n pair — no mixed endings.
        assert!(
            !content.replace("\r\n", "").contains('\n'),
            "bare \\n found in CRLF artifact after apply"
        );
    }

    #[test]
    fn rejects_dictation_nospeech_and_capture_none() {
        let dir = TempDir::new().unwrap();
        for fm in [
            "title: \"D\"\ntype: dictation\ndate: 2026-07-20\nduration: \"1m\"",
            "title: \"N\"\ntype: meeting\ndate: 2026-07-20\nduration: \"1m\"\nstatus: no-speech",
            "title: \"C\"\ntype: meeting\ndate: 2026-07-20\nduration: \"1m\"\ncapture: none",
        ] {
            let path = dir.path().join("artifact.md");
            fs::write(&path, format!("---\n{fm}\n---\n\n## Transcript\n\nt\n")).unwrap();
            let err = resummarize_meeting_with(
                &path,
                &test_config(),
                &ResummarizeOptions::default(),
                |_, _, _| Some(summary(&["x"], &[], &[], &[], &[])),
            )
            .unwrap_err();
            assert!(matches!(err, ResummarizeError::Unsupported(_)), "fm: {fm}");
        }
    }

    #[test]
    fn missing_transcript_section_is_unsupported() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        fs::write(
            &path,
            "---\ntitle: \"T\"\ntype: meeting\ndate: 2026-07-20\nduration: \"1m\"\n---\n\n## Summary\n\ns\n",
        )
        .unwrap();
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            |_, _, _| Some(summary(&["x"], &[], &[], &[], &[])),
        )
        .unwrap_err();
        assert!(matches!(err, ResummarizeError::Unsupported(_)));
    }

    #[test]
    fn recorded_template_that_no_longer_resolves_fails_visibly() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("meeting.md");
        fs::write(
            &path,
            "---\ntitle: \"T\"\ntype: meeting\ndate: 2026-07-20\nduration: \"1m\"\ntemplate: does-not-exist-xyz\n---\n\n## Transcript\n\nt\n",
        )
        .unwrap();
        let err = resummarize_meeting_with(
            &path,
            &test_config(),
            &ResummarizeOptions::default(),
            |_, _, _| Some(summary(&["x"], &[], &[], &[], &[])),
        )
        .unwrap_err();
        assert!(matches!(err, ResummarizeError::TemplateUnavailable { .. }));
    }
}
