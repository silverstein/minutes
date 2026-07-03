use crate::knowledge::slugify;
use crate::markdown::EntityRef;
use std::collections::{HashMap, HashSet};

/// Role and title descriptors that must not contaminate a person's canonical identity.
///
/// The LLM occasionally appends role context when extracting entities from transcripts
/// (e.g. "Junlei, tech lead" → should produce slug `junlei`, not `junlei-tech-lead`).
/// Listed longest-first so more specific phrases are stripped before shorter sub-phrases.
const ROLE_TITLE_SUFFIXES: &[&str] = &[
    "engineering manager",
    "technical lead",
    "engineering lead",
    "product manager",
    "project manager",
    "product lead",
    "design lead",
    "senior engineer",
    "lead engineer",
    "principal engineer",
    "backend engineer",
    "frontend engineer",
    "software engineer",
    "core team",
    "team member",
    "team lead",
    "tech lead",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PersonIdentity {
    pub slug: String,
    pub name: String,
    pub aliases: Vec<String>,
}

#[derive(Clone, Debug)]
struct PersonCandidate {
    identity: PersonIdentity,
    alias_score: usize,
    support_score: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PersonCanonicalizer {
    exact_matches: HashMap<String, Vec<usize>>,
    slug_matches: HashMap<String, Vec<usize>>,
    candidates: Vec<PersonCandidate>,
}

impl PersonCanonicalizer {
    pub(crate) fn new<I, S>(entities: &[EntityRef], context_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut canonicalizer = Self::default();

        for entity in entities {
            let Some(identity) = normalize_entity_identity(entity) else {
                continue;
            };

            let exact_keys = exact_keys_for_entity(entity);
            let slug_keys = slug_keys_for_entity(entity);

            let alias_score = exact_keys.len().max(slug_keys.len());
            let idx = canonicalizer.candidates.len();
            canonicalizer.candidates.push(PersonCandidate {
                identity,
                alias_score,
                support_score: 1,
            });

            for key in exact_keys {
                canonicalizer
                    .exact_matches
                    .entry(key)
                    .or_default()
                    .push(idx);
            }
            for key in slug_keys {
                canonicalizer.slug_matches.entry(key).or_default().push(idx);
            }
        }

        let context_values: Vec<String> = context_names
            .into_iter()
            .filter_map(|raw| normalize_raw_name(raw.as_ref()).map(|(_, name)| name.to_string()))
            .collect();

        for raw in context_values {
            let exact = canonicalizer.lookup_exact(&raw);
            if let Some(idx) = canonicalizer.pick_best_index(exact) {
                canonicalizer.candidates[idx].support_score += 1;
                continue;
            }

            let slug = slugify(&raw);
            if slug.is_empty() {
                continue;
            }

            if let Some(idx) = canonicalizer.pick_best_index(canonicalizer.lookup_slug(&slug)) {
                canonicalizer.candidates[idx].support_score += 1;
            }
        }

        canonicalizer
    }

    pub(crate) fn resolve(&self, raw: &str) -> Option<PersonIdentity> {
        let (_, trimmed) = normalize_raw_name(raw)?;

        if let Some(idx) = self.pick_best_index(self.lookup_exact(trimmed)) {
            return Some(self.candidates[idx].identity.clone());
        }

        let slug = slugify(trimmed);
        if slug.is_empty() {
            return None;
        }

        if let Some(idx) = self.pick_best_index(self.lookup_slug(&slug)) {
            return Some(self.candidates[idx].identity.clone());
        }

        Some(PersonIdentity {
            slug,
            name: trimmed.to_string(),
            aliases: vec![],
        })
    }

    pub(crate) fn resolve_entity(&self, entity: &EntityRef) -> Option<PersonIdentity> {
        if let Some(identity) = self.resolve(&entity.label) {
            return Some(identity);
        }
        if let Some(identity) = self.resolve(&entity.slug) {
            return Some(identity);
        }
        normalize_entity_identity(entity)
    }

    fn lookup_exact<'a>(&'a self, raw: &str) -> &'a [usize] {
        self.exact_matches
            .get(&raw.to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn lookup_slug<'a>(&'a self, slug: &str) -> &'a [usize] {
        self.slug_matches
            .get(slug)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    fn pick_best_index(&self, indices: &[usize]) -> Option<usize> {
        let mut best_idx: Option<usize> = None;
        let mut best_support = 0usize;
        let mut best_alias = 0usize;
        let mut ambiguous = false;

        for &idx in indices {
            let candidate = &self.candidates[idx];
            let support = candidate.support_score;
            let alias = candidate.alias_score;

            match best_idx {
                None => {
                    best_idx = Some(idx);
                    best_support = support;
                    best_alias = alias;
                }
                Some(_) if ambiguous => {
                    if support > best_support && alias > best_alias {
                        best_idx = Some(idx);
                        best_support = support;
                        best_alias = alias;
                        ambiguous = false;
                    }
                }
                Some(_) => {
                    if support > best_support || (support == best_support && alias > best_alias) {
                        best_idx = Some(idx);
                        best_support = support;
                        best_alias = alias;
                    } else if support == best_support && alias == best_alias {
                        ambiguous = true;
                    }
                }
            }
        }

        if ambiguous {
            None
        } else {
            best_idx
        }
    }
}

fn normalize_raw_name(raw: &str) -> Option<(&str, &str)> {
    let trimmed = raw.trim().trim_start_matches('@').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some((raw, trimmed))
    }
}

