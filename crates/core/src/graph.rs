use crate::config::Config;
use crate::markdown::{split_frontmatter, ContentType, Frontmatter};
use crate::person_identity::PersonCanonicalizer;
use chrono::Local;
use rusqlite::{params, Connection};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use thiserror::Error;
use walkdir::WalkDir;

// ──────────────────────────────────────────────────────────────
// Conversation graph: SQLite index derived from meeting markdown.
//
// Markdown is the source of truth. The SQLite index at
// ~/.minutes/graph.db is a derived, rebuildable cache that
// enables instant relationship queries.
//
//   ~/meetings/*.md ──parse──▶ graph.db ──query──▶ MCP / CLI
//       (canonical)             (derived)          (consumers)
//
// If graph.db is deleted, `minutes people --rebuild`
// regenerates it from markdown in <1s for 1000 meetings.
// ──────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("meetings directory does not exist: {0}")]
    DirNotFound(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphStats {
    pub people_count: usize,
    pub meeting_count: usize,
    pub commitment_count: usize,
    pub topic_count: usize,
    pub alias_suggestions: Vec<AliasSuggestion>,
    pub rebuild_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PersonSummary {
    pub slug: String,
    pub name: String,
    pub meeting_count: i64,
    pub last_seen: String,
    pub days_since: f64,
    pub open_commitments: i64,
    pub top_topics: Vec<String>,
    pub score: f64,
    pub losing_touch: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Commitment {
    pub text: String,
    pub status: String,
    pub due_date: Option<String>,
    pub created_at: String,
    pub commitment_type: String,
    pub meeting_title: String,
    pub meeting_date: String,
    pub person_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AliasSuggestion {
    pub name_a: String,
    pub name_b: String,
    pub shared_meetings: usize,
}

/// Database path: ~/.minutes/graph.db
pub fn db_path() -> PathBuf {
    let base = dirs::home_dir()
        .expect("home directory must exist for graph.db")
        .join(".minutes");
    std::fs::create_dir_all(&base).ok();
    base.join("graph.db")
}

/// Set 0600 permissions on the database file (meeting data is sensitive).
#[cfg(unix)]
fn set_db_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if path.exists() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
    }
}

fn merge_person_aliases(existing: &mut Vec<String>, incoming: &[String]) {
    let mut seen: HashSet<String> = existing
        .iter()
        .map(|alias| alias.to_ascii_lowercase())
        .collect();
    for alias in incoming {
        let trimmed = alias.trim();
        if trimmed.is_empty() {
            continue;
        }

        let key = trimmed.to_ascii_lowercase();
        if seen.insert(key) {
            existing.push(trimmed.to_string());
        }
    }
}

fn person_role_priority(role: &str) -> u8 {
    match role {
        "attendee" => 3,
        "speaker" => 2,
        "mentioned" => 1,
        _ => 0,
    }
}

fn push_file_person(
    file_people: &mut Vec<(String, String, Vec<String>, &'static str)>,
    slug: String,
    name: String,
    aliases: Vec<String>,
    role: &'static str,
) {
    if slug.is_empty() {
        return;
    }

    if let Some((_, existing_name, existing_aliases, existing_role)) = file_people
        .iter_mut()
        .find(|(existing_slug, _, _, _)| *existing_slug == slug)
    {
        if name.trim().len() > existing_name.trim().len() {
            *existing_name = name;
        }
        merge_person_aliases(existing_aliases, &aliases);
        if person_role_priority(role) > person_role_priority(existing_role) {
            *existing_role = role;
        }
        return;
    }

    file_people.push((slug, name, aliases, role));
}

#[cfg(not(unix))]
fn set_db_permissions(_path: &Path) {}

/// Calculate relationship score from meeting count, recency, and topic depth.
fn relationship_score(meeting_count: i64, days_since: f64, topic_count: usize) -> f64 {
    let recency_weight = 1.0 / (1.0 + days_since / 30.0);
    let topic_depth = (topic_count as f64 / 3.0).min(1.0);
    meeting_count as f64 * recency_weight * topic_depth
}

/// Open or create the SQLite database with schema.
fn open_db(path: &Path) -> Result<Connection, GraphError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    create_schema(&conn)?;
    Ok(conn)
}

fn create_schema(conn: &Connection) -> Result<(), GraphError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS people (
            id INTEGER PRIMARY KEY,
            slug TEXT UNIQUE NOT NULL,
            name TEXT NOT NULL,
            aliases TEXT DEFAULT '[]',
            first_seen TEXT NOT NULL,
            last_seen TEXT NOT NULL,
            meeting_count INTEGER DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS meetings (
            id INTEGER PRIMARY KEY,
            path TEXT UNIQUE NOT NULL,
            title TEXT NOT NULL,
            date TEXT NOT NULL,
            duration_secs INTEGER,
            content_type TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS people_meetings (
            person_id INTEGER REFERENCES people(id),
            meeting_id INTEGER REFERENCES meetings(id),
            role TEXT DEFAULT 'attendee',
            PRIMARY KEY (person_id, meeting_id)
        );
        CREATE TABLE IF NOT EXISTS commitments (
            id INTEGER PRIMARY KEY,
            meeting_id INTEGER REFERENCES meetings(id),
            person_id INTEGER,
            text TEXT NOT NULL,
            status TEXT DEFAULT 'open',
            due_date TEXT,
            created_at TEXT NOT NULL,
            commitment_type TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS topics (
            id INTEGER PRIMARY KEY,
            name TEXT UNIQUE NOT NULL
        );
        CREATE TABLE IF NOT EXISTS meeting_topics (
            meeting_id INTEGER REFERENCES meetings(id),
            topic_id INTEGER REFERENCES topics(id),
            PRIMARY KEY (meeting_id, topic_id)
        );
        CREATE INDEX IF NOT EXISTS idx_people_slug ON people(slug);
        CREATE INDEX IF NOT EXISTS idx_people_last_seen ON people(last_seen);
        CREATE INDEX IF NOT EXISTS idx_meetings_date ON meetings(date);
        CREATE INDEX IF NOT EXISTS idx_commitments_status ON commitments(status);
        CREATE INDEX IF NOT EXISTS idx_commitments_person ON commitments(person_id);",
    )?;
    Ok(())
}

pub fn parakeet_boost_phrases(limit: usize) -> Result<Vec<String>, GraphError> {
    if limit == 0 {
        return Ok(Vec::new());
    }

    let conn = open_db(&db_path())?;
    let mut phrases = Vec::new();
    let mut seen = std::collections::HashSet::new();

    let mut people_stmt = conn.prepare(
        "SELECT slug, name
         FROM people
         ORDER BY meeting_count DESC, last_seen DESC
         LIMIT 200",
    )?;
    let people_rows = people_stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in people_rows {
        let (slug, name) = row?;
        if let Some(phrase) = normalize_boost_phrase(&name, Some(&slug)) {
            push_unique_phrase(&mut phrases, &mut seen, phrase, limit);
        }
        if phrases.len() >= limit {
            return Ok(phrases);
        }
    }

    let mut meeting_stmt = conn.prepare(
        "SELECT title
         FROM meetings
         ORDER BY date DESC
         LIMIT 200",
    )?;
    let meeting_rows = meeting_stmt.query_map([], |row| row.get::<_, String>(0))?;
    for row in meeting_rows {
        let title = row?;
        for fragment in split_boost_title_fragments(&title) {
            if let Some(phrase) = normalize_boost_phrase(&fragment, None) {
                push_unique_phrase(&mut phrases, &mut seen, phrase, limit);
            }
            if phrases.len() >= limit {
                return Ok(phrases);
            }
        }
    }

    Ok(phrases)
}

fn push_unique_phrase(
    phrases: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
    phrase: String,
    limit: usize,
) {
    if phrases.len() >= limit {
        return;
    }
    let key = phrase.to_lowercase();
    if seen.insert(key) {
        phrases.push(phrase);
    }
}

fn normalize_boost_phrase(phrase: &str, slug: Option<&str>) -> Option<String> {
    let phrase = phrase.trim().trim_matches(|c: char| c == '"' || c == '\'');
    if phrase.len() < 3 {
        return None;
    }

    if let Some(slug) = slug {
        if slug == "unknown"
            || slug == "unknown-speaker"
            || slug.starts_with("speaker-")
            || slug.starts_with("unknown-")
        {
            return None;
        }
    }

    let lower = phrase.to_lowercase();
    if matches!(
        lower.as_str(),
        "unknown" | "unknown speaker" | "speaker 0" | "speaker 1" | "speaker 2" | "speaker 3"
    ) {
        return None;
    }

    let has_signal = phrase
        .chars()
        .any(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    if !has_signal {
        return None;
    }

    Some(phrase.to_string())
}

fn split_boost_title_fragments(title: &str) -> Vec<String> {
    title
        .replace(['—', '&', ','], "|")
        .split('|')
        .flat_map(|part| part.split(" with "))
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

// ── Rebuild ───────────────────────────────────────────────────

/// Rebuild the entire graph index from markdown files.
pub fn rebuild_index(config: &Config) -> Result<GraphStats, GraphError> {
    rebuild_index_at(config, &db_path())
}

/// Rebuild the graph index at a specific database path (for testing).
pub fn rebuild_index_at(config: &Config, path: &Path) -> Result<GraphStats, GraphError> {
    let start = std::time::Instant::now();
    let dir = &config.output_dir;
    if !dir.exists() {
        return Err(GraphError::DirNotFound(dir.display().to_string()));
    }

    // If existing db is corrupted, delete and recreate
    if path.exists()
        && Connection::open(path)
            .and_then(|c| c.execute_batch("SELECT 1 FROM people LIMIT 1"))
            .is_err()
    {
        tracing::warn!("Corrupted graph.db detected, rebuilding from scratch");
        std::fs::remove_file(path).ok();
    }

    let conn = open_db(path)?;

    // Wrap entire rebuild in a transaction for atomicity.
    // If killed mid-rebuild, the old data remains intact.
    conn.execute_batch("BEGIN IMMEDIATE")?;

    // Clear existing data for full rebuild
    conn.execute_batch(
        "DELETE FROM meeting_topics;
         DELETE FROM people_meetings;
         DELETE FROM commitments;
         DELETE FROM meetings;
         DELETE FROM topics;
         DELETE FROM people;",
    )?;

    // Walk all markdown files
    let mut people_map: HashMap<String, (String, Vec<String>)> = HashMap::new(); // slug -> (name, aliases)
    let mut meeting_count = 0usize;
    let mut commitment_count = 0usize;
    let mut topic_set: HashMap<String, i64> = HashMap::new(); // name -> id

    for entry in WalkDir::new(dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "md"))
    {
        let file_path = entry.path();
        let content = match std::fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(path = %file_path.display(), error = %e, "skipping file");
                continue;
            }
        };

        let (fm_str, body) = split_frontmatter(&content);
        if fm_str.is_empty() {
            continue;
        }

        let frontmatter: Frontmatter = match serde_yaml::from_str(fm_str) {
            Ok(fm) => fm,
            Err(_) => {
                // Fallback: try parsing with lenient date handling.
                // Many real files have dates without timezone offsets (e.g., 2026-03-17T14:00:00)
                // which fail DateTime<Local> parsing. Try fixing the date before re-parsing.
                match try_parse_with_fixed_date(fm_str) {
                    Some(fm) => fm,
                    None => {
                        tracing::debug!(path = %file_path.display(), "skipping file with unparseable frontmatter");
                        continue;
                    }
                }
            }
        };

        let content_type_str = match frontmatter.r#type {
            ContentType::Meeting => "meeting",
            ContentType::Memo => "memo",
            ContentType::Dictation => "dictation",
        };
        let date_str = frontmatter.date.to_rfc3339();
        let duration_secs = parse_duration_secs(&frontmatter.duration);

        // Insert meeting
        conn.execute(
            "INSERT OR IGNORE INTO meetings (path, title, date, duration_secs, content_type) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![file_path.to_string_lossy().as_ref(), frontmatter.title, date_str, duration_secs, content_type_str],
        )?;
        let meeting_id: i64 = conn.query_row(
            "SELECT id FROM meetings WHERE path = ?1",
            params![file_path.to_string_lossy().as_ref()],
            |row| row.get(0),
        )?;
        meeting_count += 1;

        let speakers = extract_speakers_from_transcript(body);
        let normalized_attendees = frontmatter.normalized_attendees();
        let context_names: Vec<&str> = normalized_attendees
            .iter()
            .map(String::as_str)
            .chain(frontmatter.people.iter().map(String::as_str))
            .chain(speakers.iter().map(String::as_str))
            .chain(
                frontmatter
                    .speaker_map
                    .iter()
                    .filter(|attr| attr.confidence == crate::diarize::Confidence::High)
                    .map(|attr| attr.name.as_str()),
            )
            .chain(
                frontmatter
                    .action_items
                    .iter()
                    .map(|item| item.assignee.as_str()),
            )
            .chain(
                frontmatter
                    .intents
                    .iter()
                    .filter_map(|intent| intent.who.as_deref()),
            )
            .collect();
        let canonicalizer = PersonCanonicalizer::new(&frontmatter.entities.people, context_names);

        // Extract people from multiple sources
        let mut file_people: Vec<(String, String, Vec<String>, &'static str)> = Vec::new(); // (slug, name, aliases, role)

        // Source 1: frontmatter.attendees
        for attendee in normalized_attendees {
            if let Some(identity) = canonicalizer.resolve(&attendee) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "attendee",
                );
            }
        }

        // Source 2: frontmatter.people
        for person in &frontmatter.people {
            if let Some(identity) = canonicalizer.resolve(person) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "mentioned",
                );
            }
        }

        // Source 3: entities.people (richest — has slug + aliases)
        for entity in &frontmatter.entities.people {
            if let Some(identity) = canonicalizer.resolve_entity(entity) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "attendee",
                );
            }
        }

        // Source 4: transcript speaker labels [NAME HH:MM] or [NAME M:SS]
        for speaker in &speakers {
            if let Some(identity) = canonicalizer.resolve(speaker) {
                push_file_person(
                    &mut file_people,
                    identity.slug,
                    identity.name,
                    identity.aliases,
                    "speaker",
                );
            }
        }

        // Source 5: speaker_map (confirmed speaker attributions)
        for attr in &frontmatter.speaker_map {
            if attr.confidence == crate::diarize::Confidence::High {
                if let Some(identity) = canonicalizer.resolve(&attr.name) {
                    push_file_person(
                        &mut file_people,
                        identity.slug,
                        identity.name,
                        identity.aliases,
                        "speaker",
                    );
                }
            }
        }

        // Insert/update people and link to meeting
        for (slug, name, aliases, role) in &file_people {
            let aliases_json = serde_json::to_string(aliases).unwrap_or_else(|_| "[]".into());

            // Upsert person
            conn.execute(
                "INSERT INTO people (slug, name, aliases, first_seen, last_seen, meeting_count)
                 VALUES (?1, ?2, ?3, ?4, ?4, 1)
                 ON CONFLICT(slug) DO UPDATE SET
                   last_seen = CASE WHEN ?4 > last_seen THEN ?4 ELSE last_seen END,
                   first_seen = CASE WHEN ?4 < first_seen THEN ?4 ELSE first_seen END,
                   meeting_count = meeting_count + 1,
                   aliases = CASE WHEN length(?3) > length(aliases) THEN ?3 ELSE aliases END",
                params![slug, name, aliases_json, date_str],
            )?;

            let person_id: i64 = conn.query_row(
                "SELECT id FROM people WHERE slug = ?1",
                params![slug],
                |row| row.get(0),
            )?;

            // Link person to meeting
            conn.execute(
                "INSERT OR IGNORE INTO people_meetings (person_id, meeting_id, role) VALUES (?1, ?2, ?3)",
                params![person_id, meeting_id, role],
            )?;

            people_map
                .entry(slug.clone())
                .or_insert_with(|| (name.clone(), aliases.clone()));
        }

        // Extract commitments from action_items
        for item in &frontmatter.action_items {
            let person_id = canonicalizer.resolve(&item.assignee).and_then(|identity| {
                conn.query_row(
                    "SELECT id FROM people WHERE slug = ?1",
                    params![identity.slug],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
            });
            conn.execute(
                "INSERT INTO commitments (meeting_id, person_id, text, status, due_date, created_at, commitment_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'action_item')",
                params![meeting_id, person_id, item.task, item.status, item.due, date_str],
            )?;
            commitment_count += 1;
        }

        // Extract commitments from intents
        for intent in &frontmatter.intents {
            let person_id = intent.who.as_ref().and_then(|who| {
                let identity = canonicalizer.resolve(who)?;
                conn.query_row(
                    "SELECT id FROM people WHERE slug = ?1",
                    params![identity.slug],
                    |row| row.get::<_, i64>(0),
                )
                .ok()
            });
            conn.execute(
                "INSERT INTO commitments (meeting_id, person_id, text, status, due_date, created_at, commitment_type)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'intent')",
                params![meeting_id, person_id, intent.what, intent.status, intent.by_date, date_str],
            )?;
            commitment_count += 1;
        }

        // Extract commitments from decisions (no owner)
        for decision in &frontmatter.decisions {
            conn.execute(
                "INSERT INTO commitments (meeting_id, person_id, text, status, due_date, created_at, commitment_type)
                 VALUES (?1, NULL, ?2, 'decided', NULL, ?3, 'decision')",
                params![meeting_id, decision.text, date_str],
            )?;
            commitment_count += 1;
        }

        // Extract lightweight commitments from transcript patterns
        let transcript_commitments = extract_commitments_from_transcript(body);
        for (text, _) in &transcript_commitments {
            conn.execute(
                "INSERT INTO commitments (meeting_id, person_id, text, status, due_date, created_at, commitment_type)
                 VALUES (?1, NULL, ?2, 'open', NULL, ?3, 'intent')",
                params![meeting_id, text, date_str],
            )?;
            commitment_count += 1;
        }

        // Extract topics from tags, decisions, and title
        let mut file_topics: Vec<String> = Vec::new();
        for tag in &frontmatter.tags {
            file_topics.push(tag.to_lowercase());
        }
        for decision in &frontmatter.decisions {
            if let Some(topic) = &decision.topic {
                file_topics.push(topic.to_lowercase());
            }
        }
        // Title keywords (words > 3 chars, skip common words)
        for word in extract_title_keywords(&frontmatter.title) {
            file_topics.push(word);
        }
        if let Some(cal) = &frontmatter.calendar_event {
            for word in extract_title_keywords(cal) {
                file_topics.push(word);
            }
        }

        file_topics.sort();
        file_topics.dedup();

        for topic_name in &file_topics {
            if !topic_set.contains_key(topic_name) {
                conn.execute(
                    "INSERT OR IGNORE INTO topics (name) VALUES (?1)",
                    params![topic_name],
                )?;
                let tid: i64 = conn.query_row(
                    "SELECT id FROM topics WHERE name = ?1",
                    params![topic_name],
                    |row| row.get(0),
                )?;
                topic_set.insert(topic_name.clone(), tid);
            }
            let tid = topic_set[topic_name];
            conn.execute(
                "INSERT OR IGNORE INTO meeting_topics (meeting_id, topic_id) VALUES (?1, ?2)",
                params![meeting_id, tid],
            )?;
        }
    }

    // Mark stale commitments (only compare dates that look like ISO format, skip "Friday" etc.)
    let today = Local::now().to_rfc3339();
    conn.execute(
        "UPDATE commitments SET status = 'stale'
         WHERE status = 'open' AND due_date IS NOT NULL
         AND due_date GLOB '[0-9][0-9][0-9][0-9]-*'
         AND due_date < ?1",
        params![today],
    )?;

    // Detect alias suggestions
    let alias_suggestions = detect_aliases(&conn)?;

    // Commit the transaction — all or nothing
    conn.execute_batch("COMMIT")?;

    // Set restrictive permissions on the database file
    set_db_permissions(path);

    let elapsed = start.elapsed().as_millis() as u64;
    tracing::info!(
        people = people_map.len(),
        meetings = meeting_count,
        commitments = commitment_count,
        topics = topic_set.len(),
        aliases = alias_suggestions.len(),
        elapsed_ms = elapsed,
        "Index rebuilt"
    );

    Ok(GraphStats {
        people_count: people_map.len(),
        meeting_count,
        commitment_count,
        topic_count: topic_set.len(),
        alias_suggestions,
        rebuild_ms: elapsed,
    })
}

