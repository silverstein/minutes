//! Post-write refresh for external views of the canonical Markdown corpus.
//!
//! Minutes keeps Markdown as the source of truth. Search and graph answers are
//! deliberately rebuilt as bounded, process-private, policy-attested
//! projections when they are read, so a write cannot leave a durable local
//! cache stale or retain newly restricted meeting text. The remaining
//! user-configured external views still need an explicit refresh:
//!
//! | View | Refresh | Idempotent? |
//! |---|---|---|
//! | Graph projection | fresh, policy-attested projection on read | yes — never retained |
//! | Search projection | fresh stable-corpus scan on read | yes — never retained |
//! | Vault copy (`strategy = "copy"` only) | [`crate::vault::sync_file`] | yes — overwrite |
//! | QMD policy mirror | [`crate::knowledge::refresh_qmd_collection`] | yes — rebuild policy-safe mirror |
//! | Knowledge base (wiki/PARA/Obsidian) | [`crate::knowledge::ingest_file`] | **no** — facts dedup, but the chronological log always appends |
//!
//! Vault and QMD refresh automatically after a write that changes
//! summary-derived frontmatter. Knowledge ingestion remains opt-in
//! ([`RefreshOptions::ingest_knowledge`]) precisely because it is not
//! idempotent: re-ingesting the same meeting writes a second entry into the
//! user's append-only knowledge log even when no fact changed.
//!
//! Every step is **best-effort**. A stale derived view is a nuisance; a failed
//! refresh must never fail an artifact write that already succeeded, so this
//! module has no error return — failures land in [`RefreshReport::warnings`]
//! and the tracing log.

use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::graph::GraphStats;
use crate::knowledge::UpdateResult;

/// Which derived views a refresh should touch.
#[derive(Debug, Clone, Default)]
pub struct RefreshOptions {
    /// Re-run knowledge-base ingestion for this artifact.
    ///
    /// Off by default. [`crate::knowledge::update_from_meeting`] deduplicates
    /// facts, but always appends a chronological log entry, so an automatic
    /// re-ingest on every rewrite would accumulate duplicate lines in a
    /// user-owned file. Callers expose this as an explicit opt-in
    /// (`minutes resummarize --ingest`).
    pub ingest_knowledge: bool,
}

/// Outcome of a QMD collection reindex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QmdRefresh {
    /// No `search.qmd_collection` configured — the common case.
    NotConfigured,
    /// `qmd update` ran and exited successfully.
    Refreshed,
    /// `qmd` could not be spawned, or exited non-zero. The index is stale.
    Failed(String),
}

/// What a refresh actually managed to do.
///
/// Fields are `None`/`false` both when a view is not configured (no vault, no
/// QMD collection, knowledge disabled) and when its refresh failed — the
/// distinction is in [`RefreshReport::warnings`], which is populated only on
/// failure.
#[derive(Debug, Default)]
pub struct RefreshReport {
    /// Legacy report field retained for API compatibility.
    ///
    /// Always `None`: graph answers use disposable policy projections and
    /// therefore have no durable view to refresh.
    pub graph: Option<GraphStats>,
    /// Legacy report field retained for API compatibility.
    ///
    /// Always `false`: search uses a disposable process-private projection and
    /// performs a complete stable source scan before a query.
    pub search_indexed: bool,
    /// Destination path, when a vault copy was written (`strategy = "copy"`).
    pub vault: Option<PathBuf>,
    /// Whether a QMD collection reindex ran and succeeded.
    pub qmd_refreshed: bool,
    /// Knowledge-base result, when `ingest_knowledge` was requested and the
    /// knowledge base is actually enabled. A disabled knowledge base leaves
    /// this `None` and records a warning — never a zero-fact "success".
    pub knowledge: Option<UpdateResult>,
    /// Human-readable failures. Empty on a fully clean refresh.
    pub warnings: Vec<String>,
}

