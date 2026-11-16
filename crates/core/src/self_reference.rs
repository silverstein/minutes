//! Canonical references to the recorder in summary output.
//!
//! Speech-to-text often spells the recorder's name the way other people say
//! it ("Matt" for a configured "Mat"), and the summarizer copies that spelling
//! into the title, the prose, action-item owners, `people:` and
//! `entities.people`. The recorder is already known from `[identity]`, so this
//! module maps every configured form of that identity onto one canonical self:
//!
//! - **Display form.** The first word of `identity.name` (the configured short
//!   form, "Mat" for "Mat Silverstein"). Owners, `who` and prose use it. When
//!   the short form is ambiguous in this meeting (see below), the full
//!   `identity.name` is used instead.
//! - **Entity.** One person entity keyed by `slugify(identity.name)` and
//!   labelled `identity.name`, matching the slug the graph and voice profiles
//!   already use for the recorder. Duplicate self entities are merged into it.
//!
//! # Collision rule
//!
//! A configured name alias such as "Matt" can belong to a different real person
//! in another meeting. Name forms (the short form and every configured alias
//! that is not an email) are only canonicalized when both hold:
//!
//! 1. The recorder is evidenced in this recording: a trusted attendee matches
//!    the full name, a full-name variant or a configured email; a speaker-map
//!    entry or attendee is the full name or a configured email; or the content
//!    is a memo, dictation or note, where the recorder is the speaker.
//! 2. No other known person in the meeting (attendees, summary participants,
//!    speaker-map names) has a first name that starts with that form. "Matt
//!    Jones" or "matthew@example.com" attending blocks "Matt" (and "Mat").
//!
//! The full `identity.name` and configured emails are always unambiguous and
//! are always canonicalized. Free prose is only rewritten for whole-word,
//! case-sensitive matches of an active configured alias; the full name and
//! unconfigured spellings are never touched.

use crate::config::IdentityConfig;
use crate::knowledge::slugify;
use crate::markdown::{ActionItem, Decision, EntityLinks, EntityRef, Intent};
use std::collections::BTreeSet;

/// What the pipeline knows about who was in the recording.
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SelfReferenceEvidence<'a> {
    /// Calendar or otherwise trusted attendees (not summarizer guesses).
    pub trusted_attendees: &'a [String],
    /// Every other name the meeting knows about, including summary
    /// participants merged into the attendee list.
    pub known_names: &'a [String],
    /// Names from the speaker map.
    pub speaker_names: &'a [String],
    /// True for memos, dictation and notes, where the recorder is the speaker.
    pub recorder_is_speaker: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SelfReferenceResolver {
    full_name: String,
    short_name: String,
    surname: Option<String>,
    /// Lowercased configured emails.
    emails: Vec<String>,
    /// Every configured name form other than the full name (short form and
    /// non-email aliases). Used to tell the recorder apart from other people.
    configured_names: Vec<String>,
    /// The configured name forms that may be canonicalized in this meeting.
    active_names: Vec<String>,
}

impl SelfReferenceResolver {
    /// Returns `None` when no `identity.name` is configured.
    pub(crate) fn new(
        identity: &IdentityConfig,
        evidence: SelfReferenceEvidence<'_>,
    ) -> Option<Self> {
        let full_name = identity
            .name
            .as_deref()
            .map(collapse_whitespace)
            .filter(|name| !name.is_empty())?;
        let mut tokens = full_name.split(' ');
        let short_name = tokens.next()?.to_string();
        let rest: Vec<&str> = tokens.collect();
        let surname = (!rest.is_empty()).then(|| rest.join(" "));

        let mut emails = Vec::new();
        let mut configured_names = Vec::new();
        let push_name = |names: &mut Vec<String>, value: String| {
            if value.is_empty()
                || value.eq_ignore_ascii_case(&full_name)
                || names
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(&value))
            {
                return;
            }
            names.push(value);
        };
        push_name(&mut configured_names, short_name.clone());
        for alias in identity.all_user_aliases() {
            let alias = collapse_whitespace(&alias);
            if is_email(&alias) {
                let lower = alias.to_lowercase();
                if !emails.contains(&lower) {
                    emails.push(lower);
                }
            } else {
                push_name(&mut configured_names, alias);
            }
        }

