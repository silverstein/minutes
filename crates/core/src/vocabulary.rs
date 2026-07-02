use crate::config::IdentityConfig;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Default user-level vocabulary file.
///
/// This is user data, not app configuration: it contains names, organizations,
/// projects, acronyms, and recurring terms that help Minutes bias future
/// transcription and canonicalize derived search/graph projections.
pub fn default_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join(".minutes")
        .join("vocabulary.toml")
}

#[derive(Debug, Error)]
pub enum VocabularyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    TomlParse(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
    #[error("vocabulary entry {index} has an empty canonical value")]
    EmptyCanonical { index: usize },
    #[error(
        "vocabulary alias conflict for '{alias}' between '{existing}' and '{incoming}' in {kind}/{scope}"
    )]
    AliasConflict {
        alias: String,
        existing: String,
        incoming: String,
        kind: VocabularyKind,
        scope: VocabularyScope,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VocabularyKind {
    Person,
    Organization,
    Project,
    #[default]
    Term,
    Acronym,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VocabularyScope {
    #[default]
    Global,
    Project,
    Meeting,
    SourceSpecific,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VocabularyPriority {
    Low,
    #[default]
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VocabularySource {
    #[default]
    Manual,
    Identity,
    SpeakerConfirmation,
    Calendar,
    Import,
    Suggestion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VocabularyEntry {
    pub id: String,
    pub kind: VocabularyKind,
    pub canonical: String,
    pub aliases: Vec<String>,
    pub scope: VocabularyScope,
    pub priority: VocabularyPriority,
    pub source: VocabularySource,
    pub notes: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
}

impl Default for VocabularyEntry {
    fn default() -> Self {
        Self {
            id: String::new(),
            kind: VocabularyKind::Term,
            canonical: String::new(),
            aliases: Vec::new(),
            scope: VocabularyScope::Global,
            priority: VocabularyPriority::Normal,
            source: VocabularySource::Manual,
            notes: None,
            created_at: None,
            updated_at: None,
        }
    }
}

impl VocabularyEntry {
    pub fn new(kind: VocabularyKind, canonical: impl Into<String>) -> Self {
        Self {
            kind,
            canonical: canonical.into(),
            ..Self::default()
        }
    }

    fn normalize(mut self, index: usize) -> Result<Self, VocabularyError> {
        self.canonical = self.canonical.trim().to_string();
        if self.canonical.is_empty() {
            return Err(VocabularyError::EmptyCanonical { index });
        }

        self.aliases = unique_clean_aliases(self.aliases, &self.canonical);
        if self.id.trim().is_empty() {
            self.id = entry_id(self.kind, &self.canonical);
        } else {
            self.id = slugify_id(self.id.trim());
        }
        if self.id.is_empty() {
            self.id = entry_id(self.kind, &self.canonical);
        }

        Ok(self)
    }

    pub fn surface_forms(&self) -> Vec<String> {
        let mut forms = vec![self.canonical.clone()];
        forms.extend(self.aliases.iter().cloned());
        unique_clean_aliases(forms, "")
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VocabularyStore {
    pub entries: Vec<VocabularyEntry>,
}

impl VocabularyStore {
    pub fn empty() -> Self {
        Self { entries: vec![] }
    }

    pub fn from_identity(identity: &IdentityConfig) -> Self {
        let Some(name) = identity
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return Self::empty();
        };

        let mut entry = VocabularyEntry::new(VocabularyKind::Person, name);
        entry.aliases = identity.aliases.clone();
        entry.priority = VocabularyPriority::High;
        entry.source = VocabularySource::Identity;

        Self {
            entries: vec![entry],
        }
        .normalized()
        .unwrap_or_else(|_| Self::empty())
    }

    pub fn normalized(self) -> Result<Self, VocabularyError> {
        let mut merged: HashMap<(VocabularyKind, VocabularyScope, String), VocabularyEntry> =
            HashMap::new();

        for (index, entry) in self.entries.into_iter().enumerate() {
            let entry = entry.normalize(index)?;
            let key = (entry.kind, entry.scope, key_for(&entry.canonical));
            if let Some(existing) = merged.get_mut(&key) {
                existing.aliases = merge_aliases(&existing.aliases, &entry.aliases);
                existing.priority = existing.priority.max(entry.priority);
                if existing.source != VocabularySource::Manual {
                    existing.source = entry.source;
                }
                // Advance the audit timestamp to the most recent of the two so a
                // later merge (which adds aliases) refreshes `updated_at`. Using
                // max never regresses to an older stamp and never blanks an
                // existing one when the incoming entry has no timestamp.
                existing.updated_at =
                    std::cmp::max(existing.updated_at.clone(), entry.updated_at.clone());
            } else {
                merged.insert(key, entry);
            }
        }

        let mut entries = merged.into_values().collect::<Vec<_>>();
        entries.sort_by(|a, b| {
            a.kind
                .label()
                .cmp(b.kind.label())
                .then_with(|| a.canonical.to_lowercase().cmp(&b.canonical.to_lowercase()))
        });

        let store = Self { entries };
        store.validate_alias_conflicts()?;
        Ok(store)
    }

    pub fn merge(
        stores: impl IntoIterator<Item = VocabularyStore>,
    ) -> Result<Self, VocabularyError> {
        let mut entries = Vec::new();
        for store in stores {
            entries.extend(store.entries);
        }
        Self { entries }.normalized()
    }

    pub fn decode_phrases(&self, limit: usize) -> Vec<String> {
        let mut entries = self.entries.clone();
        entries.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.kind.label().cmp(b.kind.label()))
                .then_with(|| a.canonical.to_lowercase().cmp(&b.canonical.to_lowercase()))
        });

        let mut phrases = Vec::new();
        let mut seen = HashSet::new();
        for entry in entries {
            for phrase in entry.surface_forms() {
                let key = key_for(&phrase);
                if seen.insert(key) {
                    phrases.push(phrase);
                    if phrases.len() >= limit {
                        return phrases;
                    }
                }
            }
        }
        phrases
    }

    pub fn search_expansions(&self, query: &str) -> Vec<String> {
        let query_key = key_for(query);
        if query_key.is_empty() {
            return Vec::new();
        }

        let mut out = Vec::new();
        let mut seen = HashSet::new();
        for entry in &self.entries {
            let forms = entry.surface_forms();
            if forms.iter().any(|form| key_for(form) == query_key) {
                for form in forms {
                    let key = key_for(&form);
                    if seen.insert(key) {
                        out.push(form);
                    }
                }
            }
        }
        out
    }

    fn validate_alias_conflicts(&self) -> Result<(), VocabularyError> {
        let mut seen: HashMap<(VocabularyKind, VocabularyScope, String), &VocabularyEntry> =
            HashMap::new();

        for entry in &self.entries {
            for form in entry.surface_forms() {
                let key = (entry.kind, entry.scope, key_for(&form));
                let Some(existing) = seen.insert(key, entry) else {
                    continue;
                };
                if key_for(&existing.canonical) != key_for(&entry.canonical) {
                    return Err(VocabularyError::AliasConflict {
                        alias: form,
                        existing: existing.canonical.clone(),
                        incoming: entry.canonical.clone(),
                        kind: entry.kind,
                        scope: entry.scope,
                    });
                }
            }
        }
        Ok(())
    }
}

impl VocabularyKind {
    fn label(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organization => "organization",
            Self::Project => "project",
            Self::Term => "term",
            Self::Acronym => "acronym",
        }
    }
}

impl fmt::Display for VocabularyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl VocabularyScope {
    fn label(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
            Self::Meeting => "meeting",
            Self::SourceSpecific => "source-specific",
        }
    }
}

impl fmt::Display for VocabularyScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

pub fn load() -> Result<VocabularyStore, VocabularyError> {
    load_at(&default_path())
}

pub fn load_at(path: &Path) -> Result<VocabularyStore, VocabularyError> {
    if !path.exists() {
        return Ok(VocabularyStore::empty());
    }

    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(VocabularyStore::empty());
    }

    let store: VocabularyStore = toml::from_str(&raw)?;
    store.normalized()
}