impl RefreshReport {
    /// Did every attempted step succeed?
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }
}

/// Refresh every external view that depends on `path`, best-effort.
///
/// Call this after any write that changes an artifact's summary-derived
/// frontmatter (`entities`, `people`, `intents`, `action_items`, `decisions`)
/// or its AI-owned body sections. It never fails: each step records a warning
/// and the rest continue.
///
/// Search and graph need no action here: their process-private projections are
/// rebuilt from stable, policy-authorized source snapshots when queried.
pub fn refresh_derived_views(path: &Path, config: &Config, opts: &RefreshOptions) -> RefreshReport {
    let mut report = RefreshReport::default();

    match crate::vault::sync_file(path, config) {
        Ok(Some(vault_path)) => {
            crate::events::append_event(crate::events::MinutesEvent::VaultSynced {
                source_path: path.display().to_string(),
                vault_path: vault_path.display().to_string(),
                strategy: config.vault.strategy.clone(),
            });
            report.vault = Some(vault_path);
        }
        Ok(None) => {}
        Err(error) => {
            tracing::warn!(error = %error, artifact = %path.display(), "vault sync failed");
            report.warnings.push(format!("vault sync: {error}"));
        }
    }

    match refresh_qmd_collection(config) {
        QmdRefresh::Refreshed => report.qmd_refreshed = true,
        QmdRefresh::NotConfigured => {}
        QmdRefresh::Failed(reason) => {
            report.warnings.push(format!("qmd reindex: {reason}"));
        }
    }

    if opts.ingest_knowledge {
        refresh_knowledge(path, config, &mut report);
    }

    report
}

/// Re-ingest `path` into the knowledge base, recording the outcome.
///
/// A disabled or unconfigured knowledge base is reported as a warning rather
/// than a zero-fact success: [`crate::knowledge::update_from_meeting`] returns
/// `Ok` with empty counts in that case, which is indistinguishable from a real
/// run that found nothing.
fn refresh_knowledge(path: &Path, config: &Config, report: &mut RefreshReport) {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        report.warnings.push(
            "knowledge ingest requested, but the knowledge base is not enabled \
             (set `knowledge.enabled` and `knowledge.path` in config.toml)"
                .to_string(),
        );
        return;
    }

    match crate::knowledge::ingest_file(path, config) {
        Ok(update) => {
            if update.facts_written > 0 {
                crate::events::append_event(crate::events::MinutesEvent::KnowledgeUpdated {
                    meeting_path: path.display().to_string(),
                    facts_written: update.facts_written,
                    facts_skipped: update.facts_skipped,
                    people_updated: update.people_updated.clone(),
                });
            }
            report.knowledge = Some(update);
        }
        Err(error) => {
            // Two distinct cases arrive here, both correctly non-fatal:
            // the deliberate loud refusal for `sensitivity: restricted`
            // artifacts (a policy exclusion, not a failure), and a genuine
            // partial write — `update_from_meeting` writes person files and
            // always appends its chronological log *before* returning this
            // error, so a retry adds a second log entry.
            tracing::warn!(error = %error, artifact = %path.display(), "knowledge ingest failed");
            report.warnings.push(format!("knowledge ingest: {error}"));
        }
    }
}