        let mut resolver = Self {
            full_name,
            short_name,
            surname,
            emails,
            configured_names,
            active_names: Vec::new(),
        };

        if resolver.recorder_evidenced(&evidence) {
            let others = resolver.other_first_names(&evidence);
            resolver.active_names = resolver
                .configured_names
                .iter()
                .filter(|form| !collides(form, &others))
                .cloned()
                .collect();
        }
        Some(resolver)
    }

    /// The spelling used for the recorder in owners, `who` and prose.
    pub(crate) fn display_name(&self) -> &str {
        if self.is_active(&self.short_name) {
            &self.short_name
        } else {
            &self.full_name
        }
    }

    pub(crate) fn canonical_slug(&self) -> String {
        slugify(&self.full_name)
    }

    /// Canonical display form when `raw` names the recorder, else `None`.
    /// A trailing confidence hint such as `(SPEAKER_0)` is preserved.
    pub(crate) fn canonical_person(&self, raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        let (head, suffix) = split_trailing_parenthetical(trimmed);
        let head = head.trim().trim_start_matches('@').trim();
        if !self.names_recorder(head) {
            return None;
        }
        Some(format!("{}{}", self.display_name(), suffix))
    }

    /// Rewrite whole-word, case-sensitive occurrences of active configured
    /// aliases in free text to the display form.
    pub(crate) fn canonicalize_text(&self, text: &str) -> String {
        let display = self.display_name();
        let mut forms: Vec<&str> = self
            .active_names
            .iter()
            .map(String::as_str)
            .filter(|form| *form != display)
            .collect();
        // Longer forms first so a multi-word alias wins over its first word.
        forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        let mut out = text.to_string();
        for form in forms {
            out = replace_whole_word(&out, form, display);
        }
        out
    }

    pub(crate) fn canonicalize_action_items(&self, items: &mut [ActionItem]) {
        for item in items {
            if let Some(owner) = self.canonical_person(&item.assignee) {
                item.assignee = owner;
            }
            item.task = self.canonicalize_text(&item.task);
        }
    }

    pub(crate) fn canonicalize_intents(&self, intents: &mut [Intent]) {
        for intent in intents {
            if let Some(owner) = intent
                .who
                .as_deref()
                .and_then(|who| self.canonical_person(who))
            {
                intent.who = Some(owner);
            }
            intent.what = self.canonicalize_text(&intent.what);
        }
    }

    pub(crate) fn canonicalize_decisions(&self, decisions: &mut [Decision]) {
        for decision in decisions {
            decision.text = self.canonicalize_text(&decision.text);
            if let Some(topic) = decision.topic.as_mut() {
                *topic = self.canonicalize_text(topic);
            }
        }
    }

    pub(crate) fn canonicalize_summary(&self, summary: &mut crate::summarize::Summary) {
        if let Some(title) = summary.title.as_mut() {
            *title = self.canonicalize_text(title);
        }
        summary.text = self.canonicalize_text(&summary.text);
        for list in [
            &mut summary.decisions,
            &mut summary.action_items,
            &mut summary.open_questions,
            &mut summary.commitments,
            &mut summary.key_points,
        ] {
            for line in list.iter_mut() {
                *line = self.canonicalize_text(line);
            }
        }
        let mut participants: Vec<String> = Vec::new();
        for participant in &summary.participants {
            let value = self
                .canonical_person(participant)
                .unwrap_or_else(|| self.canonicalize_text(participant));
            if !participants
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(&value))
            {
                participants.push(value);
            }
        }
        summary.participants = participants;
    }

    /// Merge every person entity that names the recorder into one entity
    /// keyed by `slugify(identity.name)`. Never invents the recorder when no
    /// self entity is present.
    pub(crate) fn canonicalize_entities(&self, entities: &mut EntityLinks) {
        let canonical_slug = self.canonical_slug();
        if canonical_slug.is_empty() {
            return;
        }
        let mut aliases = BTreeSet::new();
        let mut found = false;
        entities.people.retain(|entity| {
            if self.is_self_entity(entity, &canonical_slug) {
                found = true;
                aliases.insert(entity.label.to_lowercase());
                aliases.extend(entity.aliases.iter().map(|alias| alias.to_lowercase()));
                false
            } else {
                true
            }
        });
        if !found {
            return;
        }
        aliases.insert(self.full_name.to_lowercase());
        aliases.insert(self.display_name().to_lowercase());
        entities.people.push(EntityRef {
            slug: canonical_slug,
            label: self.full_name.clone(),
            aliases: aliases.into_iter().collect(),
        });
        entities.people.sort_by(|a, b| a.slug.cmp(&b.slug));
    }

    fn is_self_entity(&self, entity: &EntityRef, canonical_slug: &str) -> bool {
        entity.slug == canonical_slug
            || self.names_recorder(&entity.label)
            || entity
                .aliases
                .iter()
                .any(|alias| self.names_recorder(alias))
    }

    fn is_active(&self, form: &str) -> bool {
        self.active_names
            .iter()
            .any(|active| active.eq_ignore_ascii_case(form))
    }

    fn full_name_variant(&self, value: &str, forms: &[String]) -> bool {
        let Some(surname) = self.surname.as_deref() else {
            return false;
        };
        forms
            .iter()
            .any(|form| value.eq_ignore_ascii_case(&format!("{form} {surname}")))
    }

    /// Unambiguous self reference: the full name, a configured alias followed
    /// by the surname, or a configured email.
    fn strongly_names_recorder(&self, raw: &str) -> bool {
        let value = collapse_whitespace(raw);
        if value.is_empty() {
            return false;
        }
        if is_email(&value) {
            return self.emails.contains(&value.to_lowercase());
        }
        value.eq_ignore_ascii_case(&self.full_name)
            || self.full_name_variant(&value, &self.configured_names)
    }

    /// Any reference that names the recorder in this meeting.
    fn names_recorder(&self, raw: &str) -> bool {
        if self.strongly_names_recorder(raw) {
            return true;
        }
        let value = collapse_whitespace(raw);
        self.is_active(&value) || self.full_name_variant(&value, &self.active_names)
    }

    /// Whether `raw` is any configured self form, active or not.
    fn is_configured_self_form(&self, raw: &str) -> bool {
        let value = collapse_whitespace(raw);
        self.strongly_names_recorder(&value)
            || self
                .configured_names
                .iter()
                .any(|form| form.eq_ignore_ascii_case(&value))
    }

    fn recorder_evidenced(&self, evidence: &SelfReferenceEvidence<'_>) -> bool {
        if evidence.recorder_is_speaker {
            return true;
        }
        evidence
            .trusted_attendees
            .iter()
            .chain(evidence.known_names)
            .chain(evidence.speaker_names)
            .map(|name| strip_parenthetical(name))
            .any(|name| self.strongly_names_recorder(name))
    }

    /// Lowercased first names of everyone in the meeting who is not the
    /// recorder.
    fn other_first_names(&self, evidence: &SelfReferenceEvidence<'_>) -> Vec<String> {
        let mut out = Vec::new();
        for raw in evidence
            .trusted_attendees
            .iter()
            .chain(evidence.known_names)
            .chain(evidence.speaker_names)
        {
            let name = collapse_whitespace(strip_parenthetical(raw).trim_start_matches('@'));
            if name.is_empty() || is_speaker_label(&name) || self.is_configured_self_form(&name) {
                continue;
            }
            let person = match name.split_once('@') {
                Some((local, domain)) if !local.is_empty() && domain.contains('.') => local
                    .split(['.', '_', '-', '+'])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => name,
            };
            if self.is_configured_self_form(&person) {
                continue;
            }
            if let Some(first) = person
                .split_whitespace()
                .next()
                .map(|token| token.trim_matches(|c: char| !c.is_alphanumeric()))
                .filter(|token| !token.is_empty())
            {
                out.push(first.to_lowercase());
            }
        }
        out
    }
}