// ── Queries ───────────────────────────────────────────────────

/// Query a person by name or slug — returns rich profile with relationship score.
pub fn query_person(config: &Config, name: &str) -> Result<Option<PersonSummary>, GraphError> {
    let path = db_path();
    if !path.exists() {
        rebuild_index(config)?;
    }
    let conn = open_db(&path)?;
    let slug = slugify(name);

    let result = conn.query_row(
        "SELECT slug, name, meeting_count, last_seen FROM people WHERE slug = ?1",
        params![slug],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        },
    );

    let (slug, person_name, meeting_count, last_seen) = match result {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
        Err(e) => return Err(e.into()),
    };

    let person_id: i64 = conn.query_row(
        "SELECT id FROM people WHERE slug = ?1",
        params![slug],
        |row| row.get(0),
    )?;

    // Top topics
    let mut topic_stmt = conn.prepare(
        "SELECT t.name, COUNT(*) as cnt FROM meeting_topics mt
         JOIN topics t ON mt.topic_id = t.id
         JOIN people_meetings pm ON pm.meeting_id = mt.meeting_id
         WHERE pm.person_id = ?1
         GROUP BY t.name ORDER BY cnt DESC LIMIT 5",
    )?;
    let top_topics: Vec<String> = topic_stmt
        .query_map(params![person_id], |row| row.get::<_, String>(0))?
        .filter_map(|r| r.ok())
        .collect();

    // Open commitments count
    let open_commitments: i64 = conn.query_row(
        "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
        params![person_id],
        |row| row.get(0),
    )?;

    // Relationship score
    let days_since = days_since_date(&last_seen);
    let score = relationship_score(meeting_count, days_since, top_topics.len());
    let losing_touch = meeting_count >= 3 && days_since > 21.0;

    Ok(Some(PersonSummary {
        slug,
        name: person_name,
        meeting_count,
        last_seen,
        days_since,
        open_commitments,
        top_topics,
        score,
        losing_touch,
    }))
}

