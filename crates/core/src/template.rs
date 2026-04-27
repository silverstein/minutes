//! First-class templates for domain-shaped summaries.
//!
//! A template is a markdown file with YAML frontmatter that extends Minutes'
//! structured extraction for a specific domain. Templates are additive: they
//! preserve the baseline contract (`KEY POINTS`, `DECISIONS`, `ACTION ITEMS`,
//! `OPEN QUESTIONS`, `COMMITMENTS`, `PARTICIPANTS`) and layer additional
//! prompt instructions on top.
//!
//! ## Phase 1 scope
//!
//! Phase 1 implements the prompt-only slice of RFC 0001 (#147, refs #143):
//!
//! - [`Template`] struct with the Phase 1 frontmatter fields only.
//! - [`TemplateResolver`] with project > user > bundled resolution.
//! - [`compose_additional_instructions`] for appending the template's
//!   `additional_instructions` to the base summarizer prompt.
//!
//! Fields belonging to later phases (`triggers`, `extract`, `compliance`,
//! `agent_context`, `post_record_skill`, `extends`) are NOT supported in
//! Phase 1. The loader uses `serde(deny_unknown_fields)` and rejects them
//! with a clear "needs newer Minutes version" error so a Phase 2/3 template
//! never silently loses meaning on a Phase 1 binary.

use crate::error::TemplateError;
use crate::markdown::split_frontmatter;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// Where a template was loaded from. Used for diagnostics and to pick a
/// stable winner during resolution (project > user > bundled).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateSource {
    /// Repo-local: `<cwd>/.minutes/templates/`. Highest precedence.
    Project,
    /// User-installed: `~/.minutes/templates/`.
    User,
    /// Shipped with the binary via `include_str!`.
    Bundled,
}

impl TemplateSource {
    pub fn as_str(self) -> &'static str {
        match self {
            TemplateSource::Project => "project",
            TemplateSource::User => "user",
            TemplateSource::Bundled => "bundled",
        }
    }
}

/// Phase 1 frontmatter fields. Anything outside this set is rejected at
/// load time with [`TemplateError::UnsupportedField`].
///
/// Later phases will add `triggers`, `extract`, `compliance`,
/// `agent_context`, `post_record_skill`, `extends`. Once those land, the
/// `deny_unknown_fields` attribute should remain — a binary that does not
/// implement a field should not advertise support for it by accepting it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemplateFrontmatter {
    /// Human-readable name (e.g. `"Engineering Standup"`).
    pub name: String,
    /// CLI identifier and resolution key. Lowercase, alphanumeric, hyphens.
    pub slug: String,
    /// Semver version string (e.g. `"1.0.0"`).
    pub version: String,
    /// One-line description used in `minutes template list` output.
    #[serde(default)]
    pub description: String,
    /// Search and SEO keywords.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// If true (default), the baseline structured extraction still runs.
    /// Phase 1 only supports `true`; the field is parsed but a future phase
    /// will honor `false` for replacement-style templates.
    #[serde(default = "default_extends_base")]
    pub extends_base: bool,
    /// Free-form instructions appended (never substituted) to the base
    /// summarizer prompt. Empty by default.
    #[serde(default)]
    pub additional_instructions: String,
    /// Override `[summarization].language` for this template. `None` means
    /// inherit the config's effective summary language.
    #[serde(default)]
    pub language: Option<String>,
}

fn default_extends_base() -> bool {
    true
}

/// A loaded template: parsed frontmatter plus the human-readable body.
#[derive(Debug, Clone)]
pub struct Template {
    pub frontmatter: TemplateFrontmatter,
    pub body: String,
    pub source: TemplateSource,
    /// Path on disk for project/user templates. `None` for bundled.
    pub path: Option<PathBuf>,
}

