//! Knowledge base integration — maintain a Karpathy-style LLM wiki from meeting data.
//!
//! After each meeting, extract facts about people and decisions, update person
//! profiles, append to a chronological log, and maintain an index. All writes
//! include provenance (source meeting) and confidence levels to prevent
//! hallucination propagation.

use crate::config::{Config, KnowledgeConfig};
use crate::markdown::Frontmatter;
use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ── Types ───────────────────────────────────────────────────────

/// Confidence level for an extracted fact. Mirrors events::InsightConfidence
/// but applied to knowledge base writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Topic discussed, possible direction — never written to profiles by default.
    Tentative,
    /// Inferred from discussion flow.
    Inferred,
    /// Clear discussion → conclusion pattern, or extracted from structured YAML.
    Strong,
    /// Verbatim quote or explicit statement: "We decided...", "I commit to...".
    Explicit,
}

impl Confidence {
    pub fn parse(s: &str) -> Self {
        match s {
            "explicit" => Confidence::Explicit,
            "strong" => Confidence::Strong,
            "inferred" => Confidence::Inferred,
            "tentative" => Confidence::Tentative,
            other => {
                tracing::warn!(
                    value = other,
                    "unknown confidence level in config, defaulting to 'strong' (safe)"
                );
                Confidence::Strong
            }
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::Explicit => "explicit",
            Confidence::Strong => "strong",
            Confidence::Inferred => "inferred",
            Confidence::Tentative => "tentative",
        }
    }

    /// Whether this confidence meets or exceeds the given threshold.
    pub fn meets(&self, threshold: Confidence) -> bool {
        *self >= threshold
    }
}

/// A single extracted fact about a person, with provenance.
#[derive(Debug, Clone)]
pub struct Fact {
    pub text: String,
    pub category: String, // "decision", "commitment", "context", "preference", "relationship"
    pub confidence: Confidence,
    pub source_meeting: String, // filename slug for traceability
    pub source_date: String,    // ISO date
}

/// Facts grouped by person.
#[derive(Debug, Clone)]
pub struct PersonFacts {
    pub slug: String,
    pub name: String,
    pub facts: Vec<Fact>,
}

/// A log entry for the append-only chronological log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub date: DateTime<Local>,
    pub meeting_title: String,
    pub meeting_path: String,
    pub people_updated: Vec<String>,
    pub fact_count: usize,
    pub skipped_count: usize, // facts below confidence threshold
}

/// Result of a knowledge update operation.
#[derive(Debug)]
pub struct UpdateResult {
    pub facts_written: usize,
    pub facts_skipped: usize, // below confidence threshold
    pub people_updated: Vec<String>,
}

// ── Public API ──────────────────────────────────────────────────

/// Main entry point: update the knowledge base from a processed meeting.
/// Called from pipeline.rs after vault sync. Non-fatal — errors are logged, never crash.
pub fn update_from_meeting(
    result: &crate::WriteResult,
    frontmatter: &Frontmatter,
    _transcript: &str,
    config: &Config,
) -> Result<UpdateResult, Box<dyn std::error::Error>> {
    if !config.knowledge.enabled || config.knowledge.path.as_os_str().is_empty() {
        return Ok(UpdateResult {
            facts_written: 0,
            facts_skipped: 0,
            people_updated: vec![],
        });
    }

    let kc = &config.knowledge;
    let min_confidence = Confidence::parse(&kc.min_confidence);

    // Phase 1: Extract facts from structured frontmatter (zero hallucination risk)
    let person_facts = crate::knowledge_extract::extract_from_frontmatter(
        frontmatter,
        &result.path.display().to_string(),
    );

    // Phase 2 (future): Optional LLM extraction from transcript body
    // Only runs when engine != "none". Not implemented yet — structured-first is safer.

    // Phase 3: Write facts through the adapter
    let adapter = make_adapter(kc)?;
    let mut total_written = 0usize;
    let mut total_skipped = 0usize;
    let mut people_updated = Vec::new();
    let mut write_errors: Vec<String> = Vec::new();

    for pf in &person_facts {
        match adapter.update_person(&pf.slug, &pf.name, &pf.facts, min_confidence, kc) {
            Ok((written, skipped)) => {
                if written > 0 {
                    people_updated.push(pf.name.clone());
                }
                total_written += written;
                total_skipped += skipped;
            }
            Err(e) => {
                write_errors.push(format!("{}: {}", pf.name, e));
            }
        }
    }

    // Always write the log entry, even on partial failure
    adapter.append_log(
        &LogEntry {
            date: frontmatter.date,
            meeting_title: frontmatter.title.clone(),
            meeting_path: result.path.display().to_string(),
            people_updated: people_updated.clone(),
            fact_count: total_written,
            skipped_count: total_skipped,
        },
        kc,
    )?;

    if !write_errors.is_empty() {
        return Err(format!(
            "partial knowledge update ({} written, {} failed): {}",
            total_written,
            write_errors.len(),
            write_errors.join("; ")
        )
        .into());
    }

    Ok(UpdateResult {
        facts_written: total_written,
        facts_skipped: total_skipped,
        people_updated,
    })
}