pub fn save_at(path: &Path, store: &VocabularyStore) -> Result<(), VocabularyError> {
    let store = store.clone().normalized()?;
    let serialized = toml::to_string_pretty(&store)?;
    write_private_atomic(path, &serialized)?;
    Ok(())
}

fn unique_clean_aliases(aliases: Vec<String>, canonical: &str) -> Vec<String> {
    let canonical_key = key_for(canonical);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for alias in aliases {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }
        let key = key_for(trimmed);
        if key.is_empty() || key == canonical_key {
            continue;
        }
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

fn merge_aliases(existing: &[String], incoming: &[String]) -> Vec<String> {
    existing
        .iter()
        .chain(incoming.iter())
        .cloned()
        .collect::<Vec<_>>()
        .pipe(|aliases| unique_clean_aliases(aliases, ""))
}

fn key_for(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_ascii_lowercase()
}

fn entry_id(kind: VocabularyKind, canonical: &str) -> String {
    format!("{}-{}", kind.label(), slugify_id(canonical))
        .trim_end_matches('-')
        .to_string()
}

fn slugify_id(value: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn write_private_atomic(path: &Path, content: &str) -> Result<(), VocabularyError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, content)?;
    set_private_permissions(&tmp)?;
    fs::rename(&tmp, path)?;
    set_private_permissions(path)?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<(), VocabularyError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<(), VocabularyError> {
    Ok(())
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_loads_empty_store() {
        let tmp = tempfile::tempdir().unwrap();
        let store = load_at(&tmp.path().join("missing.toml")).unwrap();
        assert!(store.entries.is_empty());
    }

    #[test]
    fn normalizes_entries_and_dedupes_aliases() {
        let store = VocabularyStore {
            entries: vec![VocabularyEntry {
                kind: VocabularyKind::Organization,
                canonical: " Automattic ".into(),
                aliases: vec![
                    "Automatic".into(),
                    "automatic ".into(),
                    "Automattic".into(),
                    " ".into(),
                ],
                ..VocabularyEntry::default()
            }],
        }
        .normalized()
        .unwrap();

        assert_eq!(store.entries.len(), 1);
        let entry = &store.entries[0];
        assert_eq!(entry.id, "organization-automattic");
        assert_eq!(entry.canonical, "Automattic");
        assert_eq!(entry.aliases, vec!["Automatic"]);
    }

    #[test]
    fn merge_combines_same_canonical_entries() {
        let a = VocabularyStore {
            entries: vec![VocabularyEntry {
                kind: VocabularyKind::Person,
                canonical: "Elijah Potter".into(),
                aliases: vec!["Elijah".into()],
                priority: VocabularyPriority::Normal,
                ..VocabularyEntry::default()
            }],
        };
        let b = VocabularyStore {
            entries: vec![VocabularyEntry {
                kind: VocabularyKind::Person,
                canonical: "Elijah Potter".into(),
                aliases: vec!["Eli".into()],
                priority: VocabularyPriority::High,
                ..VocabularyEntry::default()
            }],
        };

        let merged = VocabularyStore::merge([a, b]).unwrap();
        assert_eq!(merged.entries.len(), 1);
        assert_eq!(merged.entries[0].aliases, vec!["Elijah", "Eli"]);
        assert_eq!(merged.entries[0].priority, VocabularyPriority::High);
    }

    #[test]
    fn merge_refreshes_updated_at_when_aliases_added() {
        // Two same-canonical entries with different timestamps; combining them
        // adds an alias, so updated_at should advance to the newer stamp.
        let store = VocabularyStore {
            entries: vec![
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Junrei".into(),
                    aliases: vec!["Junlei".into()],
                    updated_at: Some("2026-07-01T00:00:00+00:00".into()),
                    ..VocabularyEntry::default()
                },
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Junrei".into(),
                    aliases: vec!["Jun-Rei".into()],
                    updated_at: Some("2026-07-02T00:00:00+00:00".into()),
                    ..VocabularyEntry::default()
                },
            ],
        }
        .normalized()
        .unwrap();
        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].aliases, vec!["Junlei", "Jun-Rei"]);
        assert_eq!(
            store.entries[0].updated_at.as_deref(),
            Some("2026-07-02T00:00:00+00:00"),
            "updated_at should advance to the newer merge"
        );
    }

    #[test]
    fn merge_updated_at_never_regresses_or_blanks() {
        // Newer entry first, older second, plus an entry with no timestamp: the
        // merged updated_at must be the newest present, never the older one, and
        // never None.
        let store = VocabularyStore {
            entries: vec![
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Junrei".into(),
                    aliases: vec!["Junlei".into()],
                    updated_at: Some("2026-07-05T00:00:00+00:00".into()),
                    ..VocabularyEntry::default()
                },
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Junrei".into(),
                    aliases: vec!["Jun-Rei".into()],
                    updated_at: Some("2026-07-01T00:00:00+00:00".into()),
                    ..VocabularyEntry::default()
                },
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Junrei".into(),
                    aliases: vec!["Junwei".into()],
                    updated_at: None,
                    ..VocabularyEntry::default()
                },
            ],
        }
        .normalized()
        .unwrap();
        assert_eq!(store.entries.len(), 1);
        assert_eq!(
            store.entries[0].updated_at.as_deref(),
            Some("2026-07-05T00:00:00+00:00"),
            "must keep the newest stamp, never regress or blank"
        );
    }

    #[test]
    fn conflicting_aliases_fail_closed() {
        let err = VocabularyStore {
            entries: vec![
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Sarah Chen".into(),
                    aliases: vec!["Sarah".into()],
                    ..VocabularyEntry::default()
                },
                VocabularyEntry {
                    kind: VocabularyKind::Person,
                    canonical: "Sarah Miller".into(),
                    aliases: vec!["Sarah".into()],
                    ..VocabularyEntry::default()
                },
            ],
        }
        .normalized()
        .unwrap_err();

        assert!(matches!(err, VocabularyError::AliasConflict { .. }));
    }

    #[test]
    fn identity_becomes_high_priority_person_entry() {
        let store = VocabularyStore::from_identity(&IdentityConfig {
            name: Some("Mat Silverstein".into()),
            email: Some("mat@example.com".into()),
            emails: vec!["mathieu@example.com".into()],
            aliases: vec!["Mat".into(), "Mathieu".into()],
        });

        assert_eq!(store.entries.len(), 1);
        assert_eq!(store.entries[0].kind, VocabularyKind::Person);
        assert_eq!(store.entries[0].canonical, "Mat Silverstein");
        assert_eq!(store.entries[0].aliases, vec!["Mat", "Mathieu"]);
        assert_eq!(store.entries[0].priority, VocabularyPriority::High);
        assert_eq!(store.entries[0].source, VocabularySource::Identity);
    }

    #[test]
    fn decode_phrases_are_ranked_and_bounded() {
        let store = VocabularyStore {
            entries: vec![
                VocabularyEntry {
                    kind: VocabularyKind::Project,
                    canonical: "Harper".into(),
                    priority: VocabularyPriority::Normal,
                    ..VocabularyEntry::default()
                },
                VocabularyEntry {
                    kind: VocabularyKind::Organization,
                    canonical: "Automattic".into(),
                    aliases: vec!["Automatic".into()],
                    priority: VocabularyPriority::High,
                    ..VocabularyEntry::default()
                },
            ],
        }
        .normalized()
        .unwrap();

        assert_eq!(
            store.decode_phrases(2),
            vec!["Automattic".to_string(), "Automatic".to_string()]
        );
    }

    #[test]
    fn search_expansions_return_canonical_and_aliases() {
        let store = VocabularyStore {
            entries: vec![VocabularyEntry {
                kind: VocabularyKind::Organization,
                canonical: "Automattic".into(),
                aliases: vec!["Automatic".into(), "Automattic Inc".into()],
                ..VocabularyEntry::default()
            }],
        }
        .normalized()
        .unwrap();

        assert_eq!(
            store.search_expansions("automatic"),
            vec![
                "Automattic".to_string(),
                "Automatic".to_string(),
                "Automattic Inc".to_string()
            ]
        );
    }

    #[test]
    fn save_and_load_round_trip_toml() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("vocabulary.toml");
        let store = VocabularyStore {
            entries: vec![VocabularyEntry {
                kind: VocabularyKind::Acronym,
                canonical: "PBM".into(),
                aliases: vec!["Pharmacy benefit manager".into()],
                ..VocabularyEntry::default()
            }],
        };

        save_at(&path, &store).unwrap();
        let loaded = load_at(&path).unwrap();
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(loaded.entries[0].canonical, "PBM");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }
}