impl Template {
    /// Parse a template from raw markdown source.
    ///
    /// `display_path` is used in error messages to point the user at the
    /// offending file (or `<bundled:slug>` for bundled templates).
    pub fn from_str(
        source_text: &str,
        origin: TemplateSource,
        path: Option<PathBuf>,
        display_path: &str,
    ) -> Result<Self, TemplateError> {
        let (fm_text, body) = split_frontmatter(source_text);
        if fm_text.is_empty() {
            return Err(TemplateError::Invalid {
                path: display_path.to_string(),
                message: "missing YAML frontmatter (file must start with '---')".into(),
            });
        }

        let frontmatter: TemplateFrontmatter = match serde_yaml::from_str(fm_text) {
            Ok(fm) => fm,
            Err(e) => {
                let message = e.to_string();
                if let Some(field) = unknown_field_from_serde_error(&message) {
                    return Err(TemplateError::UnsupportedField {
                        path: display_path.to_string(),
                        field,
                    });
                }
                return Err(TemplateError::Invalid {
                    path: display_path.to_string(),
                    message,
                });
            }
        };

        validate_slug(&frontmatter.slug, display_path)?;
        validate_version(&frontmatter.version, display_path)?;

        Ok(Template {
            frontmatter,
            body: body.to_string(),
            source: origin,
            path,
        })
    }

    /// Load and parse a template from a file on disk.
    pub fn load_file(path: &Path, origin: TemplateSource) -> Result<Self, TemplateError> {
        let display = path.display().to_string();
        let text = fs::read_to_string(path)?;
        Self::from_str(&text, origin, Some(path.to_path_buf()), &display)
    }

    /// Slug shortcut used for resolver lookups and listing output.
    pub fn slug(&self) -> &str {
        &self.frontmatter.slug
    }
}

/// `serde_yaml` formats unknown-field errors as
/// `unknown field \`triggers\`, expected one of \`name\`, ...`.
/// Pull the field name out so we can return a friendlier error.
fn unknown_field_from_serde_error(message: &str) -> Option<String> {
    let prefix = "unknown field `";
    let start = message.find(prefix)? + prefix.len();
    let rest = &message[start..];
    let end = rest.find('`')?;
    Some(rest[..end].to_string())
}

fn validate_slug(slug: &str, display_path: &str) -> Result<(), TemplateError> {
    if slug.is_empty() {
        return Err(TemplateError::InvalidSlug {
            path: display_path.to_string(),
            slug: slug.to_string(),
        });
    }
    let valid = slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let bookended = !slug.starts_with('-') && !slug.ends_with('-');
    if !valid || !bookended {
        return Err(TemplateError::InvalidSlug {
            path: display_path.to_string(),
            slug: slug.to_string(),
        });
    }
    Ok(())
}

fn validate_version(version: &str, display_path: &str) -> Result<(), TemplateError> {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
    {
        return Err(TemplateError::InvalidVersion {
            path: display_path.to_string(),
            version: version.to_string(),
        });
    }
    Ok(())
}

// ── Bundled templates ─────────────────────────────────────────

/// Bundled templates compiled into the binary. Phase 1 ships four
/// non-clinical templates; clinical families (`soap`, `medical-fr-base`,
/// etc.) ship in Phase 3.
const BUNDLED: &[(&str, &str)] = &[
    ("meeting", include_str!("../templates/meeting.md")),
    ("standup", include_str!("../templates/standup.md")),
    ("1-on-1", include_str!("../templates/1-on-1.md")),
    ("voice-memo", include_str!("../templates/voice-memo.md")),
];

/// The default template slug used when `--template` is not provided.
pub const DEFAULT_TEMPLATE_SLUG: &str = "meeting";

/// Load a bundled template by slug. Used internally by the resolver and
/// directly by tests.
pub fn load_bundled(slug: &str) -> Result<Template, TemplateError> {
    let entry = BUNDLED
        .iter()
        .find(|(s, _)| *s == slug)
        .ok_or_else(|| TemplateError::NotFound(slug.to_string()))?;
    let display = format!("<bundled:{}>", entry.0);
    Template::from_str(entry.1, TemplateSource::Bundled, None, &display)
}

/// Iterate over every bundled template.
pub fn bundled_slugs() -> impl Iterator<Item = &'static str> {
    BUNDLED.iter().map(|(slug, _)| *slug)
}

// ── Resolver ─────────────────────────────────────────────────

/// One row in `minutes template list`.
#[derive(Debug, Clone)]
pub struct TemplateListing {
    pub slug: String,
    pub name: String,
    pub description: String,
    pub source: TemplateSource,
}

/// Resolves templates by slug across project, user, and bundled sources.
#[derive(Debug, Clone)]
pub struct TemplateResolver {
    project_dir: Option<PathBuf>,
    user_dir: PathBuf,
}