/// Get all open/stale commitments, optionally filtered by person.
pub fn query_commitments(
    config: &Config,
    person_slug: Option<&str>,
) -> Result<Vec<Commitment>, GraphError> {
    let path = db_path();
    if !path.exists() {
        rebuild_index(config)?;
    }
    let conn = open_db(&path)?;

    let sql = if person_slug.is_some() {
        "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type,
                m.title, m.date, p.name
         FROM commitments c
         JOIN meetings m ON c.meeting_id = m.id
         LEFT JOIN people p ON c.person_id = p.id
         WHERE c.status IN ('open', 'stale') AND p.slug = ?1
         ORDER BY m.date DESC"
    } else {
        "SELECT c.text, c.status, c.due_date, c.created_at, c.commitment_type,
                m.title, m.date, p.name
         FROM commitments c
         JOIN meetings m ON c.meeting_id = m.id
         LEFT JOIN people p ON c.person_id = p.id
         WHERE c.status IN ('open', 'stale')
         ORDER BY m.date DESC"
    };

    let mut stmt = conn.prepare(sql)?;
    let rows = if let Some(slug) = person_slug {
        stmt.query_map(params![slug], map_commitment)?
    } else {
        stmt.query_map([], map_commitment)?
    };

    Ok(rows.filter_map(|r| r.ok()).collect())
}