/// Returns true if `fragment` (the text after a separator) is a known role or title.
///
/// Normalizes hyphens to spaces and strips leading articles ("the", "a", "an") before
/// matching, so "the core team" and "core-team" both match "core team".
fn trailing_fragment_is_role(fragment: &str) -> bool {
    let normalized: String = fragment
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' { ' ' } else { c })
        .collect();
    let text = normalized
        .trim()
        .trim_start_matches("the ")
        .trim_start_matches("a ")
        .trim_start_matches("an ");
    ROLE_TITLE_SUFFIXES.iter().any(|&role| text.contains(role))
}

/// Strip a trailing role or title descriptor from a person name string.
///
/// Each structural separator (comma, parenthetical, dash, connectors) only fires when
/// the trailing fragment actually contains a [`ROLE_TITLE_SUFFIXES`] token, avoiding
/// false positives on nicknames (`"Robert (Bob) Smith"`), generational suffixes
/// (`"Sammy Davis, Jr."`), or names with connective words (`"Winnie the Pooh"`).
///
/// Examples:
/// - `"Junlei, tech lead"` → `"Junlei"`
/// - `"Junrei (core team)"` → `"Junrei"`
/// - `"Sam the tech lead"` → `"Sam"`
/// - `"Junrei from the core team"` → `"Junrei"`
/// - `"Alex — engineering lead"` → `"Alex"`
/// - `"Junlei Tech Lead"` → `"Junlei"`
/// - `"junlei-tech-lead"` → `"junlei"`
/// - `"Robert (Bob) Smith"` → `"Robert (Bob) Smith"` (unchanged)
/// - `"Sammy Davis, Jr."` → `"Sammy Davis, Jr."` (unchanged)
/// - `"Winnie the Pooh"` → `"Winnie the Pooh"` (unchanged)
/// - `"Dan Benamoz"` → `"Dan Benamoz"` (unchanged)
pub(crate) fn strip_role_suffix(name: &str) -> &str {
    // 1. Comma: "Name, role" — only when what follows is a known role.
    if let Some(pos) = name.find(", ") {
        if trailing_fragment_is_role(&name[pos + 2..]) {
            return name[..pos].trim();
        }
    }
    // 2. Parenthetical at end: "Name (role)" — the ends_with(')') guard excludes
    // "Name (nickname) Surname" patterns where the paren is not terminal.
    if name.ends_with(')') {
        if let Some(pos) = name.find(" (") {
            let inside = name[pos + 2..name.len() - 1].trim();
            if trailing_fragment_is_role(inside) {
                return name[..pos].trim();
            }
        }
    }
    // 3. Spaced dash variants: "Name — role", "Name – role", "Name - role"
    for sep in [" — ", " – ", " - "] {
        if let Some(pos) = name.find(sep) {
            if trailing_fragment_is_role(&name[pos + sep.len()..]) {
                return name[..pos].trim();
            }
        }
    }
    // 4. Connective words before a known role descriptor.
    for connector in [" from ", " the "] {
        if let Some(pos) = name.find(connector) {
            let before = name[..pos].trim();
            if !before.is_empty() && trailing_fragment_is_role(&name[pos + connector.len()..]) {
                return before;
            }
        }
    }
    // 5. Vocabulary-based suffix: "Junlei Tech Lead" or "junlei-tech-lead".
    // Normalize hyphens → spaces so slug-form and label-form are treated equally.
    let normalized: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' { ' ' } else { c })
        .collect();
    for &role in ROLE_TITLE_SUFFIXES {
        if !normalized.ends_with(role) || normalized.len() <= role.len() {
            continue;
        }
        let split = normalized.len() - role.len();
        // Word boundary: the character before the role suffix must be whitespace.
        if normalized.as_bytes().get(split.saturating_sub(1)) != Some(&b' ') {
            continue;
        }
        // Guard: split must be a valid char boundary in the original string.
        // Since '-' and ' ' are both single ASCII bytes the lengths match, but
        // non-ASCII chars that change byte count on lowercasing would invalidate this.
        if !name.is_char_boundary(split) {
            continue;
        }
        let candidate = name[..split].trim_end_matches(|c: char| c == '-' || c.is_whitespace());
        if !candidate.is_empty() {
            return candidate;
        }
    }
    name
}