impl TemplateResolver {
    /// Default resolver:
    /// - project_dir = `<cwd>/.minutes/templates/` if it exists
    /// - user_dir = `~/.minutes/templates/`
    pub fn new() -> Self {
        let project_dir = std::env::current_dir()
            .ok()
            .map(|cwd| cwd.join(".minutes").join("templates"))
            .filter(|p| p.is_dir());
        Self {
            project_dir,
            user_dir: default_user_templates_dir(),
        }
    }

    /// Test/override constructor.
    pub fn with_dirs(project_dir: Option<PathBuf>, user_dir: PathBuf) -> Self {
        Self {
            project_dir,
            user_dir,
        }
    }

    /// Resolve a template by slug. Order: project > user > bundled.
    pub fn resolve(&self, slug: &str) -> Result<Template, TemplateError> {
        if let Some(dir) = &self.project_dir {
            if let Some(t) = try_load_from_dir(dir, slug, TemplateSource::Project)? {
                return Ok(t);
            }
        }
        if let Some(t) = try_load_from_dir(&self.user_dir, slug, TemplateSource::User)? {
            return Ok(t);
        }
        load_bundled(slug)
    }

    /// List every available template, deduped by slug. Earlier source wins.
    pub fn list(&self) -> Vec<TemplateListing> {
        let mut seen: Vec<TemplateListing> = Vec::new();
        let mut record = |t: Template| {
            if !seen.iter().any(|x| x.slug == t.frontmatter.slug) {
                seen.push(TemplateListing {
                    slug: t.frontmatter.slug.clone(),
                    name: t.frontmatter.name.clone(),
                    description: t.frontmatter.description.clone(),
                    source: t.source,
                });
            }
        };

        if let Some(dir) = &self.project_dir {
            for t in list_dir(dir, TemplateSource::Project) {
                record(t);
            }
        }
        for t in list_dir(&self.user_dir, TemplateSource::User) {
            record(t);
        }
        for slug in bundled_slugs() {
            if let Ok(t) = load_bundled(slug) {
                record(t);
            }
        }

        seen.sort_by(|a, b| a.slug.cmp(&b.slug));
        seen
    }
}

impl Default for TemplateResolver {
    fn default() -> Self {
        Self::new()
    }
}

fn default_user_templates_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".minutes").join("templates")
}

fn try_load_from_dir(
    dir: &Path,
    slug: &str,
    origin: TemplateSource,
) -> Result<Option<Template>, TemplateError> {
    let path = dir.join(format!("{}.md", slug));
    if !path.exists() {
        return Ok(None);
    }
    Template::load_file(&path, origin).map(Some)
}

fn list_dir(dir: &Path, origin: TemplateSource) -> Vec<Template> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut templates = Vec::new();
    for entry in read.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        match Template::load_file(&path, origin) {
            Ok(t) => templates.push(t),
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "skipping invalid template")
            }
        }
    }
    templates
}

// ── Prompt composition ───────────────────────────────────────