fn map_commitment(row: &rusqlite::Row) -> rusqlite::Result<Commitment> {
    Ok(Commitment {
        text: row.get(0)?,
        status: row.get(1)?,
        due_date: row.get(2)?,
        created_at: row.get(3)?,
        commitment_type: row.get(4)?,
        meeting_title: row.get(5)?,
        meeting_date: row.get(6)?,
        person_name: row.get(7)?,
    })
}

/// Get all people with relationship scores — the relationship map.
pub fn relationship_map(config: &Config) -> Result<Vec<PersonSummary>, GraphError> {
    let path = db_path();
    if !path.exists() {
        rebuild_index(config)?;
    }
    let conn = open_db(&path)?;

    let mut stmt = conn.prepare(
        "SELECT p.id, p.slug, p.name, p.meeting_count, p.last_seen
         FROM people p
         ORDER BY p.meeting_count DESC",
    )?;

    let mut people: Vec<PersonSummary> = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let person_id: i64 = row.get(0)?;
        let slug: String = row.get(1)?;
        let name: String = row.get(2)?;
        let meeting_count: i64 = row.get(3)?;
        let last_seen: String = row.get(4)?;

        // Top topics for this person
        let top_topics: Vec<String> = conn
            .prepare(
                "SELECT t.name FROM meeting_topics mt
                 JOIN topics t ON mt.topic_id = t.id
                 JOIN people_meetings pm ON pm.meeting_id = mt.meeting_id
                 WHERE pm.person_id = ?1
                 GROUP BY t.name ORDER BY COUNT(*) DESC LIMIT 3",
            )?
            .query_map(params![person_id], |r| r.get::<_, String>(0))?
            .filter_map(|r| r.ok())
            .collect();

        let open_commitments: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
            params![person_id],
            |r| r.get(0),
        )?;

        let days_since = days_since_date(&last_seen);
        let topic_depth = (top_topics.len() as f64 / 3.0).min(1.0);
        let recency_weight = 1.0 / (1.0 + days_since / 30.0);
        let score = meeting_count as f64 * recency_weight * topic_depth;
        let losing_touch = meeting_count >= 3 && days_since > 21.0;

        people.push(PersonSummary {
            slug,
            name,
            meeting_count,
            last_seen,
            days_since,
            open_commitments,
            top_topics,
            score,
            losing_touch,
        });
    }

    // Sort by score descending
    people.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(people)
}