/// Curated, exact-match list of non-person "names" that must never become a person
/// entity. Deliberately NOT heuristic (no length or initials rules) so genuine short
/// names like "An", "Jo", "Hai" are never dropped — losing a real attendee is as
/// harmful as keeping a contaminated entry. Matched after normalization (lowercased,
/// hyphens -> spaces, whitespace collapsed) and after role/speaker stripping (#385).
const NON_PERSON_NAMES: &[&str] = &[
    "all",
    "none",
    "none identified",
    "not identified",
    "unknown",
    "unknown speaker",
    "unnamed",
    "unassigned",
    "everyone",
    "everybody",
    "anyone",
    "someone",
    "somebody",
    "nobody",
    "no one",
    "team",
    "the team",
    "team member",
    "team members",
    "group",
    "the group",
    "others",
    "attendee",
    "attendees",
    "participant",
    "participants",
    "speaker",
    "speakers",
    // Multi-word role/group phrases (never a real person name), plus a few
    // clearly-non-person single words. Single words that double as real
    // given names or surnames (dev, guest, host, owner, engineer, manager,
    // member, ...) are deliberately NOT listed — dropping a real attendee is
    // as harmful as keeping a contaminated entry (#385).
    "devops",
    "chatbot",
    "qa engineer",
    "chatbot owner",
    "chatbot owner developer",
    "new member",
    "new qa member",
    "additional devops team members",
];