/// Process a single existing meeting file through knowledge extraction (for `minutes ingest`).
pub fn ingest_file(
    meeting_path: &Path,
    config: &Config,
) -> Result<UpdateResult, Box<dyn std::error::Error>> {
    let content = fs::read_to_string(meeting_path)?;
    let (fm_str, body) = crate::markdown::split_frontmatter(&content);
    if fm_str.is_empty() {
        return Err("no frontmatter found".into());
    }
    let frontmatter: Frontmatter = serde_yaml::from_str(fm_str)?;
    let result = crate::WriteResult {
        path: meeting_path.to_path_buf(),
        title: frontmatter.title.clone(),
        word_count: body.split_whitespace().count(),
        content_type: frontmatter.r#type,
    };
    update_from_meeting(&result, &frontmatter, body, config)
}

// ── Adapter dispatch ────────────────────────────────────────────

fn make_adapter(
    kc: &KnowledgeConfig,
) -> Result<Box<dyn KnowledgeAdapter>, Box<dyn std::error::Error>> {
    match kc.adapter.to_lowercase().as_str() {
        "wiki" => Ok(Box::new(WikiAdapter)),
        "para" => Ok(Box::new(ParaAdapter)),
        "obsidian" => Ok(Box::new(ObsidianAdapter)),
        other => Err(format!(
            "unknown knowledge adapter '{}' — valid options: wiki, para, obsidian",
            other
        )
        .into()),
    }
}

// ── Adapter trait ───────────────────────────────────────────────

trait KnowledgeAdapter {
    /// Write facts about a person. Returns (written_count, skipped_count).
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>>;

    /// Append an entry to the chronological log.
    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>>;
}

// ── Wiki Adapter (Karpathy flat markdown) ───────────────────────

struct WikiAdapter;

impl KnowledgeAdapter for WikiAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let dir = config.path.join("people");
        fs::create_dir_all(&dir)?;
        let file_path = dir.join(format!("{}.md", slug));

        let qualifying: Vec<&Fact> = facts
            .iter()
            .filter(|f| f.confidence.meets(min_confidence))
            .collect();
        let skipped = facts.len() - qualifying.len();
        if qualifying.is_empty() {
            return Ok((0, skipped));
        }

        let mut content = if file_path.exists() {
            fs::read_to_string(&file_path)?
        } else {
            format!("# {}\n\n", name)
        };

        // Deduplicate: skip facts whose text already appears in the file
        let new_facts: Vec<&&Fact> = qualifying
            .iter()
            .filter(|f| !content.contains(&f.text))
            .collect();
        if new_facts.is_empty() {
            return Ok((0, skipped));
        }

        // Group by category for structured sections
        let mut by_category: HashMap<&str, Vec<&&Fact>> = HashMap::new();
        for fact in &new_facts {
            by_category
                .entry(fact.category.as_str())
                .or_default()
                .push(fact);
        }

        for (category, cat_facts) in &by_category {
            let section_header = format!("## {}", capitalize(category));
            if !content.contains(&section_header) {
                if !content.ends_with("\n\n") {
                    if !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push('\n');
                }
                content.push_str(&section_header);
                content.push('\n');
                content.push('\n');
            }

            // Insert facts before the next section or at end
            let insert_pos = find_section_end(&content, &section_header);
            let mut block = String::new();
            for fact in cat_facts {
                block.push_str(&format!(
                    "- {} *({}; {} — {})*\n",
                    fact.text,
                    fact.confidence.as_str(),
                    fact.source_date,
                    fact.source_meeting,
                ));
            }
            content.insert_str(insert_pos, &block);
        }

        fs::write(&file_path, &content)?;
        set_restrictive_permissions(&file_path);
        Ok((new_facts.len(), skipped))
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use std::io::Write;

        let log_path = config.path.join(&config.log_file);

        // Create with header if new, then true append (no full rewrite)
        let is_new = !log_path.exists();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)?;
        set_restrictive_permissions(&log_path);

        if is_new {
            write!(file, "# Knowledge Log\n\n")?;
        }

        let people_str = if entry.people_updated.is_empty() {
            "no people updated".to_string()
        } else {
            entry.people_updated.join(", ")
        };

        write!(
            file,
            "## [{}] ingest | {}\n\n- Source: `{}`\n- Facts written: {}, skipped: {}\n- People: {}\n\n",
            entry.date.format("%Y-%m-%d %H:%M"),
            entry.meeting_title,
            entry.meeting_path,
            entry.fact_count,
            entry.skipped_count,
            people_str,
        )?;

        Ok(())
    }
}