/// Detect people who might be the same person (fuzzy name matching).
fn detect_aliases(conn: &Connection) -> Result<Vec<AliasSuggestion>, GraphError> {
    let mut stmt = conn.prepare("SELECT slug, name FROM people")?;
    let people: Vec<(String, String)> = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();

    let mut suggestions = Vec::new();

    for i in 0..people.len() {
        for j in (i + 1)..people.len() {
            let (_, name_a) = &people[i];
            let (_, name_b) = &people[j];

            if names_likely_same(name_a, name_b) {
                // Check shared meeting count
                let (slug_a, _) = &people[i];
                let (slug_b, _) = &people[j];
                let shared: i64 = conn.query_row(
                    "SELECT COUNT(DISTINCT pm1.meeting_id) FROM people_meetings pm1
                     JOIN people p1 ON pm1.person_id = p1.id
                     JOIN people_meetings pm2 ON pm1.meeting_id = pm2.meeting_id
                     JOIN people p2 ON pm2.person_id = p2.id
                     WHERE p1.slug = ?1 AND p2.slug = ?2",
                    params![slug_a, slug_b],
                    |row| row.get(0),
                )?;

                suggestions.push(AliasSuggestion {
                    name_a: name_a.clone(),
                    name_b: name_b.clone(),
                    shared_meetings: shared as usize,
                });
            }
        }
    }

    Ok(suggestions)
}

// ── Helpers ───────────────────────────────────────────────────