/// Ask QMD to reindex the configured collection.
pub fn refresh_qmd_collection(config: &Config) -> QmdRefresh {
    if config.search.qmd_collection.is_none() {
        return QmdRefresh::NotConfigured;
    }
    match crate::knowledge::refresh_qmd_collection(config) {
        Ok(_) => QmdRefresh::Refreshed,
        Err(error) => QmdRefresh::Failed(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// A config whose meetings live in `dir`.
    fn fixture(dir: &TempDir) -> Config {
        Config {
            output_dir: dir.path().to_path_buf(),
            ..Config::default()
        }
    }

    fn write_meeting(dir: &TempDir, name: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(
            &path,
            "---\ntitle: Test\ntype: meeting\ndate: 2026-07-26\n---\n\n## Summary\n\nhi\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn refresh_options_default_leaves_knowledge_opt_in() {
        // The append-only knowledge log makes ingestion non-idempotent, so it
        // must never be the default.
        assert!(!RefreshOptions::default().ingest_knowledge);
    }

    #[test]
    fn qmd_refresh_is_skipped_when_no_collection_configured() {
        let dir = TempDir::new().unwrap();
        let config = fixture(&dir);
        assert!(config.search.qmd_collection.is_none());
        assert_eq!(refresh_qmd_collection(&config), QmdRefresh::NotConfigured);
    }

    #[test]
    fn post_write_refresh_never_creates_durable_search_or_graph_caches() {
        let dir = TempDir::new().unwrap();
        let config = fixture(&dir);
        let artifact = write_meeting(&dir, "meeting.md");
        let graph_db = dir.path().join("graph.db");
        let search_db = dir.path().join("search.db");

        let report = refresh_derived_views(&artifact, &config, &RefreshOptions::default());

        assert!(report.graph.is_none());
        assert!(!report.search_indexed);
        assert!(!graph_db.exists(), "post-write refresh retained graph.db");
        assert!(!search_db.exists(), "post-write refresh retained search.db");
    }

    #[test]
    fn clean_refresh_reports_no_warnings_and_skips_unconfigured_views() {
        let dir = TempDir::new().unwrap();
        let config = fixture(&dir);
        let artifact = write_meeting(&dir, "meeting.md");

        let report = refresh_derived_views(&artifact, &config, &RefreshOptions::default());

        assert!(
            report.is_clean(),
            "unexpected warnings: {:?}",
            report.warnings
        );
        assert!(report.graph.is_none());
        assert!(!report.search_indexed);
        // Vault and QMD are unconfigured by default; knowledge was not opted in.
        assert!(report.vault.is_none());
        assert!(!report.qmd_refreshed);
        assert!(report.knowledge.is_none());
    }

    #[test]
    fn ingest_on_a_disabled_knowledge_base_warns_instead_of_faking_success() {
        // `update_from_meeting` returns Ok with zero counts when knowledge is
        // disabled, which is indistinguishable from "ran, found nothing".
        let dir = TempDir::new().unwrap();
        let config = fixture(&dir);
        let artifact = write_meeting(&dir, "meeting.md");
        assert!(!config.knowledge.enabled);

        let opts = RefreshOptions {
            ingest_knowledge: true,
        };
        let report = refresh_derived_views(&artifact, &config, &opts);

        assert!(
            report.knowledge.is_none(),
            "a disabled knowledge base must not report a zero-fact success"
        );
        assert!(
            report
                .warnings
                .iter()
                .any(|w| w.contains("knowledge base is not enabled")),
            "expected a disabled-knowledge warning, got {:?}",
            report.warnings
        );
    }

    #[test]
    fn restricted_artifacts_are_refused_by_knowledge_ingest() {
        let dir = TempDir::new().unwrap();
        let knowledge_dir = TempDir::new().unwrap();
        let mut config = fixture(&dir);
        config.knowledge.enabled = true;
        config.knowledge.path = knowledge_dir.path().to_path_buf();

        let artifact = dir.path().join("restricted.md");
        std::fs::write(
            &artifact,
            "---\ntitle: Private\ntype: meeting\ndate: 2026-07-26\nsensitivity: restricted\n---\n\n## Summary\n\nhi\n",
        )
        .unwrap();

        let opts = RefreshOptions {
            ingest_knowledge: true,
        };
        let report = refresh_derived_views(&artifact, &config, &opts);

        assert!(report.knowledge.is_none());
        assert!(
            report.warnings.iter().any(|w| w.contains("restricted")),
            "expected the restricted refusal to surface, got {:?}",
            report.warnings
        );
    }
}