/// A configured form collides when another person's first name starts with
/// it (case-insensitive). Prefix matching is deliberately conservative:
/// "Matthew" attending blocks rewriting "Matt".
fn collides(form: &str, other_first_names: &[String]) -> bool {
    let Some(first) = form.split_whitespace().next() else {
        return false;
    };
    let first = first.to_lowercase();
    other_first_names
        .iter()
        .any(|other| other.starts_with(&first))
}

fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_email(value: &str) -> bool {
    match value.split_once('@') {
        Some((local, domain)) => {
            !local.is_empty() && domain.contains('.') && !value.contains(char::is_whitespace)
        }
        None => false,
    }
}

fn is_speaker_label(value: &str) -> bool {
    let upper = value.to_ascii_uppercase();
    upper.starts_with("SPEAKER_") || upper.starts_with("SPEAKER ") || upper == "UNKNOWN"
}

/// Split "Name (SPEAKER_0)" into ("Name", " (SPEAKER_0)").
fn split_trailing_parenthetical(value: &str) -> (&str, &str) {
    if value.ends_with(')') {
        if let Some(open) = value.rfind(" (") {
            return (&value[..open], &value[open..]);
        }
    }
    (value, "")
}

fn strip_parenthetical(value: &str) -> &str {
    split_trailing_parenthetical(value.trim()).0
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Replace whole-word, case-sensitive occurrences of `needle`. A match is
/// skipped when it sits inside a word, an email, a domain, a path or a
/// hyphenated compound ("Matt-Pierre", "matt.dev", "Matt@x.com").
fn replace_whole_word(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() || !haystack.contains(needle) {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut last = 0;
    for (start, _) in haystack.match_indices(needle) {
        if start < last {
            continue;
        }
        let end = start + needle.len();
        let before = &haystack[..start];
        let after = &haystack[end..];
        let mut before_chars = before.chars().rev();
        let prev = before_chars.next();
        let prev2 = before_chars.next();
        let mut after_chars = after.chars();
        let next = after_chars.next();
        let next2 = after_chars.next();

        // A leading "@" is an owner mention ("@Matt:") unless it follows a
        // word character, which makes it part of an email address.
        let prev_blocks = match prev {
            None => false,
            Some(c) if is_word_char(c) || c == '/' => true,
            Some('.') | Some('-') | Some('@') => prev2.is_some_and(is_word_char),
            Some(_) => false,
        };
        let next_blocks = match next {
            None => false,
            Some(c) if is_word_char(c) || c == '@' || c == '/' => true,
            Some('.') | Some('-') => next2.is_some_and(is_word_char),
            Some(_) => false,
        };
        if prev_blocks || next_blocks {
            continue;
        }
        out.push_str(&haystack[last..start]);
        out.push_str(replacement);
        last = end;
    }
    out.push_str(&haystack[last..]);
    out
}

/// One instruction block telling the summarizer who recorded the meeting and
/// which spellings refer to them. `None` when no `identity.name` is set.
pub(crate) fn recorder_prompt_block(identity: &IdentityConfig) -> Option<String> {
    let resolver = SelfReferenceResolver::new(identity, SelfReferenceEvidence::default())?;
    let short = &resolver.short_name;
    let mut block = format!(
        "RECORDER: This recording was made by {}. Refer to the recorder as \"{}\" in the title, key points, @person owners and participants.",
        resolver.full_name, short
    );
    let spellings: Vec<String> = resolver
        .configured_names
        .iter()
        .filter(|form| !form.eq_ignore_ascii_case(short))
        .map(|form| format!("\"{form}\""))
        .collect();
    if !spellings.is_empty() {
        block.push_str(&format!(
            " The transcript may spell the recorder's name as {}. Those spellings mean the recorder, so write \"{}\" instead, unless a different person with that name clearly takes part.",
            spellings.join(", "),
            short
        ));
    }
    Some(block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::IntentKind;

    fn identity(name: &str, aliases: &[&str], emails: &[&str]) -> IdentityConfig {
        IdentityConfig {
            name: Some(name.into()),
            email: None,
            emails: emails.iter().map(|value| value.to_string()).collect(),
            aliases: aliases.iter().map(|value| value.to_string()).collect(),
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    fn sample_identity() -> IdentityConfig {
        identity(
            "Robin Example",
            &["Robbin", "Robyn", "Robert"],
            &["robin@example.com"],
        )
    }

    fn resolver_for(
        identity: &IdentityConfig,
        trusted: &[String],
        known: &[String],
        speakers: &[String],
    ) -> SelfReferenceResolver {
        SelfReferenceResolver::new(
            identity,
            SelfReferenceEvidence {
                trusted_attendees: trusted,
                known_names: known,
                speaker_names: speakers,
                recorder_is_speaker: false,
            },
        )
        .expect("identity name is configured")
    }

    fn entity(slug: &str, label: &str, aliases: &[&str]) -> EntityRef {
        EntityRef {
            slug: slug.into(),
            label: label.into(),
            aliases: strings(aliases),
        }
    }

    #[test]
    fn alias_in_people_and_entities_merges_into_one_self_entity() {
        let identity = sample_identity();
        let trusted = strings(&["robin@example.com", "Casey Other"]);
        let known = strings(&["Casey Other", "Robyn", "Dana"]);
        let resolver = resolver_for(&identity, &trusted, &known, &[]);

        let mut links = EntityLinks {
            people: vec![
                entity("casey-other", "Casey Other", &["casey other"]),
                entity("dana", "Dana", &["dana"]),
                entity("robin", "Robin", &["robin", "robin@example.com"]),
                entity("robyn", "Robyn", &["robyn"]),
            ],
            projects: vec![],
        };
        resolver.canonicalize_entities(&mut links);

        let slugs: Vec<&str> = links.people.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["casey-other", "dana", "robin-example"]);
        let me = &links.people[2];
        assert_eq!(me.label, "Robin Example");
        for alias in ["robin", "robyn", "robin@example.com", "robin example"] {
            assert!(me.aliases.contains(&alias.to_string()), "missing {alias}");
        }
        let people: Vec<&str> = links.people.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(people, vec!["Casey Other", "Dana", "Robin Example"]);
    }

    #[test]
    fn existing_canonical_entity_absorbs_alias_duplicates() {
        let identity = sample_identity();
        let speakers = strings(&["Robin Example"]);
        let resolver = resolver_for(&identity, &[], &[], &speakers);
        let mut links = EntityLinks {
            people: vec![
                entity("robbin", "Robbin", &["robbin"]),
                entity("robin-example", "Robin Example", &["robin example"]),
            ],
            projects: vec![],
        };
        resolver.canonicalize_entities(&mut links);
        assert_eq!(links.people.len(), 1);
        assert_eq!(links.people[0].slug, "robin-example");
        assert!(links.people[0].aliases.contains(&"robbin".to_string()));
    }

    #[test]
    fn assignees_and_who_normalize_to_short_form() {
        let identity = sample_identity();
        let trusted = strings(&["robin@example.com"]);
        let resolver = resolver_for(&identity, &trusted, &[], &[]);

        let mut actions = vec![
            ActionItem {
                assignee: "Robyn".into(),
                task: "Send Casey the deck; Robyn owns follow-up".into(),
                due: None,
                status: "open".into(),
            },
            ActionItem {
                assignee: "Robin Example (SPEAKER_0)".into(),
                task: "Review the draft".into(),
                due: None,
                status: "open".into(),
            },
            ActionItem {
                assignee: "@robin@example.com".into(),
                task: "Book the room".into(),
                due: None,
                status: "open".into(),
            },
            ActionItem {
                assignee: "Casey".into(),
                task: "Update Robbin on progress".into(),
                due: None,
                status: "open".into(),
            },
        ];
        resolver.canonicalize_action_items(&mut actions);
        assert_eq!(actions[0].assignee, "Robin");
        assert_eq!(actions[0].task, "Send Casey the deck; Robin owns follow-up");
        assert_eq!(actions[1].assignee, "Robin (SPEAKER_0)");
        assert_eq!(actions[2].assignee, "Robin");
        assert_eq!(actions[3].assignee, "Casey");
        assert_eq!(actions[3].task, "Update Robin on progress");

        let mut intents = vec![Intent {
            kind: IntentKind::Commitment,
            what: "Robert will share the link".into(),
            who: Some("Robert".into()),
            status: "open".into(),
            by_date: None,
        }];
        resolver.canonicalize_intents(&mut intents);
        assert_eq!(intents[0].who.as_deref(), Some("Robin"));
        assert_eq!(intents[0].what, "Robin will share the link");
    }

    #[test]
    fn prose_rewrite_is_whole_word_and_case_sensitive() {
        let identity = sample_identity();
        let resolver = resolver_for(&identity, &strings(&["robin@example.com"]), &[], &[]);
        assert_eq!(
            resolver.canonicalize_text("Casey and Robyn on pricing"),
            "Casey and Robin on pricing"
        );
        assert_eq!(
            resolver.canonicalize_text("Robyn's plan, (Robyn) and \"Robyn.\""),
            "Robin's plan, (Robin) and \"Robin.\""
        );
        // Inside words, other casing, emails, domains and compounds stay.
        for untouched in [
            "Robynne joined",
            "ROBYN said",
            "robyn said",
            "mail Robyn@example.org",
            "see robyn.dev or Robyn.dev",
            "Robyn-Lee spoke",
            "the Robertson file",
            "write to ops@Robyn",
            "Robin Example presented",
        ] {
            assert_eq!(resolver.canonicalize_text(untouched), untouched);
        }
    }

    #[test]
    fn no_op_when_alias_not_configured() {
        let identity = identity("Robin Example", &[], &["robin@example.com"]);
        let trusted = strings(&["robin@example.com"]);
        let resolver = resolver_for(&identity, &trusted, &[], &[]);
        assert_eq!(
            resolver.canonicalize_text("Robyn will send it"),
            "Robyn will send it"
        );
        assert_eq!(resolver.canonical_person("Robyn"), None);
        let mut links = EntityLinks {
            people: vec![entity("robyn", "Robyn", &["robyn"])],
            projects: vec![],
        };
        resolver.canonicalize_entities(&mut links);
        assert_eq!(links.people.len(), 1);
        assert_eq!(links.people[0].slug, "robyn");
    }

    #[test]
    fn no_identity_name_means_no_resolver() {
        let identity = IdentityConfig {
            aliases: strings(&["Robyn"]),
            ..IdentityConfig::default()
        };
        assert!(SelfReferenceResolver::new(&identity, SelfReferenceEvidence::default()).is_none());
        assert!(recorder_prompt_block(&identity).is_none());
    }

    #[test]
    fn another_attendee_with_alias_name_blocks_that_alias() {
        let identity = sample_identity();
        let trusted = strings(&["robin@example.com", "robert.other@example.org"]);
        let known = strings(&["Robert Other", "Robyn"]);
        let resolver = resolver_for(&identity, &trusted, &known, &[]);

        // "Robert" belongs to someone else here; "Robyn" still resolves.
        assert_eq!(resolver.canonical_person("Robert"), None);
        assert_eq!(resolver.canonical_person("Robyn").as_deref(), Some("Robin"));
        assert_eq!(
            resolver.canonicalize_text("Robert and Robyn met"),
            "Robert and Robin met"
        );
        let mut links = EntityLinks {
            people: vec![
                entity("robert", "Robert", &["robert"]),
                entity("robin", "Robin", &["robin@example.com"]),
            ],
            projects: vec![],
        };
        resolver.canonicalize_entities(&mut links);
        let slugs: Vec<&str> = links.people.iter().map(|e| e.slug.as_str()).collect();
        assert_eq!(slugs, vec!["robert", "robin-example"]);
    }

    #[test]
    fn colliding_short_form_falls_back_to_full_name() {
        let identity = sample_identity();
        let trusted = strings(&["robin@example.com", "Robin Second"]);
        let resolver = resolver_for(&identity, &trusted, &[], &[]);
        assert_eq!(resolver.display_name(), "Robin Example");
        assert_eq!(
            resolver.canonical_person("Robyn").as_deref(),
            Some("Robin Example")
        );
        // The bare short form is ambiguous and stays as written.
        assert_eq!(resolver.canonical_person("Robin"), None);
    }

    #[test]
    fn prefix_collision_is_conservative() {
        let identity = identity("Rob Example", &["Robb"], &["rob@example.com"]);
        let trusted = strings(&["rob@example.com"]);
        let known = strings(&["Robbie"]);
        let resolver = resolver_for(&identity, &trusted, &known, &[]);
        assert_eq!(resolver.canonical_person("Robb"), None);
        assert_eq!(resolver.canonicalize_text("Robb spoke"), "Robb spoke");
    }

    #[test]
    fn no_rewrite_without_recorder_evidence() {
        // A recorded third-party call: the recorder is nowhere in it, so a
        // "Robyn" on that call is someone else.
        let identity = sample_identity();
        let known = strings(&["Robyn", "Casey"]);
        let resolver = resolver_for(&identity, &strings(&["Casey"]), &known, &[]);
        assert_eq!(resolver.canonical_person("Robyn"), None);
        assert_eq!(resolver.canonicalize_text("Robyn spoke"), "Robyn spoke");
        // The full name and configured emails stay unambiguous, and with no
        // evidence the full name is the safe display form.
        assert_eq!(
            resolver.canonical_person("robin@example.com").as_deref(),
            Some("Robin Example")
        );
    }

    #[test]
    fn memo_counts_as_recorder_evidence() {
        let identity = sample_identity();
        let resolver = SelfReferenceResolver::new(
            &identity,
            SelfReferenceEvidence {
                recorder_is_speaker: true,
                ..SelfReferenceEvidence::default()
            },
        )
        .unwrap();
        assert_eq!(resolver.canonical_person("Robyn").as_deref(), Some("Robin"));
    }

    #[test]
    fn summary_fields_and_participants_are_canonicalized() {
        let identity = sample_identity();
        let resolver = resolver_for(&identity, &strings(&["robin@example.com"]), &[], &[]);
        let mut summary = crate::summarize::Summary {
            title: Some("Casey and Robyn on Pricing".into()),
            text: "Robyn walked through the plan.".into(),
            decisions: strings(&["Robyn owns pricing"]),
            action_items: strings(&["@Robyn: send the deck", "@Casey: ping Robyn"]),
            open_questions: vec![],
            commitments: strings(&["@Robbin: review"]),
            key_points: strings(&["Robyn is traveling"]),
            participants: strings(&["Casey", "Robyn", "Robin"]),
        };
        resolver.canonicalize_summary(&mut summary);
        assert_eq!(summary.title.as_deref(), Some("Casey and Robin on Pricing"));
        assert_eq!(summary.text, "Robin walked through the plan.");
        assert_eq!(
            summary.action_items,
            strings(&["@Robin: send the deck", "@Casey: ping Robin"])
        );
        assert_eq!(summary.commitments, strings(&["@Robin: review"]));
        assert_eq!(summary.participants, strings(&["Casey", "Robin"]));
    }

    #[test]
    fn prompt_block_names_recorder_and_aliases() {
        let block = recorder_prompt_block(&sample_identity()).unwrap();
        assert!(block.contains("made by Robin Example"));
        assert!(block.contains("\"Robyn\""));
        assert!(block.contains("\"Robbin\""));
        assert!(!block.contains("robin@example.com"));
        assert!(!block.contains('\u{2014}'));
    }
}