/// Fix common frontmatter issues before YAML parsing:
/// 1. Bare ISO dates without timezone offsets (e.g., `date: 2026-03-17T14:00:00`)
/// 2. Wikilink syntax in people field (e.g., `people: [[alex-chen], [mat]]`)
/// 3. Non-date strings in `due` fields (e.g., `due: Friday`)
fn fix_frontmatter(fm_str: &str) -> String {
    let offset = Local::now().format("%:z").to_string();
    fm_str
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            // Fix bare ISO dates
            if trimmed.starts_with("date:") && trimmed.len() > 5 {
                let value = trimmed[5..].trim();
                if value.contains('T')
                    && !value.contains('+')
                    && !value.contains('Z')
                    && value.chars().filter(|c| *c == '-').count() <= 2
                {
                    return format!("date: {}{}", value, offset);
                }
            }
            // Fix wikilinks in people field:
            // people: [[alex-chen], [mat]] → people: [alex-chen, mat]
            if trimmed.starts_with("people:") && trimmed.contains('[') {
                let colon_pos = line.find(':').unwrap_or(0);
                let key = &line[..=colon_pos];
                let value = line[colon_pos + 1..].replace(['[', ']'], "");
                let items: Vec<String> = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                return format!("{} [{}]", key, items.join(", "));
            }
            // Fix non-date due fields: quote them so they parse as strings
            if trimmed.starts_with("due:") && !trimmed.contains('"') {
                let value = trimmed[4..].trim();
                if !value.is_empty()
                    && !value.starts_with('"')
                    && !value
                        .chars()
                        .next()
                        .map(|c| c.is_ascii_digit())
                        .unwrap_or(false)
                {
                    let indent = line.len() - line.trim_start().len();
                    return format!("{}due: \"{}\"", " ".repeat(indent), value);
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Try parsing frontmatter with fixes applied for real-world format issues.
fn try_parse_with_fixed_date(fm_str: &str) -> Option<Frontmatter> {
    let fixed = fix_frontmatter(fm_str);
    serde_yaml::from_str(&fixed).ok()
}

/// Slugify a name: "Sarah Chen" -> "sarah-chen"
fn slugify(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Parse duration string like "5m 30s" or "1h 2m" into seconds.
fn parse_duration_secs(duration: &str) -> Option<i64> {
    let mut total = 0i64;
    let mut num_buf = String::new();
    for c in duration.chars() {
        if c.is_ascii_digit() {
            num_buf.push(c);
        } else if !num_buf.is_empty() {
            let n: i64 = num_buf.parse().unwrap_or(0);
            match c {
                'h' => total += n * 3600,
                'm' => total += n * 60,
                's' => total += n,
                _ => {}
            }
            num_buf.clear();
        }
    }
    if total > 0 {
        Some(total)
    } else {
        None
    }
}

/// Extract speaker names from transcript lines like "[SARAH 0:45]" or "[MAT 1:20]"
fn extract_speakers_from_transcript(body: &str) -> Vec<String> {
    let mut speakers: Vec<String> = Vec::new();
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('[') {
            if let Some(bracket_end) = rest.find(']') {
                let inside = &rest[..bracket_end];
                // Pattern: NAME followed by timestamp (H:MM or M:SS)
                if let Some(space_pos) = inside.rfind(' ') {
                    let name_part = inside[..space_pos].trim();
                    let time_part = inside[space_pos + 1..].trim();
                    if time_part.contains(':')
                        && time_part.chars().all(|c| c.is_ascii_digit() || c == ':')
                        && !name_part.is_empty()
                    {
                        // Capitalize first letter of each word
                        let name = name_part
                            .split_whitespace()
                            .map(|w| {
                                let mut chars = w.chars();
                                match chars.next() {
                                    Some(first) => {
                                        first.to_uppercase().collect::<String>()
                                            + &chars.as_str().to_lowercase()
                                    }
                                    None => String::new(),
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(" ");
                        if !speakers.contains(&name) {
                            speakers.push(name);
                        }
                    }
                }
            }
        }
    }
    speakers
}

/// Extract lightweight commitments from transcript text patterns.
fn extract_commitments_from_transcript(body: &str) -> Vec<(String, String)> {
    let patterns = [
        "i'll send",
        "i will send",
        "let me follow up",
        "i'll follow up",
        "action item:",
        "todo:",
        "i'll get",
        "i will get",
        "let me check",
        "i'll look into",
    ];

    let mut commitments = Vec::new();
    for line in body.lines() {
        let lower = line.trim().to_lowercase();
        for pattern in &patterns {
            if lower.contains(pattern) {
                // Clean up the line — remove speaker labels and timestamps
                let clean = line
                    .trim()
                    .trim_start_matches('[')
                    .split(']')
                    .next_back()
                    .unwrap_or(line.trim())
                    .trim();
                if clean.len() > 10 {
                    commitments.push((clean.to_string(), pattern.to_string()));
                    break;
                }
            }
        }
    }
    commitments
}

/// Extract meaningful keywords from a meeting title.
fn extract_title_keywords(title: &str) -> Vec<String> {
    let stopwords = [
        "a",
        "an",
        "and",
        "as",
        "at",
        "by",
        "for",
        "from",
        "in",
        "of",
        "on",
        "or",
        "the",
        "to",
        "with",
        "we",
        "should",
        "will",
        "be",
        "is",
        "are",
        "use",
        "using",
        "meeting",
        "call",
        "sync",
        "chat",
        "discussion",
        "review",
        "update",
        "weekly",
        "daily",
        "standup",
    ];
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 3 && !stopwords.contains(w))
        .map(|w| w.to_string())
        .collect()
}

/// Check if two names likely refer to the same person.
/// "Sarah Chen" and "Sarah" → true (one is prefix of the other)
/// "Sarah" and "Sam" → false
fn names_likely_same(a: &str, b: &str) -> bool {
    let a_lower = a.to_lowercase();
    let b_lower = b.to_lowercase();
    if a_lower == b_lower {
        return false; // Same slug would have been deduped already
    }
    let a_parts: Vec<&str> = a_lower.split_whitespace().collect();
    let b_parts: Vec<&str> = b_lower.split_whitespace().collect();
    let a_first = a_parts.first().copied().unwrap_or("");
    let b_first = b_parts.first().copied().unwrap_or("");
    if a_first.is_empty() || b_first.is_empty() {
        return false;
    }
    // First names must match
    if a_first != b_first {
        return false;
    }
    // If BOTH have last names and they differ → different people
    // "Alex Chen" vs "Alex Kumar" → false (different last names)
    // "Alex Chen" vs "Alex" → true (one is a shortened form)
    if a_parts.len() >= 2 && b_parts.len() >= 2 {
        return a_parts[1] == b_parts[1]; // Same last name = likely same person
    }
    // One has a last name, the other doesn't → likely same person
    a_parts.len() != b_parts.len()
}

/// Calculate days since an RFC3339 date string.
fn days_since_date(date_str: &str) -> f64 {
    chrono::DateTime::parse_from_rfc3339(date_str)
        .map(|dt| {
            let now = Local::now();
            (now.signed_duration_since(dt)).num_hours() as f64 / 24.0
        })
        .unwrap_or(999.0)
}

// ── Tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_config(dir: &Path) -> Config {
        Config {
            output_dir: dir.to_path_buf(),
            ..Config::default()
        }
    }

    /// Rebuild index into a temp db file (avoids test parallelism issues).
    fn rebuild_to_temp(config: &Config, tmp: &TempDir) -> GraphStats {
        let db = tmp.path().join("graph.db");
        rebuild_index_at(config, &db).unwrap()
    }

    fn write_meeting(dir: &Path, filename: &str, content: &str) {
        let path = dir.join(filename);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).ok();
        }
        fs::write(path, content).unwrap();
    }

    const MEETING_1: &str = r#"---
title: Q2 Planning
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 42m
attendees: [Sarah Chen, Alex Kumar]
tags: [planning, roadmap]
action_items:
  - assignee: Alex Kumar
    task: Send tech spec
    due: "2026-03-25"
    status: open
decisions:
  - text: Use SQLite for the graph index
    topic: architecture
intents:
  - kind: commitment
    what: Review pricing grid
    who: Sarah Chen
    status: open
    by_date: "2026-03-22"
---

## Transcript
[SARAH 0:00] So for Q2, I think we should focus on the API
[ALEX 0:45] Right, I'll send the tech spec by Friday
[SARAH 1:20] Perfect, let me follow up on the pricing grid
"#;

    const MEETING_2: &str = r#"---
title: Product Sync
type: meeting
date: 2026-03-22T10:00:00-07:00
duration: 30m
attendees: [Sarah Chen, Jordan Mills]
tags: [product, pricing]
decisions:
  - text: Pricing must pass fairness test
    topic: pricing
---

## Transcript
[SARAH 0:00] Let's discuss the pricing updates
[JORDAN 0:30] I think we need to validate against competitors
"#;

    const MEETING_3: &str = r#"---
title: Onboarding Idea
type: memo
date: 2026-03-21T08:15:00-07:00
duration: 1m 22s
source: voice-memos
tags: [onboarding, product]
---

## Summary
Skip the wizard. Drop users into a pre-populated demo workspace.
"#;

    #[test]
    fn test_rebuild_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();

        // Override db_path for test
        let db = tmp.path().join("test.db");
        let conn = open_db(&db).unwrap();
        // Verify tables exist
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_rebuild_single_meeting() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert!(stats.people_count >= 2); // Sarah + Alex (from attendees + transcript)
        assert_eq!(stats.meeting_count, 1);
        assert!(stats.commitment_count >= 3); // 1 action_item + 1 intent + 1 decision + transcript patterns
    }

    #[test]
    fn test_rebuild_multiple_meetings() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);
        write_meeting(&meetings, "memos/onboarding.md", MEETING_3);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert!(stats.people_count >= 3); // Sarah, Alex, Jordan
        assert_eq!(stats.meeting_count, 3);
        assert!(stats.topic_count >= 3); // planning, roadmap, pricing, product, ...
    }

    #[test]
    fn test_rebuild_malformed_yaml() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "good.md", MEETING_1);
        write_meeting(&meetings, "bad.md", "---\ntitle: [invalid yaml\n---\nbody");

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert_eq!(stats.meeting_count, 1); // Only the good file
    }

    #[test]
    fn test_extract_speakers_from_transcript() {
        let body =
            "[SARAH 0:00] Hello\n[ALEX 0:45] Hi there\n[SARAH 1:20] Let's begin\nNo bracket line";
        let speakers = extract_speakers_from_transcript(body);
        assert_eq!(speakers, vec!["Sarah", "Alex"]);
    }

    #[test]
    fn test_extract_speakers_empty() {
        let body = "Just plain text with no speaker labels.";
        let speakers = extract_speakers_from_transcript(body);
        assert!(speakers.is_empty());
    }

    #[test]
    fn test_extract_commitments_from_transcript() {
        let body = "[ALEX 0:45] Right, I'll send the tech spec by Friday\n[SARAH 1:20] Let me follow up on pricing";
        let commitments = extract_commitments_from_transcript(body);
        assert_eq!(commitments.len(), 2);
        assert!(commitments[0].0.contains("tech spec"));
        assert!(commitments[1].0.contains("pricing"));
    }

    #[test]
    fn test_extract_title_keywords() {
        let keywords = extract_title_keywords("Q2 Planning Discussion with Team");
        assert!(keywords.contains(&"planning".to_string()));
        assert!(!keywords.contains(&"with".to_string())); // stopword
    }

    #[test]
    fn test_names_likely_same() {
        assert!(names_likely_same("Sarah Chen", "Sarah"));
        assert!(names_likely_same("Sarah", "Sarah Chen"));
        assert!(!names_likely_same("Sarah", "Sam"));
        assert!(!names_likely_same("Sarah Chen", "Sarah Chen")); // exact match = already deduped
                                                                 // False positive fix: same first name, different last name = different people
        assert!(!names_likely_same("Alex Chen", "Alex Kumar"));
        assert!(!names_likely_same("Jordan Mills", "Jordan Lee"));
        // Both have same first + last (case insensitive) = same slug, already deduped
        assert!(!names_likely_same("Sarah Chen", "Sarah chen"));
        // Different last name initials are different people
        assert!(!names_likely_same("Sarah C.", "Sarah Chen"));
    }

    #[test]
    fn test_slugify() {
        assert_eq!(slugify("Sarah Chen"), "sarah-chen");
        assert_eq!(slugify("Alex  Kumar"), "alex-kumar");
        assert_eq!(slugify("  Mat  "), "mat");
    }

    #[test]
    fn test_parse_duration_secs() {
        assert_eq!(parse_duration_secs("42m"), Some(2520));
        assert_eq!(parse_duration_secs("1h 2m"), Some(3720));
        assert_eq!(parse_duration_secs("5m 30s"), Some(330));
        assert_eq!(parse_duration_secs("1m 22s"), Some(82));
        assert_eq!(parse_duration_secs(""), None);
    }

    #[test]
    fn test_relationship_scoring() {
        // meeting_count=5, days_since=0, topic_depth=1.0 (3+ topics)
        let recency_weight = 1.0 / (1.0 + 0.0 / 30.0); // 1.0
        let topic_depth = (3.0_f64 / 3.0).min(1.0); // 1.0
        let score = 5.0 * recency_weight * topic_depth;
        assert!((score - 5.0).abs() < 0.001);

        // meeting_count=5, days_since=30, topic_depth=0.33 (1 topic)
        let recency_weight = 1.0 / (1.0 + 30.0 / 30.0); // 0.5
        let topic_depth = (1.0_f64 / 3.0).min(1.0); // 0.33
        let score = 5.0 * recency_weight * topic_depth;
        assert!(score < 1.0); // Decayed significantly
    }

    #[test]
    fn test_query_person_not_found() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let result = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = ?1",
                params!["nonexistent-person"],
                |row| row.get::<_, i64>(0),
            )
            .unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_query_person_found() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let (name, count): (String, i64) = conn
            .query_row(
                "SELECT name, meeting_count FROM people WHERE slug = 'sarah-chen'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(name, "Sarah Chen");
        assert_eq!(count, 2);

        // Check open commitments
        let person_id: i64 = conn
            .query_row(
                "SELECT id FROM people WHERE slug = 'sarah-chen'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let open: i64 = conn.query_row(
            "SELECT COUNT(*) FROM commitments WHERE person_id = ?1 AND status IN ('open', 'stale')",
            params![person_id],
            |row| row.get(0),
        ).unwrap();
        assert!(open >= 1, "Sarah should have at least 1 open commitment");
    }

    #[test]
    fn test_query_commitments() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE status IN ('open', 'stale')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(total > 0, "Should have at least 1 open commitment");
    }

    #[test]
    fn test_relationship_map_ordering() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "q2-planning.md", MEETING_1);
        write_meeting(&meetings, "product-sync.md", MEETING_2);
        write_meeting(&meetings, "memos/onboarding.md", MEETING_3);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        // Sarah appears in 2 meetings, should have highest meeting count
        let top: (String, i64) = conn
            .query_row(
                "SELECT name, meeting_count FROM people ORDER BY meeting_count DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(top.0, "Sarah Chen");
        assert_eq!(top.1, 2);
    }

    #[test]
    fn test_relationship_map_includes_attendees_raw_imports() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Imported Granola Meeting