// ── PARA Adapter ────────────────────────────────────────────────

struct ParaAdapter;

impl KnowledgeAdapter for ParaAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        let dir = config.path.join("areas").join("people").join(slug);
        fs::create_dir_all(&dir)?;
        let items_path = dir.join("items.json");
        let summary_path = dir.join("summary.md");

        let qualifying: Vec<&Fact> = facts
            .iter()
            .filter(|f| f.confidence.meets(min_confidence))
            .collect();
        let skipped = facts.len() - qualifying.len();
        if qualifying.is_empty() {
            return Ok((0, skipped));
        }

        // Load existing items.json or create empty array.
        // If the file exists but is malformed, back it up and fail rather than
        // silently discarding all accumulated facts.
        let mut items: Vec<serde_json::Value> = if items_path.exists() {
            let raw = fs::read_to_string(&items_path)?;
            match serde_json::from_str(&raw) {
                Ok(parsed) => parsed,
                Err(e) => {
                    let backup = items_path.with_extension("json.corrupt");
                    let _ = fs::copy(&items_path, &backup);
                    return Err(format!(
                        "items.json for {} is malformed (backed up to {}): {}",
                        slug,
                        backup.display(),
                        e
                    )
                    .into());
                }
            }
        } else {
            vec![]
        };

        // Deduplicate by checking if fact text already exists
        let existing_texts: Vec<String> = items
            .iter()
            .filter_map(|item| item.get("fact").and_then(|f| f.as_str()).map(String::from))
            .collect();

        let mut written = 0usize;
        for fact in &qualifying {
            if existing_texts.contains(&fact.text) {
                continue;
            }
            let id = format!("{}-{:x}", slug, hash_fact(&fact.text));
            items.push(serde_json::json!({
                "id": id,
                "fact": fact.text,
                "category": fact.category,
                "confidence": fact.confidence.as_str(),
                "timestamp": fact.source_date,
                "source": fact.source_meeting,
                "status": "active",
                "supersededBy": null,
            }));
            written += 1;
        }

        if written > 0 {
            // Atomic write: temp file → rename (prevents partial writes on crash)
            let tmp_items = items_path.with_extension("json.tmp");
            fs::write(&tmp_items, serde_json::to_string_pretty(&items)?)?;
            fs::rename(&tmp_items, &items_path)?;
            set_restrictive_permissions(&items_path);

            // Regenerate summary.md from active items
            let active_items: Vec<&serde_json::Value> = items
                .iter()
                .filter(|i| i.get("status").and_then(|s| s.as_str()) != Some("superseded"))
                .collect();
            let mut summary = format!("# {}\n\n", name);
            let mut by_cat: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
            for item in &active_items {
                let cat = item
                    .get("category")
                    .and_then(|c| c.as_str())
                    .unwrap_or("context");
                by_cat.entry(cat.to_string()).or_default().push(item);
            }
            for (cat, cat_items) in &by_cat {
                summary.push_str(&format!("## {}\n\n", capitalize(cat)));
                for item in cat_items {
                    let fact_text = item.get("fact").and_then(|f| f.as_str()).unwrap_or("");
                    summary.push_str(&format!("- {}\n", fact_text));
                }
                summary.push('\n');
            }
            let tmp_summary = summary_path.with_extension("md.tmp");
            fs::write(&tmp_summary, &summary)?;
            fs::rename(&tmp_summary, &summary_path)?;
            set_restrictive_permissions(&summary_path);
        }

        Ok((written, skipped))
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // PARA stores log in memory/ directory
        let log_path = config.path.join("memory").join(&config.log_file);
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Reuse wiki log format
        WikiAdapter.append_log(
            entry,
            &KnowledgeConfig {
                path: log_path.parent().unwrap_or(&config.path).to_path_buf(),
                log_file: log_path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                ..config.clone()
            },
        )
    }
}

