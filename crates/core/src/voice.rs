use crate::config::Config;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::Serialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

// ──────────────────────────────────────────────────────────────
// Voice profile storage and matching.
//
// Stored in ~/.minutes/voices.db — separate from graph.db
// (which is a rebuildable cache that wipes on rebuild).
// ──────────────────────────────────────────────────────────────

/// Resolve the model version tag for the currently configured embedding model.
/// Falls back to the cam++-lm version string if the config value is unrecognized.
pub fn model_version(config: &Config) -> &'static str {
    crate::diarize::embedding_model_for_config(config).version
}

#[derive(Debug, Error)]
pub enum VoiceError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct VoiceProfile {
    pub person_slug: String,
    pub name: String,
    pub enrolled_at: String,
    pub updated_at: String,
    pub sample_count: u32,
    pub source: String,
    pub model_version: String,
}

pub struct VoiceProfileWithEmbedding {
    pub person_slug: String,
    pub name: String,
    pub embedding: Vec<f32>,
    pub sample_count: u32,
}

pub fn db_path() -> PathBuf {
    let base = dirs::home_dir()
        .expect("home directory must exist")
        .join(".minutes");
    std::fs::create_dir_all(&base).ok();
    base.join("voices.db")
}

pub fn open_db() -> Result<Connection, VoiceError> {
    open_db_at(&db_path())
}

pub fn open_db_at(path: &Path) -> Result<Connection, VoiceError> {
    let conn = Connection::open(path)?;
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS voice_profiles (
            id INTEGER PRIMARY KEY,
            person_slug TEXT UNIQUE NOT NULL,
            name TEXT NOT NULL,
            embedding BLOB NOT NULL,
            enrolled_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            sample_count INTEGER DEFAULT 1,
            source TEXT NOT NULL,
            model_version TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS voice_samples (
            id INTEGER PRIMARY KEY,
            person_slug TEXT NOT NULL,
            name TEXT NOT NULL,
            embedding BLOB NOT NULL,
            embedding_dim INTEGER NOT NULL,
            model_id TEXT NOT NULL,
            normalization TEXT NOT NULL DEFAULT 'l2',
            trust_class TEXT NOT NULL,
            meeting_path TEXT,
            sidecar_speaker TEXT,
            capture_source TEXT,
            speech_seconds REAL NOT NULL DEFAULT 0,
            segment_count INTEGER NOT NULL DEFAULT 0,
            quality_json TEXT,
            similarity REAL,
            top2_margin REAL,
            threshold_version TEXT,
            sensitivity TEXT NOT NULL DEFAULT 'normal',
            created_at TEXT NOT NULL,
            revoked_at TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_voice_samples_slug_model
            ON voice_samples(person_slug, model_id);

        CREATE TABLE IF NOT EXISTS voice_active_profiles (
            person_slug TEXT NOT NULL,
            model_id TEXT NOT NULL,
            name TEXT NOT NULL,
            embedding BLOB NOT NULL,
            embedding_dim INTEGER NOT NULL,
            sample_count INTEGER NOT NULL,
            updated_at TEXT NOT NULL,
            PRIMARY KEY (person_slug, model_id)
        );

        CREATE TRIGGER IF NOT EXISTS voice_samples_prevent_delete
        BEFORE DELETE ON voice_samples
        BEGIN
            SELECT RAISE(ABORT, 'voice samples are immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS voice_samples_prevent_content_update
        BEFORE UPDATE OF person_slug, name, embedding, embedding_dim, model_id,
            normalization, trust_class, meeting_path, sidecar_speaker,
            capture_source, speech_seconds, segment_count, quality_json,
            similarity, top2_margin, threshold_version, sensitivity, created_at
        ON voice_samples
        BEGIN
            SELECT RAISE(ABORT, 'voice samples are immutable');
        END;

        CREATE TRIGGER IF NOT EXISTS voice_samples_prevent_rerevoke
        BEFORE UPDATE OF revoked_at ON voice_samples
        WHEN NEW.revoked_at IS NULL OR OLD.revoked_at IS NOT NULL
        BEGIN
            SELECT RAISE(ABORT, 'voice sample revocation is immutable');
        END;",
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if path.exists() {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok();
        }
    }
    Ok(conn)
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

/// The provenance and confidence class assigned to an immutable voice sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrustClass {
    /// A sample captured through explicit manual enrollment.
    Manual,
    /// A candidate sample explicitly confirmed by a person.
    ManuallyConfirmed,
    /// An unconfirmed candidate inferred from its capture source.
    SourceCandidate,
    /// An unconfirmed candidate proposed by voice matching.
    VoicematchCandidate,
}

impl TrustClass {
    /// Return the stable snake_case database representation.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::ManuallyConfirmed => "manually_confirmed",
            Self::SourceCandidate => "source_candidate",
            Self::VoicematchCandidate => "voicematch_candidate",
        }
    }

    /// Parse a stable snake_case database representation.
    #[allow(clippy::should_implement_trait)] // The WU1 storage API explicitly requires this helper.
    pub fn from_str(value: &str) -> Result<Self, VoiceError> {
        value.parse()
    }
}