type: meeting
date: 2026-03-24T09:00:00-07:00
duration: 25m
source: granola-import
attendees_raw: Alice Smith (alice@example.com), Bob Brown (bob@example.com)
---

## Notes

Imported notes only.
"#;
        write_meeting(&meetings, "granola.md", meeting);

        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();

        let conn = open_db(&db).unwrap();
        let names: Vec<String> = conn
            .prepare("SELECT name FROM people ORDER BY name")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|row| row.ok())
            .collect();
        assert!(names.contains(&"Alice Smith".to_string()));
        assert!(names.contains(&"Bob Brown".to_string()));
    }

    #[test]
    fn test_alias_detection() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);

        let meeting_sarah_only = r#"---
title: Quick Chat
type: meeting
date: 2026-03-23T09:00:00-07:00
duration: 15m
attendees: [Sarah]
tags: []
---
Short meeting.
"#;
        write_meeting(&meetings, "m2.md", meeting_sarah_only);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        assert!(
            stats.alias_suggestions.iter().any(|s| {
                (s.name_a == "Sarah Chen" && s.name_b == "Sarah")
                    || (s.name_a == "Sarah" && s.name_b == "Sarah Chen")
            }),
            "Expected alias suggestion for Sarah Chen / Sarah, got: {:?}",
            stats.alias_suggestions
        );
    }

    #[test]
    fn test_no_false_positive_aliases() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);

        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);

        assert!(
            !stats.alias_suggestions.iter().any(|s| {
                (s.name_a.contains("Sarah") && s.name_b.contains("Alex"))
                    || (s.name_a.contains("Alex") && s.name_b.contains("Sarah"))
            }),
            "False positive alias detected: {:?}",
            stats.alias_suggestions
        );
    }

    #[test]
    fn test_fix_frontmatter_date() {
        let fm = "title: Test\ntype: meeting\ndate: 2026-03-17T14:00:00\nduration: 5m";
        let fixed = fix_frontmatter(fm);
        // Should have a timezone offset now
        assert!(
            fixed.contains('+') || fixed.contains("-07:00") || fixed.contains("-08:00"),
            "Date should have timezone offset: {}",
            fixed
        );
    }

    #[test]
    fn test_fix_frontmatter_wikilinks() {
        let fm = "title: Test\ntype: meeting\ndate: 2026-03-17T14:00:00-07:00\nduration: 5m\npeople: [[alex-chen], [mat]]";
        let fixed = fix_frontmatter(fm);
        assert!(
            fixed.contains("people: [alex-chen, mat]"),
            "Wikilinks should be flattened: {}",
            fixed
        );
    }

    #[test]
    fn test_fix_frontmatter_due_string() {
        let fm = "  due: Friday";
        let fixed = fix_frontmatter(fm);
        assert!(
            fixed.contains("due: \"Friday\""),
            "Non-date due should be quoted: {}",
            fixed
        );
    }

    #[test]
    fn test_extract_dedup_person() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Dedup Test\ntype: meeting\ndate: 2026-03-20T14:00:00-07:00\nduration: 10m\nattendees: [Sarah]\n---\n\n## Transcript\n[SARAH 0:00] Hello everyone\n";
        write_meeting(&meetings, "dedup.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'sarah'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "Sarah should appear once (deduped)");
    }

    #[test]
    fn test_canonicalizes_attendee_aliases_to_entity_slug() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = r#"---