// ── Obsidian Adapter (wiki + [[wikilinks]]) ─────────────────────

struct ObsidianAdapter;

impl KnowledgeAdapter for ObsidianAdapter {
    fn update_person(
        &self,
        slug: &str,
        name: &str,
        facts: &[Fact],
        min_confidence: Confidence,
        config: &KnowledgeConfig,
    ) -> Result<(usize, usize), Box<dyn std::error::Error>> {
        // Same as wiki adapter but adds [[wikilinks]] to cross-references
        WikiAdapter.update_person(slug, name, facts, min_confidence, config)
        // TODO: post-process to add [[name]] links for any person slugs mentioned in fact text
    }

    fn append_log(
        &self,
        entry: &LogEntry,
        config: &KnowledgeConfig,
    ) -> Result<(), Box<dyn std::error::Error>> {
        WikiAdapter.append_log(entry, config)
    }
}

// ── Helpers ─────────────────────────────────────────────────────

pub(crate) fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Set 0600 permissions on sensitive knowledge files (person profiles, logs).
/// Matches the rest of Minutes' security posture for meeting data.
#[cfg(unix)]
fn set_restrictive_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_restrictive_permissions(_path: &Path) {}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

fn find_section_end(content: &str, section_header: &str) -> usize {
    if let Some(start) = content.find(section_header) {
        let after_header = start + section_header.len();
        // Find next ## or end of file
        if let Some(next_section) = content[after_header..].find("\n## ") {
            after_header + next_section
        } else {
            content.len()
        }
    } else {
        content.len()
    }
}

/// Deterministic FNV-1a hash — stable across Rust toolchain versions.
/// DefaultHasher is NOT stable across versions, which would silently
/// change fact IDs in items.json on compiler upgrades.
fn hash_fact(text: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV offset basis
    for byte in text.to_lowercase().as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash
}