/// Normalize a name for non-person matching: lowercase, hyphens -> spaces, collapse
/// whitespace.
fn normalize_for_match(name: &str) -> String {
    name.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c == '-' { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True if `name` is plausibly a person, not a generic/group/role token.
///
/// Conservative exact-match denylist: only the explicitly listed non-person tokens
/// are rejected, so anything ambiguous (including short real names) is kept. Apply
/// after [`strip_role_suffix`] / [`strip_speaker_label`] so e.g. "Junlei Tech Lead"
/// has already become "Junlei" before this check.
/// Group/role marker tokens used only for the multi-word all-markers rule below.
/// Safe to be broad here (including name-colliding words like "dev"/"owner") because
/// the rule requires EVERY token to be a marker, and a real multi-word person name
/// always carries at least one non-marker token ("Mark Engineer", "An Le").
const GROUP_ROLE_TOKENS: &[&str] = &[
    "team",
    "teams",
    "group",
    "groups",
    "devops",
    "ops",
    "qa",
    "member",
    "members",
    "new",
    "additional",
    "the",
    "a",
    "an",
    "engineering",
    "engineer",
    "developer",
    "dev",
    "owner",
    "manager",
    "lead",
    "leads",
    "staff",
    "crew",
    "admin",
    "intern",
    "analyst",
    "designer",
    "consultant",
    "chatbot",
    "bot",
    "others",
    "attendees",
    "participants",
    "speaker",
    "speakers",
    "unknown",
    "unnamed",
    "unassigned",
];

pub(crate) fn is_plausible_person_name(name: &str) -> bool {
    let normalized = normalize_for_match(name);
    if normalized.is_empty() || NON_PERSON_NAMES.contains(&normalized.as_str()) {
        return false;
    }
    // Reject multi-word phrases that are entirely group/role markers ("qa team",
    // "devops team members"). Single tokens are governed only by the exact denylist
    // above, so real one-word names (Dev, Owner, An) survive (#385).
    let tokens: Vec<&str> = normalized.split(' ').collect();
    if tokens.len() >= 2 && tokens.iter().all(|t| GROUP_ROLE_TOKENS.contains(t)) {
        return false;
    }
    true
}

/// Strip a trailing diarization speaker label ("speaker 0", "-speaker-2") from a name,
/// in slug or label form, leaving the rest of the name intact (#385).
///
/// - `"Geert speaker 0"` -> `"Geert"`
/// - `"geert-speaker-0"` -> `"geert"`
/// - `"gert anne speaker 2"` -> `"gert anne"`
/// - `"speaker-1"` / `"speaker 0"` -> `""` (caller treats empty as non-person)
/// - `"Dan Benamoz"` / `"Catch 22"` -> unchanged (no trailing speaker token)
pub(crate) fn strip_speaker_label(name: &str) -> &str {
    // Scan the ORIGINAL bytes (not a lowercased/normalized copy) so byte offsets
    // can never drift on non-ASCII names (e.g. "İ" lowercasing to two chars). The
    // label tokens ("speaker", digits, "-"/" ") are all ASCII, so ASCII-only
    // comparison on the original is exact, and the cut index lands on a char
    // boundary because the byte before it is an ASCII separator (or string start).
    let b = name.as_bytes();
    let mut end = b.len();
    while end > 0 && b[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    // Require a trailing run of digits...
    let mut i = end;
    while i > 0 && b[i - 1].is_ascii_digit() {
        i -= 1;
    }
    if i == end {
        return name;
    }
    // ...preceded by optional spaces/hyphens and the word "speaker".
    let mut j = i;
    while j > 0 && (b[j - 1] == b' ' || b[j - 1] == b'-') {
        j -= 1;
    }
    const KW: &[u8] = b"speaker";
    if j < KW.len() || !b[j - KW.len()..j].eq_ignore_ascii_case(KW) {
        return name;
    }
    let start = j - KW.len();
    // "speaker" must be at a word boundary: string start or an ASCII space/hyphen.
    if start != 0 {
        let prev = b[start - 1];
        if prev != b' ' && prev != b'-' {
            return name;
        }
    }
    name[..start].trim_end_matches(|c: char| c == '-' || c.is_whitespace())
}

/// Strip both role/title suffixes and trailing speaker labels, to a fixpoint.
///
/// A single pass is order-sensitive ("Junlei Tech Lead speaker 0" needs the speaker
/// label removed before the role becomes trailing) and not idempotent for stacked
/// labels ("Geert speaker 0 speaker 1"). Iterating until stable handles both and
/// guarantees `strip_contamination(strip_contamination(x)) == strip_contamination(x)`.
pub(crate) fn strip_contamination(name: &str) -> &str {
    let mut cur = name.trim();
    loop {
        let next = strip_speaker_label(strip_role_suffix(cur)).trim();
        if next.len() == cur.len() {
            return next;
        }
        cur = next;
    }
}

fn normalize_entity_identity(entity: &EntityRef) -> Option<PersonIdentity> {
    // Role/title descriptors ("tech lead"), diarization speaker labels
    // ("speaker 0"), and generic/group tokens ("team", "qa engineer") must not
    // become or contaminate a person identity. Strip then gate before slugging (#385).
    let raw = if entity.label.trim().is_empty() {
        entity.slug.trim()
    } else {
        entity.label.trim()
    };
    let name = strip_contamination(raw).to_string();
    if !is_plausible_person_name(&name) {
        return None;
    }
    let slug = slugify(&name);
    if slug.is_empty() {
        return None;
    }

    Some(PersonIdentity {
        slug,
        name,
        aliases: unique_aliases(entity.aliases.iter().cloned()),
    })
}

fn exact_keys_for_entity(entity: &EntityRef) -> HashSet<String> {
    let mut keys = HashSet::new();

    for value in [entity.slug.trim(), entity.label.trim()]
        .into_iter()
        .chain(entity.aliases.iter().map(|a| a.trim()))
    {
        if value.is_empty() {
            continue;
        }
        keys.insert(value.to_ascii_lowercase());
        // Also index the stripped form so clean lookups ("Junlei") resolve to the entity
        // even when the stored label is "Junlei, tech lead".
        let stripped = strip_role_suffix(value);
        if stripped != value {
            keys.insert(stripped.to_ascii_lowercase());
        }
    }

    keys
}

fn slug_keys_for_entity(entity: &EntityRef) -> HashSet<String> {
    let mut keys = HashSet::new();

    for value in std::iter::once(entity.slug.as_str())
        .chain(std::iter::once(entity.label.as_str()))
        .chain(entity.aliases.iter().map(String::as_str))
    {
        let slug = slugify(value);
        if !slug.is_empty() {
            keys.insert(slug);
        }
        // Also index the role-stripped slug so clean-form lookups ("Junlei") still
        // resolve to the entity when the stored label is "Junlei Tech Lead".
        let stripped = strip_role_suffix(value.trim());
        if stripped != value.trim() {
            let stripped_slug = slugify(stripped);
            if !stripped_slug.is_empty() {
                keys.insert(stripped_slug);
            }
        }
    }

    keys
}

fn unique_aliases<I>(aliases: I) -> Vec<String>
where
    I: IntoIterator<Item = String>,
{
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for alias in aliases {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            out.push(trimmed.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dan_entities() -> Vec<EntityRef> {
        vec![EntityRef {
            slug: "dan-benamoz".into(),
            label: "Dan Benamoz".into(),
            aliases: vec!["Dan".into(), "dan".into()],
        }]
    }

    #[test]
    fn resolves_raw_name_through_alias_table() {
        let resolver = PersonCanonicalizer::new(&dan_entities(), ["Dan"]);
        let identity = resolver.resolve("Dan").expect("resolved identity");
        assert_eq!(identity.slug, "dan-benamoz");
        assert_eq!(identity.name, "Dan Benamoz");
    }

    #[test]
    fn falls_back_to_raw_slug_when_no_entity_matches() {
        let resolver = PersonCanonicalizer::new(&[], ["Dan"]);
        let identity = resolver.resolve("Dan").expect("fallback identity");
        assert_eq!(identity.slug, "dan");
        assert_eq!(identity.name, "Dan");
    }

    #[test]
    fn chooses_stronger_context_when_aliases_collide() {
        let resolver = PersonCanonicalizer::new(
            &[
                EntityRef {
                    slug: "dan-benamoz".into(),
                    label: "Dan Benamoz".into(),
                    aliases: vec!["Dan".into(), "DB".into(), "Daniel".into()],
                },
                EntityRef {
                    slug: "dan-smith".into(),
                    label: "Dan Smith".into(),
                    aliases: vec!["Dan".into()],
                },
            ],
            ["Dan", "Dan Benamoz", "DB"],
        );

        let identity = resolver.resolve("Dan").expect("collision resolution");
        assert_eq!(identity.slug, "dan-benamoz");
    }

    #[test]
    fn case_insensitive_matching_works() {
        let resolver = PersonCanonicalizer::new(&dan_entities(), ["DAN"]);
        let identity = resolver.resolve("DAN").expect("case-insensitive identity");
        assert_eq!(identity.slug, "dan-benamoz");
    }

    #[test]
    fn ambiguous_collision_without_stronger_signal_falls_back() {
        let resolver = PersonCanonicalizer::new(
            &[
                EntityRef {
                    slug: "dan-benamoz".into(),
                    label: "Dan Benamoz".into(),
                    aliases: vec!["Dan".into()],
                },
                EntityRef {
                    slug: "dan-smith".into(),
                    label: "Dan Smith".into(),
                    aliases: vec!["Dan".into()],
                },
            ],
            ["Dan"],
        );

        let identity = resolver.resolve("Dan").expect("ambiguous fallback");
        assert_eq!(identity.slug, "dan");
        assert_eq!(identity.name, "Dan");
    }

    fn candidate(alias_score: usize, support_score: usize) -> PersonCandidate {
        PersonCandidate {
            identity: PersonIdentity {
                slug: format!("candidate-{alias_score}-{support_score}"),
                name: format!("Candidate {alias_score}/{support_score}"),
                aliases: vec![],
            },
            alias_score,
            support_score,
        }
    }

    #[test]
    fn pick_best_index_keeps_ambiguity_latched_after_equal_top_tie() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(3, 1)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), None);
    }

    #[test]
    fn pick_best_index_returns_strictly_higher_scoring_candidate() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(3, 2)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), Some(2));
    }

    #[test]
    fn pick_best_index_returns_none_when_all_top_candidates_tie() {
        let canonicalizer = PersonCanonicalizer {
            candidates: vec![candidate(2, 1), candidate(2, 1), candidate(2, 1)],
            ..Default::default()
        };

        assert_eq!(canonicalizer.pick_best_index(&[0, 1, 2]), None);
    }

    // ── strip_role_suffix ────────────────────────────────────────

    #[test]
    fn strip_role_suffix_comma_separator() {
        assert_eq!(strip_role_suffix("Junlei, tech lead"), "Junlei");
        assert_eq!(strip_role_suffix("Junlei, the tech lead"), "Junlei");
    }

    #[test]
    fn strip_role_suffix_parenthetical() {
        assert_eq!(strip_role_suffix("Junrei (core team)"), "Junrei");
        assert_eq!(strip_role_suffix("Alex (engineering lead)"), "Alex");
    }

    #[test]
    fn strip_role_suffix_em_dash() {
        assert_eq!(strip_role_suffix("Alex — engineering lead"), "Alex");
        assert_eq!(strip_role_suffix("Sam – product manager"), "Sam");
    }

    #[test]
    fn strip_role_suffix_spaced_dash() {
        assert_eq!(strip_role_suffix("Alex - tech lead"), "Alex");
        assert_eq!(strip_role_suffix("Sam - product manager"), "Sam");
    }

    #[test]
    fn strip_role_suffix_from_connector() {
        assert_eq!(strip_role_suffix("Junrei from the core team"), "Junrei");
        // "engineering" alone is not a role token — must not strip.
        assert_eq!(
            strip_role_suffix("Sam from engineering"),
            "Sam from engineering"
        );
    }

    // ── false-positive guard tests (requested in silverstein/minutes#374) ─────

    #[test]
    fn strip_role_suffix_nickname_in_parens_left_intact() {
        // Parenthetical nickname with surname: "Robert (Bob) Smith" must not lose the surname.
        assert_eq!(
            strip_role_suffix("Robert (Bob) Smith"),
            "Robert (Bob) Smith"
        );
        assert_eq!(
            strip_role_suffix("Mike (Michael) Johnson"),
            "Mike (Michael) Johnson"
        );
    }

    #[test]
    fn strip_role_suffix_generational_suffix_left_intact() {
        // Generational and credential suffixes after a comma must not be stripped.
        assert_eq!(strip_role_suffix("Sammy Davis, Jr."), "Sammy Davis, Jr.");
        assert_eq!(strip_role_suffix("Jane Doe, PhD"), "Jane Doe, PhD");
    }

    #[test]
    fn strip_role_suffix_the_in_name_left_intact() {
        // "the" connector must only fire when a known role follows.
        assert_eq!(strip_role_suffix("Winnie the Pooh"), "Winnie the Pooh");
        assert_eq!(
            strip_role_suffix("Alexander the Great"),
            "Alexander the Great"
        );
    }

    #[test]
    fn strip_role_suffix_the_connector() {
        assert_eq!(strip_role_suffix("Sam the tech lead"), "Sam");
    }

    #[test]
    fn strip_role_suffix_vocabulary_label_form() {
        assert_eq!(strip_role_suffix("Junlei Tech Lead"), "Junlei");
        assert_eq!(strip_role_suffix("Junrei Core Team"), "Junrei");
        assert_eq!(strip_role_suffix("Alex Senior Engineer"), "Alex");
        assert_eq!(strip_role_suffix("Pat Engineering Manager"), "Pat");
    }

    #[test]
    fn strip_role_suffix_vocabulary_slug_form() {
        assert_eq!(strip_role_suffix("junlei-tech-lead"), "junlei");
        assert_eq!(strip_role_suffix("junrei-core-team"), "junrei");
        assert_eq!(strip_role_suffix("alex-senior-engineer"), "alex");
    }

    #[test]
    fn strip_role_suffix_clean_name_untouched() {
        assert_eq!(strip_role_suffix("Dan Benamoz"), "Dan Benamoz");
        assert_eq!(strip_role_suffix("Sarah Chen"), "Sarah Chen");
        assert_eq!(strip_role_suffix("Jordan Mills"), "Jordan Mills");
        assert_eq!(strip_role_suffix("dan-benamoz"), "dan-benamoz");
    }

    // ── speaker-label stripping (#385) ──────────────────────────

    #[test]
    fn strip_speaker_label_removes_trailing_label() {
        assert_eq!(strip_speaker_label("Geert speaker 0"), "Geert");
        assert_eq!(strip_speaker_label("geert-speaker-0"), "geert");
        assert_eq!(strip_speaker_label("Tanya Speaker 1"), "Tanya");
        assert_eq!(strip_speaker_label("gert anne speaker 2"), "gert anne");
    }

    #[test]
    fn strip_speaker_label_bare_label_becomes_empty() {
        assert_eq!(strip_speaker_label("speaker-1"), "");
        assert_eq!(strip_speaker_label("speaker 0"), "");
        assert_eq!(strip_speaker_label("Speaker 4"), "");
    }

    #[test]
    fn strip_speaker_label_leaves_clean_names_and_trailing_numbers() {
        assert_eq!(strip_speaker_label("Dan Benamoz"), "Dan Benamoz");
        assert_eq!(strip_speaker_label("dan-benamoz"), "dan-benamoz");
        // a trailing number that is not a speaker label is untouched
        assert_eq!(strip_speaker_label("Catch 22"), "Catch 22");
        assert_eq!(strip_speaker_label("Studio 54"), "Studio 54");
    }

    // ── non-person gate (#385) ──────────────────────────────────

    #[test]
    fn is_plausible_person_name_rejects_generic_and_role_tokens() {
        for bad in [
            "all",
            "team",
            "unassigned",
            "none-identified",
            "unknown",
            "unnamed",
            "speaker",
            "qa-engineer",
            "devops",
            "chatbot-owner",
            "chatbot-owner-developer",
            "new-qa-member",
            "additional-devops-team-members",
            "The Team",
            "PARTICIPANTS",
        ] {
            assert!(!is_plausible_person_name(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn is_plausible_person_name_keeps_real_short_and_compound_names() {
        // genuine short (often non-Western) names must never be dropped, and single
        // words that double as real given names / surnames must survive too (#385).
        for ok in [
            "An",
            "Jo",
            "Hai",
            "Tao",
            "Jia",
            "Geert",
            "Dan Benamoz",
            "Mark Engineer",
            "Dev",
            "Guest",
            "Host",
            "Owner",
            "Engineer",
            "Manager",
        ] {
            assert!(is_plausible_person_name(ok), "should keep {ok:?}");
        }
    }

    #[test]
    fn is_plausible_person_name_rejects_group_phrase_variants() {
        // multi-word phrases that are all group/role markers (#385 follow-on)
        for bad in [
            "qa team",
            "devops team",
            "devops team members",
            "new qa members",
            "additional devops team member",
            "the team",
        ] {
            assert!(!is_plausible_person_name(bad), "should reject {bad:?}");
        }
        // but real multi-word names with a genuine name token survive
        for ok in [
            "Mark Engineer",
            "Dev Patel",
            "An Le",
            "Dan Benamoz",
            "Owner Mcowner",
        ] {
            assert!(is_plausible_person_name(ok), "should keep {ok:?}");
        }
    }

    #[test]
    fn strip_contamination_does_not_eat_real_name_tokens() {
        // codex follow-up: confirm strip_role_suffix doesn't strip bare role-ish
        // surnames before the gate sees them.
        for name in ["Mark Engineer", "Dev", "Owner", "Manager", "Guest"] {
            assert_eq!(
                strip_contamination(name),
                name,
                "{name:?} must be untouched"
            );
        }
    }

    #[test]
    fn strip_speaker_label_preserves_non_ascii_prefix() {
        // byte-offset regression: lowercasing the Turkish dotted I changes byte length;
        // scanning the original must not slice mid-name (#385).
        assert_eq!(strip_speaker_label("İ Speaker 1"), "İ");
        assert_eq!(strip_speaker_label("İsmet speaker 1"), "İsmet");
        assert_eq!(strip_speaker_label("José speaker 2"), "José");
    }

    #[test]
    fn strip_contamination_handles_order_and_is_idempotent() {
        // speaker label trailing the role: must remove both regardless of order
        assert_eq!(strip_contamination("Junlei Tech Lead speaker 0"), "Junlei");
        assert_eq!(strip_contamination("Junlei, tech lead"), "Junlei");
        // stacked labels collapse fully
        assert_eq!(strip_contamination("Geert speaker 0 speaker 1"), "Geert");
        // idempotent
        for s in [
            "Junlei Tech Lead speaker 0",
            "Geert speaker 0 speaker 1",
            "Dan Benamoz",
        ] {
            let once = strip_contamination(s);
            assert_eq!(strip_contamination(once), once, "not idempotent for {s:?}");
        }
    }

    #[test]
    fn normalize_entity_identity_strips_speaker_label() {
        let entity = EntityRef {
            slug: "geert-speaker-0".into(),
            label: "Geert speaker 0".into(),
            aliases: vec![],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "geert");
    }

    #[test]
    fn normalize_entity_identity_rejects_non_person_tokens() {
        for bad in ["all", "team", "unassigned", "qa-engineer", "speaker-1"] {
            let entity = EntityRef {
                slug: bad.into(),
                label: String::new(),
                aliases: vec![],
            };
            assert!(
                normalize_entity_identity(&entity).is_none(),
                "should drop {bad:?}"
            );
        }
    }

    #[test]
    fn normalize_entity_identity_strips_comma_role_from_label() {
        let entity = EntityRef {
            slug: "junlei-tech-lead".into(),
            label: "Junlei, tech lead".into(),
            aliases: vec![],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "junlei");
        assert_eq!(identity.name, "Junlei");
    }

    #[test]
    fn normalize_entity_identity_strips_parenthetical_role() {
        let entity = EntityRef {
            slug: "junrei-core-team".into(),
            label: "Junrei (core team)".into(),
            aliases: vec![],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "junrei");
        assert_eq!(identity.name, "Junrei");
    }

    #[test]
    fn normalize_entity_identity_strips_vocab_suffix_from_label() {
        let entity = EntityRef {
            slug: "junlei-tech-lead".into(),
            label: "Junlei Tech Lead".into(),
            aliases: vec![],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "junlei");
        assert_eq!(identity.name, "Junlei");
    }

    #[test]
    fn normalize_entity_identity_strips_slug_when_label_is_empty() {
        let entity = EntityRef {
            slug: "junlei-tech-lead".into(),
            label: "".into(),
            aliases: vec![],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "junlei");
    }

    #[test]
    fn normalize_entity_identity_clean_entity_unchanged() {
        let entity = EntityRef {
            slug: "dan-benamoz".into(),
            label: "Dan Benamoz".into(),
            aliases: vec!["Dan".into()],
        };
        let identity = normalize_entity_identity(&entity).expect("should produce identity");
        assert_eq!(identity.slug, "dan-benamoz");
        assert_eq!(identity.name, "Dan Benamoz");
    }

    #[test]
    fn canonicalizer_resolves_contaminated_entity_to_clean_slug() {
        let entities = vec![
            EntityRef {
                slug: "junlei-tech-lead".into(),
                label: "Junlei, tech lead".into(),
                aliases: vec![],
            },
            EntityRef {
                slug: "junrei-core-team".into(),
                label: "Junrei (core team)".into(),
                aliases: vec![],
            },
        ];
        let resolver = PersonCanonicalizer::new(&entities, ["Junlei", "Junrei"]);

        let junlei = resolver.resolve("Junlei").expect("should resolve Junlei");
        assert_eq!(junlei.slug, "junlei", "role-stripped slug expected");

        let junrei = resolver.resolve("Junrei").expect("should resolve Junrei");
        assert_eq!(junrei.slug, "junrei", "role-stripped slug expected");

        // The contaminated form also resolves to the same slug
        let junlei_full = resolver
            .resolve("Junlei, tech lead")
            .expect("contaminated form should still resolve");
        assert_eq!(junlei_full.slug, "junlei");
    }
}