impl std::str::FromStr for TrustClass {
    type Err = VoiceError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "manual" => Ok(Self::Manual),
            "manually_confirmed" => Ok(Self::ManuallyConfirmed),
            "source_candidate" => Ok(Self::SourceCandidate),
            "voicematch_candidate" => Ok(Self::VoicematchCandidate),
            _ => Err(VoiceError::Other(format!(
                "unknown voice sample trust class: {value}"
            ))),
        }
    }
}

/// An immutable, provenance-bearing voice embedding sample.
#[derive(Debug, Clone)]
pub struct VoiceSample {
    /// Database row identifier.
    pub id: i64,
    /// Stable person slug associated with the sample.
    pub person_slug: String,
    /// Display name associated with the sample when it was captured.
    pub name: String,
    /// Voice embedding decoded as little-endian `f32` values.
    pub embedding: Vec<f32>,
    /// Number of values in the embedding.
    pub embedding_dim: usize,
    /// Identifier of the model that produced the embedding.
    pub model_id: String,
    /// Embedding normalization convention.
    pub normalization: String,
    /// Provenance and confidence class of the sample.
    pub trust_class: TrustClass,
    /// Optional meeting artifact from which the sample was derived.
    pub meeting_path: Option<String>,
    /// Optional speaker label used in the meeting sidecar.
    pub sidecar_speaker: Option<String>,
    /// Optional capture source description.
    pub capture_source: Option<String>,
    /// Amount of speech represented by the sample, in seconds.
    pub speech_seconds: f64,
    /// Number of speech segments represented by the sample.
    pub segment_count: u32,
    /// Optional serialized quality metrics.
    pub quality_json: Option<String>,
    /// Optional similarity score that produced the sample.
    pub similarity: Option<f64>,
    /// Optional margin between the two strongest matches.
    pub top2_margin: Option<f64>,
    /// Optional version of the threshold policy used for the sample.
    pub threshold_version: Option<String>,
    /// Sensitivity policy assigned to the sample.
    pub sensitivity: String,
    /// Timestamp at which the immutable sample was created.
    pub created_at: String,
    /// Timestamp at which the sample was revoked, if any.
    pub revoked_at: Option<String>,
}

/// Caller-supplied fields used to insert an immutable voice sample.
#[derive(Debug, Clone)]
pub struct VoiceSampleInput {
    /// Stable person slug associated with the sample.
    pub person_slug: String,
    /// Display name associated with the sample.
    pub name: String,
    /// Voice embedding produced by `model_id`.
    pub embedding: Vec<f32>,
    /// Identifier of the model that produced the embedding.
    pub model_id: String,
    /// Provenance and confidence class of the sample.
    pub trust_class: TrustClass,
    /// Optional meeting artifact from which the sample was derived.
    pub meeting_path: Option<String>,
    /// Optional speaker label used in the meeting sidecar.
    pub sidecar_speaker: Option<String>,
    /// Optional capture source description.
    pub capture_source: Option<String>,
    /// Amount of speech represented by the sample, in seconds.
    pub speech_seconds: f64,
    /// Number of speech segments represented by the sample.
    pub segment_count: u32,
    /// Optional serialized quality metrics.
    pub quality_json: Option<String>,
    /// Optional similarity score that produced the sample.
    pub similarity: Option<f64>,
    /// Optional margin between the two strongest matches.
    pub top2_margin: Option<f64>,
    /// Optional version of the threshold policy used for the sample.
    pub threshold_version: Option<String>,
    /// Sensitivity policy assigned to the sample.
    pub sensitivity: String,
    /// Optional deterministic creation timestamp; current local time is used when absent.
    pub created_at: Option<String>,
}

/// A model-scoped voice profile derived from non-revoked immutable samples.
#[derive(Debug, Clone)]
pub struct ActiveProfile {
    /// Stable person slug represented by the profile.
    pub person_slug: String,
    /// Identifier of the embedding model used by every contributing sample.
    pub model_id: String,
    /// Display name associated with the derived profile.
    pub name: String,
    /// Robust mean of the contributing embeddings.
    pub embedding: Vec<f32>,
    /// Number of values in the embedding.
    pub embedding_dim: usize,
    /// Number of non-outlier samples included in the robust mean.
    pub sample_count: u32,
}

#[derive(Debug)]
struct StoredSample {
    id: i64,
    name: String,
    embedding: Vec<f32>,
    embedding_dim: usize,
}

const ACTIVE_PROFILE_COSINE_FLOOR: f32 = 0.5;

fn now_timestamp() -> String {
    chrono::Local::now().to_rfc3339()
}