// ── Tests ───────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn confidence_ordering() {
        assert!(Confidence::Explicit > Confidence::Strong);
        assert!(Confidence::Strong > Confidence::Inferred);
        assert!(Confidence::Inferred > Confidence::Tentative);
        assert!(Confidence::Strong.meets(Confidence::Strong));
        assert!(Confidence::Explicit.meets(Confidence::Strong));
        assert!(!Confidence::Inferred.meets(Confidence::Strong));
    }

    #[test]
    fn slugify_names() {
        assert_eq!(slugify("Dan Benamoz"), "dan-benamoz");
        assert_eq!(slugify("  Mat  "), "mat");
        assert_eq!(slugify("Sarah O'Brien"), "sarah-o-brien");
    }

    #[test]
    fn wiki_adapter_creates_person_file() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            adapter: "wiki".into(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "Leads pharmacy operations for RxVIP".into(),
            category: "context".into(),
            confidence: Confidence::Strong,
            source_meeting: "2026-04-03-consult".into(),
            source_date: "2026-04-03".into(),
        }];

        let adapter = WikiAdapter;
        let (written, skipped) = adapter
            .update_person(
                "dan-benamoz",
                "Dan Benamoz",
                &facts,
                Confidence::Strong,
                &config,
            )
            .unwrap();

        assert_eq!(written, 1);
        assert_eq!(skipped, 0);

        let content = fs::read_to_string(dir.path().join("people/dan-benamoz.md")).unwrap();
        assert!(content.contains("# Dan Benamoz"));
        assert!(content.contains("Leads pharmacy operations for RxVIP"));
        assert!(content.contains("strong"));
        assert!(content.contains("2026-04-03-consult"));
    }

    #[test]
    fn wiki_adapter_deduplicates_facts() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "CTO at Acme Corp".into(),
            category: "context".into(),
            confidence: Confidence::Explicit,
            source_meeting: "meeting-1".into(),
            source_date: "2026-04-01".into(),
        }];

        let adapter = WikiAdapter;
        let (w1, _) = adapter
            .update_person("alice", "Alice", &facts, Confidence::Strong, &config)
            .unwrap();
        let (w2, _) = adapter
            .update_person("alice", "Alice", &facts, Confidence::Strong, &config)
            .unwrap();

        assert_eq!(w1, 1);
        assert_eq!(w2, 0); // deduped
    }

    #[test]
    fn wiki_adapter_skips_low_confidence() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            min_confidence: "strong".into(),
            ..Default::default()
        };

        let facts = vec![
            Fact {
                text: "Might be interested in partnership".into(),
                category: "context".into(),
                confidence: Confidence::Tentative,
                source_meeting: "meeting-1".into(),
                source_date: "2026-04-01".into(),
            },
            Fact {
                text: "Confirmed: wants monthly billing".into(),
                category: "decision".into(),
                confidence: Confidence::Explicit,
                source_meeting: "meeting-1".into(),
                source_date: "2026-04-01".into(),
            },
        ];

        let adapter = WikiAdapter;
        let (written, skipped) = adapter
            .update_person("bob", "Bob", &facts, Confidence::Strong, &config)
            .unwrap();

        assert_eq!(written, 1);
        assert_eq!(skipped, 1);

        let content = fs::read_to_string(dir.path().join("people/bob.md")).unwrap();
        assert!(content.contains("Confirmed: wants monthly billing"));
        assert!(!content.contains("Might be interested"));
    }

    #[test]
    fn para_adapter_writes_items_json() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            adapter: "para".into(),
            ..Default::default()
        };

        let facts = vec![Fact {
            text: "Building medicare billing into consultation software".into(),
            category: "commitment".into(),
            confidence: Confidence::Explicit,
            source_meeting: "2026-04-03-consult".into(),
            source_date: "2026-04-03".into(),
        }];

        let adapter = ParaAdapter;
        let (written, _) = adapter
            .update_person(
                "dan-benamoz",
                "Dan Benamoz",
                &facts,
                Confidence::Strong,
                &config,
            )
            .unwrap();

        assert_eq!(written, 1);

        let items_path = dir.path().join("areas/people/dan-benamoz/items.json");
        assert!(items_path.exists());

        let items: Vec<serde_json::Value> =
            serde_json::from_str(&fs::read_to_string(&items_path).unwrap()).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["status"], "active");
        assert_eq!(items[0]["confidence"], "explicit");
        assert_eq!(items[0]["source"], "2026-04-03-consult");

        let summary_path = dir.path().join("areas/people/dan-benamoz/summary.md");
        assert!(summary_path.exists());
        let summary = fs::read_to_string(&summary_path).unwrap();
        assert!(summary.contains("# Dan Benamoz"));
        assert!(summary.contains("medicare billing"));
    }

    #[test]
    fn log_append_creates_and_appends() {
        let dir = TempDir::new().unwrap();
        let config = KnowledgeConfig {
            enabled: true,
            path: dir.path().to_path_buf(),
            ..Default::default()
        };

        let entry = LogEntry {
            date: chrono::Local::now(),
            meeting_title: "Q2 Pricing Call".into(),
            meeting_path: "~/meetings/2026-04-03-pricing.md".into(),
            people_updated: vec!["Dan".into(), "Mat".into()],
            fact_count: 3,
            skipped_count: 1,
        };

        WikiAdapter.append_log(&entry, &config).unwrap();
        WikiAdapter.append_log(&entry, &config).unwrap();

        let log = fs::read_to_string(dir.path().join("log.md")).unwrap();
        assert!(log.contains("# Knowledge Log"));
        assert!(log.contains("Q2 Pricing Call"));
        assert!(log.contains("Facts written: 3, skipped: 1"));
        assert_eq!(log.matches("Q2 Pricing Call").count(), 2); // two appends
    }
}