/// Append a template's `additional_instructions` to a base summarizer
/// prompt. Returns `base.to_string()` unchanged when the template is
/// `None` or its `additional_instructions` field is empty/whitespace.
///
/// This is the only Phase 1 mechanism for prompt customization: the base
/// prompt and its structured-output contract are preserved verbatim.
pub fn compose_additional_instructions(base: &str, template: Option<&Template>) -> String {
    let Some(template) = template else {
        return base.to_string();
    };
    let extra = template.frontmatter.additional_instructions.trim();
    if extra.is_empty() {
        return base.to_string();
    }
    format!(
        "{base}\n\nADDITIONAL INSTRUCTIONS (from template \"{name}\"):\n{extra}",
        base = base,
        name = template.frontmatter.name,
        extra = extra,
    )
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn minimal_template() -> &'static str {
        "---\nname: Test\nslug: test\nversion: 1.0.0\n---\n\nbody\n"
    }

    fn write_template(dir: &Path, slug: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{}.md", slug));
        fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn parses_minimal_template() {
        let t = Template::from_str(minimal_template(), TemplateSource::Bundled, None, "<test>")
            .unwrap();
        assert_eq!(t.frontmatter.slug, "test");
        assert_eq!(t.frontmatter.name, "Test");
        assert_eq!(t.frontmatter.version, "1.0.0");
        assert!(t.frontmatter.extends_base, "extends_base defaults to true");
        assert!(t.frontmatter.additional_instructions.is_empty());
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let err = Template::from_str("just a body\n", TemplateSource::Bundled, None, "<test>")
            .unwrap_err();
        match err {
            TemplateError::Invalid { message, .. } => assert!(message.contains("frontmatter")),
            other => panic!("expected Invalid, got {:?}", other),
        }
    }

    #[test]
    fn rejects_missing_required_fields() {
        let src = "---\nslug: test\nversion: 1.0.0\n---\n";
        let err = Template::from_str(src, TemplateSource::Bundled, None, "<test>").unwrap_err();
        assert!(matches!(err, TemplateError::Invalid { .. }));
    }

    #[test]
    fn rejects_invalid_slug() {
        let cases = ["Test", "with spaces", "-leading", "trailing-", "UP", ""];
        for bad in cases {
            let src = format!("---\nname: T\nslug: {}\nversion: 1.0.0\n---\n", bad);
            let err = Template::from_str(&src, TemplateSource::Bundled, None, "<test>")
                .expect_err(&format!("slug {:?} should be rejected", bad));
            assert!(
                matches!(err, TemplateError::InvalidSlug { .. }),
                "slug {:?} produced {:?}",
                bad,
                err
            );
        }
    }

    #[test]
    fn accepts_valid_slugs() {
        for good in ["t", "test", "1-on-1", "soap-fr", "abc123"] {
            let src = format!("---\nname: T\nslug: {}\nversion: 1.0.0\n---\n", good);
            Template::from_str(&src, TemplateSource::Bundled, None, "<test>")
                .unwrap_or_else(|e| panic!("slug {:?} should parse: {:?}", good, e));
        }
    }

    #[test]
    fn rejects_invalid_version() {
        for bad in ["1", "1.0", "v1.0.0", "1.0.0-rc1", "1.a.0", ""] {
            let src = format!("---\nname: T\nslug: t\nversion: {}\n---\n", bad);
            let err = Template::from_str(&src, TemplateSource::Bundled, None, "<test>")
                .expect_err(&format!("version {:?} should be rejected", bad));
            assert!(
                matches!(err, TemplateError::InvalidVersion { .. }),
                "version {:?} produced {:?}",
                bad,
                err
            );
        }
    }

    #[test]
    fn rejects_unknown_phase2_field() {
        let src = "---\nname: T\nslug: t\nversion: 1.0.0\nextract:\n  foo: bar\n---\n";
        let err = Template::from_str(src, TemplateSource::Bundled, None, "<test>").unwrap_err();
        match err {
            TemplateError::UnsupportedField { field, .. } => assert_eq!(field, "extract"),
            other => panic!("expected UnsupportedField, got {:?}", other),
        }
    }

    #[test]
    fn rejects_unknown_phase3_field() {
        let src = "---\nname: T\nslug: t\nversion: 1.0.0\ncompliance:\n  redact_phi: true\n---\n";
        let err = Template::from_str(src, TemplateSource::Bundled, None, "<test>").unwrap_err();
        match err {
            TemplateError::UnsupportedField { field, .. } => assert_eq!(field, "compliance"),
            other => panic!("expected UnsupportedField, got {:?}", other),
        }
    }

    #[test]
    fn all_bundled_templates_load() {
        for slug in bundled_slugs() {
            let t = load_bundled(slug)
                .unwrap_or_else(|e| panic!("bundled template {} failed to load: {:?}", slug, e));
            assert_eq!(t.frontmatter.slug, slug);
            assert_eq!(t.source, TemplateSource::Bundled);
            assert!(t.path.is_none());
        }
    }

    #[test]
    fn bundled_includes_default_meeting() {
        let t = load_bundled(DEFAULT_TEMPLATE_SLUG).unwrap();
        assert_eq!(t.frontmatter.slug, "meeting");
        assert!(t.frontmatter.additional_instructions.is_empty());
    }

    #[test]
    fn resolver_prefers_project_over_user_over_bundled() {
        let dir = TempDir::new().unwrap();
        let project_dir = dir.path().join("project");
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&project_dir).unwrap();
        fs::create_dir_all(&user_dir).unwrap();

        // All three define `meeting` with a distinct name.
        write_template(
            &project_dir,
            "meeting",
            "---\nname: Project Meeting\nslug: meeting\nversion: 1.0.0\n---\n",
        );
        write_template(
            &user_dir,
            "meeting",
            "---\nname: User Meeting\nslug: meeting\nversion: 1.0.0\n---\n",
        );

        // Project wins.
        let r = TemplateResolver::with_dirs(Some(project_dir.clone()), user_dir.clone());
        let t = r.resolve("meeting").unwrap();
        assert_eq!(t.frontmatter.name, "Project Meeting");
        assert_eq!(t.source, TemplateSource::Project);

        // Without project, user wins over bundled.
        let r = TemplateResolver::with_dirs(None, user_dir.clone());
        let t = r.resolve("meeting").unwrap();
        assert_eq!(t.frontmatter.name, "User Meeting");
        assert_eq!(t.source, TemplateSource::User);

        // Without project or user, bundled wins.
        let r = TemplateResolver::with_dirs(None, dir.path().join("nonexistent"));
        let t = r.resolve("meeting").unwrap();
        assert_eq!(t.source, TemplateSource::Bundled);
    }

    #[test]
    fn resolver_returns_not_found_for_unknown_slug() {
        let dir = TempDir::new().unwrap();
        let r = TemplateResolver::with_dirs(None, dir.path().to_path_buf());
        let err = r.resolve("does-not-exist").unwrap_err();
        assert!(matches!(err, TemplateError::NotFound(_)));
    }

    #[test]
    fn list_dedups_by_slug_and_sorts() {
        let dir = TempDir::new().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        // Override one bundled, add a custom one.
        write_template(
            &user_dir,
            "standup",
            "---\nname: Custom Standup\nslug: standup\nversion: 2.0.0\n---\n",
        );
        write_template(
            &user_dir,
            "intake",
            "---\nname: Intake\nslug: intake\nversion: 1.0.0\n---\n",
        );

        let r = TemplateResolver::with_dirs(None, user_dir);
        let listings = r.list();
        let slugs: Vec<&str> = listings.iter().map(|l| l.slug.as_str()).collect();
        // Sorted, no duplicates, includes the override and the new one
        // alongside bundled templates.
        assert!(
            slugs.windows(2).all(|w| w[0] < w[1]),
            "expected sorted, got {:?}",
            slugs
        );
        let standup = listings.iter().find(|l| l.slug == "standup").unwrap();
        assert_eq!(standup.source, TemplateSource::User);
        assert_eq!(standup.name, "Custom Standup");
        assert!(listings.iter().any(|l| l.slug == "intake"));
        assert!(listings.iter().any(|l| l.slug == "meeting"));
    }

    #[test]
    fn list_skips_invalid_files() {
        let dir = TempDir::new().unwrap();
        let user_dir = dir.path().join("user");
        fs::create_dir_all(&user_dir).unwrap();
        // Valid template
        write_template(
            &user_dir,
            "ok",
            "---\nname: OK\nslug: ok\nversion: 1.0.0\n---\n",
        );
        // Invalid (bad version) — should be skipped, not panic.
        write_template(
            &user_dir,
            "bad",
            "---\nname: Bad\nslug: bad\nversion: notsemver\n---\n",
        );

        let r = TemplateResolver::with_dirs(None, user_dir);
        let listings = r.list();
        assert!(listings.iter().any(|l| l.slug == "ok"));
        assert!(!listings.iter().any(|l| l.slug == "bad"));
    }

    #[test]
    fn compose_returns_base_when_template_none() {
        let base = "BASE PROMPT";
        assert_eq!(compose_additional_instructions(base, None), base);
    }

    #[test]
    fn compose_returns_base_when_instructions_empty() {
        let t = load_bundled("meeting").unwrap();
        assert!(t.frontmatter.additional_instructions.is_empty());
        assert_eq!(compose_additional_instructions("BASE", Some(&t)), "BASE");
    }

    #[test]
    fn compose_appends_additional_instructions() {
        let t = load_bundled("standup").unwrap();
        assert!(!t.frontmatter.additional_instructions.is_empty());
        let composed = compose_additional_instructions("BASE PROMPT", Some(&t));
        assert!(composed.starts_with("BASE PROMPT"));
        assert!(
            composed.contains("ADDITIONAL INSTRUCTIONS (from template \"Engineering Standup\")")
        );
        assert!(composed.contains("Blockers are the priority"));
    }

    #[test]
    fn compose_treats_whitespace_only_as_empty() {
        let src =
            "---\nname: WS\nslug: ws\nversion: 1.0.0\nadditional_instructions: \"   \\n  \"\n---\n";
        let t = Template::from_str(src, TemplateSource::Bundled, None, "<test>").unwrap();
        assert_eq!(compose_additional_instructions("BASE", Some(&t)), "BASE");
    }
}