fn mean_embedding(samples: &[&StoredSample]) -> Vec<f32> {
    let embedding_dim = samples[0].embedding_dim;
    let mut mean = vec![0.0; embedding_dim];
    for sample in samples {
        for (sum, value) in mean.iter_mut().zip(&sample.embedding) {
            *sum += value;
        }
    }
    let count = samples.len() as f32;
    for value in &mut mean {
        *value /= count;
    }
    mean
}

fn rebuild_active_profile_in_transaction(
    conn: &Connection,
    slug: &str,
    model_id: &str,
) -> Result<Option<ActiveProfile>, VoiceError> {
    if model_id == "unknown" {
        conn.execute(
            "DELETE FROM voice_active_profiles WHERE person_slug = ?1 AND model_id = ?2",
            params![slug, model_id],
        )?;
        return Ok(None);
    }

    let samples = {
        let mut stmt = conn.prepare(
            "SELECT id, name, embedding, embedding_dim
             FROM voice_samples
             WHERE person_slug = ?1 AND model_id = ?2 AND revoked_at IS NULL
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![slug, model_id], |row| {
            let blob: Vec<u8> = row.get(2)?;
            Ok(StoredSample {
                id: row.get(0)?,
                name: row.get(1)?,
                embedding: bytes_to_embedding(&blob),
                embedding_dim: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    if samples.is_empty() {
        conn.execute(
            "DELETE FROM voice_active_profiles WHERE person_slug = ?1 AND model_id = ?2",
            params![slug, model_id],
        )?;
        return Ok(None);
    }

    let mut dimension_counts = std::collections::BTreeMap::<usize, (usize, i64)>::new();
    for sample in &samples {
        let entry = dimension_counts
            .entry(sample.embedding_dim)
            .or_insert((0, sample.id));
        entry.0 += 1;
        entry.1 = entry.1.min(sample.id);
    }
    let embedding_dim = dimension_counts
        .into_iter()
        .max_by(|(dim_a, (count_a, first_a)), (dim_b, (count_b, first_b))| {
            count_a
                .cmp(count_b)
                .then_with(|| first_b.cmp(first_a))
                .then_with(|| dim_b.cmp(dim_a))
        })
        .map(|(dimension, _)| dimension)
        .expect("samples is non-empty");

    let matching_samples: Vec<&StoredSample> = samples
        .iter()
        .filter(|sample| {
            sample.embedding_dim == embedding_dim && sample.embedding.len() == embedding_dim
        })
        .collect();

    if matching_samples.is_empty() {
        conn.execute(
            "DELETE FROM voice_active_profiles WHERE person_slug = ?1 AND model_id = ?2",
            params![slug, model_id],
        )?;
        return Ok(None);
    }

    let provisional_centroid = mean_embedding(&matching_samples);
    let similarities: Vec<f32> = matching_samples
        .iter()
        .map(|sample| cosine_similarity(&sample.embedding, &provisional_centroid))
        .collect();
    let mut accepted: Vec<&StoredSample> = matching_samples
        .iter()
        .zip(&similarities)
        .filter_map(|(sample, similarity)| {
            (*similarity >= ACTIVE_PROFILE_COSINE_FLOOR).then_some(*sample)
        })
        .collect();
    if accepted.is_empty() {
        let best_index = similarities
            .iter()
            .enumerate()
            .max_by(|(index_a, similarity_a), (index_b, similarity_b)| {
                similarity_a
                    .total_cmp(similarity_b)
                    .then_with(|| index_b.cmp(index_a))
            })
            .map(|(index, _)| index)
            .expect("matching samples is non-empty");
        accepted.push(matching_samples[best_index]);
    }

    let embedding = mean_embedding(&accepted);
    let name = accepted
        .iter()
        .max_by_key(|sample| sample.id)
        .expect("accepted samples is non-empty")
        .name
        .clone();
    let sample_count = u32::try_from(accepted.len())
        .map_err(|_| VoiceError::Other("voice sample count exceeds u32".to_string()))?;
    let updated_at = now_timestamp();
    conn.execute(
        "INSERT INTO voice_active_profiles
            (person_slug, model_id, name, embedding, embedding_dim, sample_count, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(person_slug, model_id) DO UPDATE SET
            name = excluded.name,
            embedding = excluded.embedding,
            embedding_dim = excluded.embedding_dim,
            sample_count = excluded.sample_count,
            updated_at = excluded.updated_at",
        params![
            slug,
            model_id,
            name,
            embedding_to_bytes(&embedding),
            embedding_dim,
            sample_count,
            updated_at,
        ],
    )?;

    Ok(Some(ActiveProfile {
        person_slug: slug.to_string(),
        model_id: model_id.to_string(),
        name,
        embedding,
        embedding_dim,
        sample_count,
    }))
}

/// Insert one immutable voice sample and rebuild its model-scoped active profile atomically.
pub fn insert_voice_sample(
    conn: &Connection,
    sample: &VoiceSampleInput,
) -> Result<i64, VoiceError> {
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let created_at = sample.created_at.clone().unwrap_or_else(now_timestamp);
    transaction.execute(
        "INSERT INTO voice_samples (
            person_slug, name, embedding, embedding_dim, model_id, normalization,
            trust_class, meeting_path, sidecar_speaker, capture_source, speech_seconds,
            segment_count, quality_json, similarity, top2_margin, threshold_version,
            sensitivity, created_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, 'l2', ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17
         )",
        params![
            sample.person_slug,
            sample.name,
            embedding_to_bytes(&sample.embedding),
            sample.embedding.len(),
            sample.model_id,
            sample.trust_class.as_str(),
            sample.meeting_path,
            sample.sidecar_speaker,
            sample.capture_source,
            sample.speech_seconds,
            sample.segment_count,
            sample.quality_json,
            sample.similarity,
            sample.top2_margin,
            sample.threshold_version,
            sample.sensitivity,
            created_at,
        ],
    )?;
    let id = transaction.last_insert_rowid();
    rebuild_active_profile_in_transaction(&transaction, &sample.person_slug, &sample.model_id)?;
    transaction.commit()?;
    Ok(id)
}

/// Revoke an immutable voice sample and rebuild its model-scoped active profile atomically.
pub fn revoke_voice_sample(conn: &Connection, id: i64) -> Result<(), VoiceError> {
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let sample_key: Option<(String, String, Option<String>)> = transaction
        .query_row(
            "SELECT person_slug, model_id, revoked_at FROM voice_samples WHERE id = ?1",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let (slug, model_id, revoked_at) =
        sample_key.ok_or_else(|| VoiceError::Other(format!("voice sample {id} does not exist")))?;
    if revoked_at.is_none() {
        transaction.execute(
            "UPDATE voice_samples SET revoked_at = ?1 WHERE id = ?2",
            params![now_timestamp(), id],
        )?;
    }
    rebuild_active_profile_in_transaction(&transaction, &slug, &model_id)?;
    transaction.commit()?;
    Ok(())
}

/// Rebuild one model-scoped active profile transactionally from non-revoked samples.
pub fn rebuild_active_profile(
    conn: &Connection,
    slug: &str,
    model_id: &str,
) -> Result<Option<ActiveProfile>, VoiceError> {
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let profile = rebuild_active_profile_in_transaction(&transaction, slug, model_id)?;
    transaction.commit()?;
    Ok(profile)
}

fn active_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ActiveProfile> {
    let blob: Vec<u8> = row.get(3)?;
    Ok(ActiveProfile {
        person_slug: row.get(0)?,
        model_id: row.get(1)?,
        name: row.get(2)?,
        embedding: bytes_to_embedding(&blob),
        embedding_dim: row.get(4)?,
        sample_count: row.get(5)?,
    })
}

/// Read the cached active profile for one person and embedding model.
pub fn active_profile(
    conn: &Connection,
    slug: &str,
    model_id: &str,
) -> Result<Option<ActiveProfile>, VoiceError> {
    conn.query_row(
        "SELECT person_slug, model_id, name, embedding, embedding_dim, sample_count
         FROM voice_active_profiles
         WHERE person_slug = ?1 AND model_id = ?2",
        params![slug, model_id],
        active_profile_from_row,
    )
    .optional()
    .map_err(Into::into)
}

/// List every cached active profile produced by one embedding model.
pub fn list_active_profiles(
    conn: &Connection,
    model_id: &str,
) -> Result<Vec<ActiveProfile>, VoiceError> {
    let mut stmt = conn.prepare(
        "SELECT person_slug, model_id, name, embedding, embedding_dim, sample_count
         FROM voice_active_profiles
         WHERE model_id = ?1
         ORDER BY person_slug ASC",
    )?;
    let profiles = stmt
        .query_map(params![model_id], active_profile_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(profiles)
}

/// Import legacy mutable profiles as manual immutable samples without creating duplicates.
pub fn migrate_legacy_profiles(conn: &Connection) -> Result<usize, VoiceError> {
    let transaction = Transaction::new_unchecked(conn, TransactionBehavior::Immediate)?;
    let legacy_profiles = {
        let mut stmt = transaction.prepare(
            "SELECT person_slug, name, embedding, enrolled_at, source, model_version
             FROM voice_profiles
             ORDER BY id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    let mut migrated = 0;
    for (slug, name, embedding, enrolled_at, source, legacy_model_id) in legacy_profiles {
        let model_id = if embedding.len() % std::mem::size_of::<f32>() == 0 {
            legacy_model_id
        } else {
            "unknown".to_string()
        };
        let exists: bool = transaction.query_row(
            "SELECT EXISTS(
                SELECT 1 FROM voice_samples
                WHERE person_slug = ?1 AND model_id = ?2 AND created_at = ?3
                    AND trust_class = 'manual'
             )",
            params![slug, model_id, enrolled_at],
            |row| row.get(0),
        )?;
        if !exists {
            transaction.execute(
                "INSERT INTO voice_samples (
                    person_slug, name, embedding, embedding_dim, model_id, normalization,
                    trust_class, capture_source, sensitivity, created_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'l2', 'manual', ?6, 'normal', ?7)",
                params![
                    slug,
                    name,
                    embedding,
                    embedding.len() / std::mem::size_of::<f32>(),
                    model_id,
                    source,
                    enrolled_at,
                ],
            )?;
            migrated += 1;
        }
        rebuild_active_profile_in_transaction(&transaction, &slug, &model_id)?;
    }
    transaction.commit()?;
    Ok(migrated)
}

pub fn save_profile(
    conn: &Connection,
    slug: &str,
    name: &str,
    embedding: &[f32],
    source: &str,
    model_version: &str,
) -> Result<(), VoiceError> {
    let now = chrono::Local::now().to_rfc3339();
    let blob = embedding_to_bytes(embedding);
    conn.execute(
        "INSERT INTO voice_profiles (person_slug, name, embedding, enrolled_at, updated_at, sample_count, source, model_version)
         VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
         ON CONFLICT(person_slug) DO UPDATE SET
            name = excluded.name, embedding = excluded.embedding, updated_at = excluded.updated_at,
            sample_count = sample_count + 1, source = excluded.source, model_version = excluded.model_version",
        params![slug, name, blob, now, now, source, model_version],
    )?;
    Ok(())
}

pub fn save_profile_blended(
    conn: &Connection,
    slug: &str,
    name: &str,
    new_embedding: &[f32],
    source: &str,
    model_version: &str,
) -> Result<(), VoiceError> {
    if let Some(existing) = load_profile_with_embedding(conn, slug)? {
        let total = existing.sample_count as f32 + 1.0;
        let old_weight = existing.sample_count as f32;
        let blended: Vec<f32> = existing
            .embedding
            .iter()
            .zip(new_embedding.iter())
            .map(|(old, new)| (old * old_weight + new) / total)
            .collect();
        save_profile(conn, slug, name, &blended, source, model_version)
    } else {
        save_profile(conn, slug, name, new_embedding, source, model_version)
    }
}

fn load_profile_with_embedding(
    conn: &Connection,
    slug: &str,
) -> Result<Option<VoiceProfileWithEmbedding>, VoiceError> {
    let mut stmt = conn.prepare("SELECT person_slug, name, embedding, sample_count FROM voice_profiles WHERE person_slug = ?1")?;
    match stmt.query_row(params![slug], |row| {
        let blob: Vec<u8> = row.get(2)?;
        Ok(VoiceProfileWithEmbedding {
            person_slug: row.get(0)?,
            name: row.get(1)?,
            embedding: bytes_to_embedding(&blob),
            sample_count: row.get(3)?,
        })
    }) {
        Ok(p) => Ok(Some(p)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn list_profiles(conn: &Connection) -> Result<Vec<VoiceProfile>, VoiceError> {
    let mut stmt = conn.prepare("SELECT person_slug, name, enrolled_at, updated_at, sample_count, source, model_version FROM voice_profiles ORDER BY updated_at DESC")?;
    let profiles = stmt
        .query_map([], |row| {
            Ok(VoiceProfile {
                person_slug: row.get(0)?,
                name: row.get(1)?,
                enrolled_at: row.get(2)?,
                updated_at: row.get(3)?,
                sample_count: row.get(4)?,
                source: row.get(5)?,
                model_version: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(profiles)
}

pub fn load_all_with_embeddings(
    conn: &Connection,
) -> Result<Vec<VoiceProfileWithEmbedding>, VoiceError> {
    let mut stmt =
        conn.prepare("SELECT person_slug, name, embedding, sample_count FROM voice_profiles")?;
    let profiles = stmt
        .query_map([], |row| {
            let blob: Vec<u8> = row.get(2)?;
            Ok(VoiceProfileWithEmbedding {
                person_slug: row.get(0)?,
                name: row.get(1)?,
                embedding: bytes_to_embedding(&blob),
                sample_count: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(profiles)
}

pub fn delete_profile(conn: &Connection, slug: &str) -> Result<bool, VoiceError> {
    Ok(conn.execute(
        "DELETE FROM voice_profiles WHERE person_slug = ?1",
        params![slug],
    )? > 0)
}

pub fn match_embedding(
    embedding: &[f32],
    profiles: &[VoiceProfileWithEmbedding],
    threshold: f32,
) -> Option<String> {
    let mut best_name = None;
    let mut best_sim = f32::MIN;

    for p in profiles {
        let sim = cosine_similarity(embedding, &p.embedding);
        tracing::debug!(
            profile = %p.name,
            similarity = format!("{:.4}", sim),
            "voice embedding comparison"
        );
        if sim > best_sim {
            best_sim = sim;
            if sim > threshold {
                best_name = Some(p.name.clone());
            }
        }
    }

    if let Some(ref name) = best_name {
        tracing::info!(matched = %name, similarity = format!("{:.4}", best_sim), "voice profile matched");
    } else if !profiles.is_empty() {
        tracing::info!(
            best_similarity = format!("{:.4}", best_sim),
            threshold = format!("{:.4}", threshold),
            "no voice profile matched"
        );
    }

    best_name
}

/// Save per-speaker embeddings as a sidecar file next to the meeting markdown.
/// Path: ~/meetings/.2026-03-25-standup.embeddings (hidden file, same dir)
pub fn save_meeting_embeddings(
    meeting_path: &std::path::Path,
    embeddings: &std::collections::HashMap<String, Vec<f32>>,
) {
    if embeddings.is_empty() {
        return;
    }
    let sidecar = meeting_embeddings_sidecar_path(meeting_path);
    let data = serde_json::to_vec(embeddings).unwrap_or_default();
    if let Err(e) = std::fs::write(&sidecar, &data) {
        tracing::warn!(path = %sidecar.display(), error = %e, "failed to write meeting embeddings");
    } else {
        // Set 0600 permissions (embeddings are biometric-adjacent data)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&sidecar, std::fs::Permissions::from_mode(0o600)).ok();
        }
        tracing::debug!(path = %sidecar.display(), speakers = embeddings.len(), "meeting embeddings saved");
    }
}

/// Load per-speaker embeddings from a meeting's sidecar file.
pub fn load_meeting_embeddings(
    meeting_path: &std::path::Path,
) -> Option<std::collections::HashMap<String, Vec<f32>>> {
    let sidecar = meeting_embeddings_sidecar_path(meeting_path);
    let data = std::fs::read(&sidecar).ok()?;
    serde_json::from_slice(&data).ok()
}

pub fn meeting_embeddings_sidecar_path(meeting_path: &std::path::Path) -> std::path::PathBuf {
    let dir = meeting_path.parent().unwrap_or(std::path::Path::new("."));
    let stem = meeting_path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    dir.join(format!(".{}.embeddings", stem.trim_end_matches(".md")))
}

pub fn load_self_profile(config: &Config) -> Option<VoiceProfileWithEmbedding> {
    if !config.voice.enabled {
        return None;
    }
    let name = config.identity.name.as_ref()?;
    let slug = slugify(name);
    let conn = open_db().ok()?;
    load_profile_with_embedding(&conn, &slug).ok().flatten()
}

fn slugify(text: &str) -> String {
    let slug: String = text
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let mut result = String::new();
    let mut prev_hyphen = false;
    for c in slug.chars() {
        if c == '-' {
            if !prev_hyphen && !result.is_empty() {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    result.trim_end_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_db() -> (Connection, NamedTempFile) {
        let tmp = NamedTempFile::new().unwrap();
        let conn = open_db_at(tmp.path()).unwrap();
        (conn, tmp)
    }

    fn voice_sample_input(
        slug: &str,
        name: &str,
        embedding: &[f32],
        model_id: &str,
        created_at: &str,
    ) -> VoiceSampleInput {
        VoiceSampleInput {
            person_slug: slug.to_string(),
            name: name.to_string(),
            embedding: embedding.to_vec(),
            model_id: model_id.to_string(),
            trust_class: TrustClass::Manual,
            meeting_path: None,
            sidecar_speaker: None,
            capture_source: Some("test".to_string()),
            speech_seconds: 3.0,
            segment_count: 1,
            quality_json: None,
            similarity: None,
            top2_margin: None,
            threshold_version: None,
            sensitivity: "normal".to_string(),
            created_at: Some(created_at.to_string()),
        }
    }

    fn assert_embedding_close(actual: &[f32], expected: &[f32]) {
        assert_eq!(actual.len(), expected.len());
        for (actual, expected) in actual.iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1e-6,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn voice_samples_insert_same_model_derives_mean() {
        let (conn, _tmp) = test_db();
        insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[1.0, 0.0], "model-a", "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[0.8, 0.2], "model-a", "2026-01-02T00:00:00Z"),
        )
        .unwrap();

        let profile = active_profile(&conn, "mat", "model-a").unwrap().unwrap();
        assert_eq!(profile.sample_count, 2);
        assert_eq!(profile.embedding_dim, 2);
        assert_embedding_close(&profile.embedding, &[0.9, 0.1]);
        assert_eq!(list_active_profiles(&conn, "model-a").unwrap().len(), 1);
    }

    #[test]
    fn voice_samples_different_models_are_isolated() {
        let (conn, _tmp) = test_db();
        insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[1.0, 0.0], "model-a", "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[0.0, 1.0], "model-b", "2026-01-02T00:00:00Z"),
        )
        .unwrap();

        let model_a = active_profile(&conn, "mat", "model-a").unwrap().unwrap();
        let model_b = active_profile(&conn, "mat", "model-b").unwrap().unwrap();
        assert_embedding_close(&model_a.embedding, &[1.0, 0.0]);
        assert_embedding_close(&model_b.embedding, &[0.0, 1.0]);
        assert_eq!(model_a.sample_count, 1);
        assert_eq!(model_b.sample_count, 1);
    }

    #[test]
    fn voice_samples_revocation_rebuilds_and_removes_profile() {
        let (conn, _tmp) = test_db();
        let first = insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[1.0, 0.0], "model-a", "2026-01-01T00:00:00Z"),
        )
        .unwrap();
        let second = insert_voice_sample(
            &conn,
            &voice_sample_input("mat", "Mat", &[0.8, 0.2], "model-a", "2026-01-02T00:00:00Z"),
        )
        .unwrap();

        revoke_voice_sample(&conn, first).unwrap();
        let profile = active_profile(&conn, "mat", "model-a").unwrap().unwrap();
        assert_eq!(profile.sample_count, 1);
        assert_embedding_close(&profile.embedding, &[0.8, 0.2]);

        revoke_voice_sample(&conn, second).unwrap();
        assert!(active_profile(&conn, "mat", "model-a").unwrap().is_none());
    }

    #[test]
    fn voice_samples_rebuild_rejects_outlier() {
        let (conn, _tmp) = test_db();
        for (index, embedding) in [
            vec![1.0, 0.0, 0.0],
            vec![0.98, 0.02, 0.0],
            vec![0.97, -0.03, 0.0],
            vec![0.0, 1.0, 0.0],
        ]
        .iter()
        .enumerate()
        {
            insert_voice_sample(
                &conn,
                &voice_sample_input(
                    "mat",
                    "Mat",
                    embedding,
                    "model-a",
                    &format!("2026-01-0{}T00:00:00Z", index + 1),
                ),
            )
            .unwrap();
        }

        let profile = rebuild_active_profile(&conn, "mat", "model-a")
            .unwrap()
            .unwrap();
        assert_eq!(profile.sample_count, 3);
        assert_embedding_close(&profile.embedding, &[0.98333335, -0.003333333, 0.0]);
    }

    #[test]
    fn voice_samples_migrate_legacy_profiles_is_idempotent() {
        let (conn, _tmp) = test_db();
        for (slug, name, embedding, model_id, timestamp) in [
            (
                "mat",
                "Mat",
                vec![1.0, 0.0],
                "model-a",
                "2026-01-01T00:00:00Z",
            ),
            (
                "alex",
                "Alex",
                vec![0.0, 1.0],
                "model-b",
                "2026-01-02T00:00:00Z",
            ),
        ] {
            conn.execute(
                "INSERT INTO voice_profiles (
                    person_slug, name, embedding, enrolled_at, updated_at,
                    sample_count, source, model_version
                 ) VALUES (?1, ?2, ?3, ?4, ?4, 1, 'legacy', ?5)",
                params![
                    slug,
                    name,
                    embedding_to_bytes(&embedding),
                    timestamp,
                    model_id
                ],
            )
            .unwrap();
        }

        assert_eq!(migrate_legacy_profiles(&conn).unwrap(), 2);
        assert_eq!(migrate_legacy_profiles(&conn).unwrap(), 0);
        let sample_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM voice_samples WHERE trust_class = 'manual'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(sample_count, 2);
        assert!(active_profile(&conn, "mat", "model-a").unwrap().is_some());
        assert!(active_profile(&conn, "alex", "model-b").unwrap().is_some());
    }

    #[test]
    fn voice_samples_migrate_malformed_legacy_blob_as_unknown() {
        let (conn, _tmp) = test_db();
        conn.execute(
            "INSERT INTO voice_profiles (
                person_slug, name, embedding, enrolled_at, updated_at,
                sample_count, source, model_version
             ) VALUES ('mat', 'Mat', ?1, ?2, ?2, 1, 'legacy', 'model-a')",
            params![vec![1_u8, 2, 3], "2026-01-01T00:00:00Z"],
        )
        .unwrap();

        assert_eq!(migrate_legacy_profiles(&conn).unwrap(), 1);
        let model_id: String = conn
            .query_row(
                "SELECT model_id FROM voice_samples WHERE person_slug = 'mat'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(model_id, "unknown");
        assert!(active_profile(&conn, "mat", "unknown").unwrap().is_none());
        assert!(list_active_profiles(&conn, "unknown").unwrap().is_empty());
    }

    #[test]
    fn voice_samples_trust_class_round_trips() {
        for trust_class in [
            TrustClass::Manual,
            TrustClass::ManuallyConfirmed,
            TrustClass::SourceCandidate,
            TrustClass::VoicematchCandidate,
        ] {
            assert_eq!(
                TrustClass::from_str(trust_class.as_str()).unwrap(),
                trust_class
            );
        }
        assert!(TrustClass::from_str("untrusted").is_err());
    }

    #[test]
    fn cosine_identical() {
        assert!((cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]) - 1.0).abs() < 1e-6);
    }
    #[test]
    fn cosine_orthogonal() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }
    #[test]
    fn cosine_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn embedding_roundtrip() {
        let orig = vec![0.1, 0.2, -0.3, 1.0];
        assert_eq!(bytes_to_embedding(&embedding_to_bytes(&orig)), orig);
    }

    const TEST_MODEL_VERSION: &str = "test_model_v1";

    #[test]
    fn save_and_list() {
        let (conn, _tmp) = test_db();
        save_profile(
            &conn,
            "mat",
            "Mat",
            &vec![0.1f32; 512],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        let profiles = list_profiles(&conn).unwrap();
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].person_slug, "mat");
        assert_eq!(profiles[0].sample_count, 1);
    }

    #[test]
    fn upsert_increments_count() {
        let (conn, _tmp) = test_db();
        save_profile(
            &conn,
            "mat",
            "Mat",
            &[0.1f32; 4],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        save_profile(
            &conn,
            "mat",
            "Mat",
            &[0.2f32; 4],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        assert_eq!(list_profiles(&conn).unwrap()[0].sample_count, 2);
    }

    #[test]
    fn blended_averages() {
        let (conn, _tmp) = test_db();
        save_profile(
            &conn,
            "mat",
            "Mat",
            &[1.0f32; 4],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        save_profile_blended(
            &conn,
            "mat",
            "Mat",
            &[3.0f32; 4],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        let p = load_profile_with_embedding(&conn, "mat").unwrap().unwrap();
        assert!((p.embedding[0] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn delete_works() {
        let (conn, _tmp) = test_db();
        save_profile(
            &conn,
            "mat",
            "Mat",
            &[0.1f32; 4],
            "self-enrollment",
            TEST_MODEL_VERSION,
        )
        .unwrap();
        assert!(delete_profile(&conn, "mat").unwrap());
        assert!(list_profiles(&conn).unwrap().is_empty());
    }

    #[test]
    fn match_finds_best() {
        let profiles = vec![
            VoiceProfileWithEmbedding {
                person_slug: "mat".into(),
                name: "Mat".into(),
                embedding: vec![1.0, 0.0, 0.0],
                sample_count: 1,
            },
            VoiceProfileWithEmbedding {
                person_slug: "alex".into(),
                name: "Alex".into(),
                embedding: vec![0.0, 1.0, 0.0],
                sample_count: 1,
            },
        ];
        assert_eq!(
            match_embedding(&[0.9, 0.1, 0.0], &profiles, 0.5),
            Some("Mat".into())
        );
        assert_eq!(
            match_embedding(&[0.0, 1.0, 0.0], &profiles, 0.5),
            Some("Alex".into())
        );
    }

    #[test]
    fn match_none_below_threshold() {
        let profiles = vec![VoiceProfileWithEmbedding {
            person_slug: "mat".into(),
            name: "Mat".into(),
            embedding: vec![1.0, 0.0],
            sample_count: 1,
        }];
        assert_eq!(match_embedding(&[0.0, 1.0], &profiles, 0.5), None);
    }

    #[test]
    fn slugify_basic() {
        assert_eq!(slugify("Mat Silverstein"), "mat-silverstein");
    }

    #[test]
    fn meeting_embeddings_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let meeting = dir.path().join("2026-03-25-standup.md");
        std::fs::write(&meeting, "---\ntitle: test\n---\ntranscript").unwrap();

        let mut embeddings = std::collections::HashMap::new();
        embeddings.insert("SPEAKER_1".to_string(), vec![0.1f32, 0.2, 0.3]);
        embeddings.insert("SPEAKER_2".to_string(), vec![0.4f32, 0.5, 0.6]);

        save_meeting_embeddings(&meeting, &embeddings);

        let loaded = load_meeting_embeddings(&meeting).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["SPEAKER_1"], vec![0.1f32, 0.2, 0.3]);
        assert_eq!(loaded["SPEAKER_2"], vec![0.4f32, 0.5, 0.6]);
    }

    #[test]
    fn meeting_embeddings_missing_returns_none() {
        let dir = tempfile::TempDir::new().unwrap();
        let meeting = dir.path().join("nonexistent.md");
        assert!(load_meeting_embeddings(&meeting).is_none());
    }

    #[test]
    fn sidecar_path_is_hidden_file() {
        let p = meeting_embeddings_sidecar_path(std::path::Path::new(
            "/tmp/meetings/2026-03-25-standup.md",
        ));
        assert_eq!(
            p.file_name().unwrap().to_str().unwrap(),
            ".2026-03-25-standup.embeddings"
        );
    }
}