title: Canonical Dan
type: meeting
date: 2026-03-20T14:00:00-07:00
duration: 10m
attendees: [Dan]
entities:
  people:
    - slug: dan-benamoz
      label: Dan Benamoz
      aliases: [Dan, dan]
action_items:
  - assignee: Dan
    task: Review extraction pass
    status: open
intents:
  - kind: commitment
    what: Follow up with Mat
    who: DAN
    status: open
---

## Transcript
[DAN 0:00] Happy to help
"#;
        write_meeting(&meetings, "canonical-dan.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();

        let canonical_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM people WHERE slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let alias_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM people WHERE slug = 'dan'", [], |r| {
                r.get(0)
            })
            .unwrap();
        let (name, aliases): (String, String) = conn
            .query_row(
                "SELECT name, aliases FROM people WHERE slug = 'dan-benamoz'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let commitment_owner_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments c
                 JOIN people p ON c.person_id = p.id
                 WHERE p.slug = 'dan-benamoz'",
                [],
                |r| r.get(0),
            )
            .unwrap();

        assert_eq!(canonical_count, 1, "canonical person row should exist once");
        assert_eq!(
            alias_count, 0,
            "raw alias slug should not be written separately"
        );
        assert_eq!(name, "Dan Benamoz");
        assert!(aliases.contains("Dan"));
        assert!(
            commitment_owner_count >= 2,
            "action items and intents should resolve to canonical person"
        );
    }

    #[test]
    fn test_commitment_staleness_detection() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Stale Test\ntype: meeting\ndate: 2026-01-01T10:00:00-07:00\nduration: 30m\nattendees: [Alex]\nintents:\n  - kind: commitment\n    what: Deliver the report\n    who: Alex\n    status: open\n    by_date: \"2026-01-15\"\n---\nContent.\n";
        write_meeting(&meetings, "stale.md", meeting);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE status = 'stale'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(stale >= 1, "Past-due commitment should be stale");
    }

    #[test]
    fn test_no_transcript_section() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        let meeting = "---\ntitle: Memo Only\ntype: memo\ndate: 2026-03-20T10:00:00-07:00\nduration: 1m\ntags: [idea]\n---\n\n## Summary\nJust a summary.\n";
        write_meeting(&meetings, "memo.md", meeting);
        let config = test_config(&meetings);
        let stats = rebuild_to_temp(&config, &tmp);
        assert_eq!(stats.meeting_count, 1);
        assert_eq!(stats.people_count, 0);
        assert!(stats.topic_count >= 1); // "idea" from tags
    }

    #[test]
    fn test_corrupted_db_auto_rebuild() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);
        let db = tmp.path().join("graph.db");
        fs::write(&db, b"not a sqlite database").unwrap();
        let config = test_config(&meetings);
        let stats = rebuild_index_at(&config, &db).unwrap();
        assert_eq!(stats.meeting_count, 1);
        assert!(stats.people_count >= 2);
    }

    #[test]
    fn test_decision_has_null_person() {
        let tmp = TempDir::new().unwrap();
        let meetings = tmp.path().join("meetings");
        fs::create_dir_all(&meetings).unwrap();
        write_meeting(&meetings, "m1.md", MEETING_1);
        let config = test_config(&meetings);
        let db = tmp.path().join("graph.db");
        rebuild_index_at(&config, &db).unwrap();
        let conn = open_db(&db).unwrap();
        let null_decisions: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commitments WHERE commitment_type = 'decision' AND person_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(null_decisions >= 1, "Decisions should have NULL person_id");
    }

    #[test]
    fn normalize_boost_phrase_filters_placeholder_people() {
        assert!(normalize_boost_phrase("Speaker 1", Some("speaker-1")).is_none());
        assert!(normalize_boost_phrase("Unknown speaker", Some("unknown-speaker")).is_none());
        assert_eq!(
            normalize_boost_phrase("Matt Mullenweg", Some("matt-mullenweg")),
            Some("Matt Mullenweg".into())
        );
    }

    #[test]
    fn split_boost_title_fragments_keeps_high_signal_chunks() {
        let parts = split_boost_title_fragments("Wesley Asana, Box & X1 Integration");
        assert_eq!(
            parts,
            vec![
                "Wesley Asana".to_string(),
                "Box".to_string(),
                "X1 Integration".to_string()
            ]
        );
    }
}
