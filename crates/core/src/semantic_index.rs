//! Durable semantic vectors for meetings.
//!
//! A private SQLite file under `~/.minutes/semantic/`. What keeps it honest:
//! - restricted meetings are never embedded, and a meeting that becomes
//!   restricted loses its vectors at the next sync;
//! - every vector is stored with the SHA-256 of the exact bytes it was
//!   computed from, and search uses only vectors whose digest matches the
//!   meeting's current, authorized content (a stale vector is never used);
//! - the file is owner-only and `clear` removes the whole index.
//!
//! Vectors are stored at [`crate::semantic::DIMENSIONS`] as `i8` (normalized
//! components scaled by 127): ~256 bytes per chunk.
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const SCHEMA_VERSION: i64 = 1;
const FILE_NAME: &str = "index-v1.sqlite";

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("semantic index database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("semantic index file error: {0}")]
    Io(#[from] std::io::Error),
    #[error("the semantic index location is not a private directory")]
    UnsafeLocation,
}

/// One embedded span of a meeting body (byte offsets into the body).
#[derive(Debug, Clone, PartialEq)]
pub struct StoredChunk {
    pub start: usize,
    pub end: usize,
    pub vector: Vec<i8>,
}

pub struct SemanticIndex {
    conn: Connection,
}

pub fn index_dir(minutes_dir: &Path) -> PathBuf {
    minutes_dir.join("semantic")
}

/// Normalized f32 components to stored i8.
pub fn quantize(vector: &[f32]) -> Vec<i8> {
    vector
        .iter()
        .map(|v| (v * 127.0).round().clamp(-127.0, 127.0) as i8)
        .collect()
}

impl SemanticIndex {
    /// Open (or create) the index for `model_id` at `dimensions`. A file
    /// written for a different model or width is wiped: its vectors would be
    /// meaningless against new queries.
    pub fn open(minutes_dir: &Path, model_id: &str, dimensions: usize) -> Result<Self, IndexError> {
        let dir = index_dir(minutes_dir);
        create_private_dir(&dir)?;
        let path = dir.join(FILE_NAME);
        if std::fs::symlink_metadata(&path).is_ok_and(|meta| !meta.file_type().is_file()) {
            return Err(IndexError::UnsafeLocation);
        }
        let conn = Connection::open(&path)?;
        set_owner_only(&path)?;
        conn.execute_batch(
            "PRAGMA journal_mode = DELETE;
             PRAGMA secure_delete = ON;
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS documents (
                 path TEXT PRIMARY KEY,
                 content_sha256 BLOB NOT NULL,
                 title TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS chunks (
                 path TEXT NOT NULL,
                 chunk INTEGER NOT NULL,
                 start INTEGER NOT NULL,
                 end INTEGER NOT NULL,
                 vector BLOB NOT NULL,
                 PRIMARY KEY (path, chunk)
             ) WITHOUT ROWID;",
        )?;
        let index = Self { conn };
        let identity = format!("{SCHEMA_VERSION}:{model_id}:{dimensions}");
        if index.meta("identity")?.as_deref() != Some(identity.as_str()) {
            index
                .conn
                .execute_batch("DELETE FROM chunks; DELETE FROM documents;")?;
            index.conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('identity', ?1)",
                params![identity],
            )?;
        }
        Ok(index)
    }

    fn meta(&self, key: &str) -> Result<Option<String>, IndexError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Digest each indexed meeting was embedded from.
    pub fn indexed(&self) -> Result<HashMap<PathBuf, [u8; 32]>, IndexError> {
        let mut statement = self
            .conn
            .prepare("SELECT path, content_sha256 FROM documents")?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(0)?;
            let digest: Vec<u8> = row.get(1)?;
            Ok((path, digest))
        })?;
        let mut indexed = HashMap::new();
        for row in rows {
            let (path, digest) = row?;
            if let Ok(digest) = <[u8; 32]>::try_from(digest.as_slice()) {
                indexed.insert(PathBuf::from(path), digest);
            }
        }
        Ok(indexed)
    }

    /// (meetings, chunks) currently stored.
    pub fn counts(&self) -> Result<(usize, usize), IndexError> {
        let count = |table: &str| -> Result<usize, IndexError> {
            let n: i64 =
                self.conn
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })?;
            Ok(n as usize)
        };
        Ok((count("documents")?, count("chunks")?))
    }

    /// Replace a meeting's vectors in one transaction.
    pub fn upsert(
        &mut self,
        path: &Path,
        content_sha256: &[u8; 32],
        title: &str,
        chunks: &[StoredChunk],
    ) -> Result<(), IndexError> {
        let key = path.to_string_lossy();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM chunks WHERE path = ?1", params![key])?;
        tx.execute(
            "INSERT OR REPLACE INTO documents (path, content_sha256, title) VALUES (?1, ?2, ?3)",
            params![key, content_sha256.as_slice(), title],
        )?;
        for (i, chunk) in chunks.iter().enumerate() {
            let bytes: Vec<u8> = chunk.vector.iter().map(|v| *v as u8).collect();
            tx.execute(
                "INSERT INTO chunks (path, chunk, start, end, vector) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![key, i as i64, chunk.start as i64, chunk.end as i64, bytes],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Drop a meeting's vectors (deleted, restricted, or unreadable).
    pub fn remove(&mut self, path: &Path) -> Result<(), IndexError> {
        let key = path.to_string_lossy();
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM chunks WHERE path = ?1", params![key])?;
        tx.execute("DELETE FROM documents WHERE path = ?1", params![key])?;
        tx.commit()?;
        Ok(())
    }

    /// Every stored chunk of meetings whose stored digest still matches the
    /// digest `current` reports for them; anything else is skipped.
    pub fn current_chunks(
        &self,
        current: impl Fn(&Path) -> Option<[u8; 32]>,
    ) -> Result<Vec<(PathBuf, String, StoredChunk)>, IndexError> {
        let fresh: HashMap<PathBuf, ()> = self
            .indexed()?
            .into_iter()
            .filter(|(path, digest)| current(path).as_ref() == Some(digest))
            .map(|(path, _)| (path, ()))
            .collect();
        let mut statement = self.conn.prepare(
            "SELECT c.path, d.title, c.start, c.end, c.vector
             FROM chunks c JOIN documents d ON d.path = c.path
             ORDER BY c.path, c.chunk",
        )?;
        let rows = statement.query_map([], |row| {
            let path: String = row.get(0)?;
            let title: String = row.get(1)?;
            let start: i64 = row.get(2)?;
            let end: i64 = row.get(3)?;
            let vector: Vec<u8> = row.get(4)?;
            Ok((path, title, start, end, vector))
        })?;
        let mut chunks = Vec::new();
        for row in rows {
            let (path, title, start, end, vector) = row?;
            let path = PathBuf::from(path);
            if fresh.contains_key(&path) {
                chunks.push((
                    path,
                    title,
                    StoredChunk {
                        start: start as usize,
                        end: end as usize,
                        vector: vector.into_iter().map(|v| v as i8).collect(),
                    },
                ));
            }
        }
        Ok(chunks)
    }
}

/// Remove the whole index (turning semantic search off).
pub fn clear(minutes_dir: &Path) -> Result<(), IndexError> {
    let dir = index_dir(minutes_dir);
    match std::fs::symlink_metadata(&dir) {
        Ok(meta) if meta.file_type().is_symlink() => Err(IndexError::UnsafeLocation),
        Ok(_) => Ok(std::fs::remove_dir_all(&dir)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_private_dir(dir: &Path) -> Result<(), IndexError> {
    match std::fs::symlink_metadata(dir) {
        Ok(meta) if !meta.file_type().is_dir() => return Err(IndexError::UnsafeLocation),
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir)?;
        }
        Err(error) => return Err(error.into()),
    }
    set_owner_only_dir(dir)
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), IndexError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only_dir(path: &Path) -> Result<(), IndexError> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_: &Path) -> Result<(), IndexError> {
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_dir(_: &Path) -> Result<(), IndexError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(start: usize, v: &[f32]) -> StoredChunk {
        StoredChunk {
            start,
            end: start + 10,
            vector: quantize(v),
        }
    }

    #[test]
    fn only_vectors_from_the_current_content_are_returned() {
        let home = tempfile::tempdir().unwrap();
        let mut index = SemanticIndex::open(home.path(), "m", 2).unwrap();
        let (a, b) = (PathBuf::from("/m/a.md"), PathBuf::from("/m/b.md"));
        index
            .upsert(&a, &[1; 32], "A", &[chunk(0, &[1.0, 0.0])])
            .unwrap();
        index
            .upsert(&b, &[2; 32], "B", &[chunk(0, &[0.0, 1.0])])
            .unwrap();

        // b changed since it was embedded; c was never indexed.
        let live = |path: &Path| match path.to_str() {
            Some("/m/a.md") => Some([1; 32]),
            Some("/m/b.md") => Some([9; 32]),
            _ => None,
        };
        let chunks = index.current_chunks(live).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, a);
        assert_eq!(chunks[0].2.vector, vec![127, 0]);

        index.remove(&a).unwrap();
        assert!(index.current_chunks(live).unwrap().is_empty());
    }

    #[test]
    fn upsert_replaces_and_a_model_change_wipes() {
        let home = tempfile::tempdir().unwrap();
        let path = PathBuf::from("/m/a.md");
        {
            let mut index = SemanticIndex::open(home.path(), "m1", 2).unwrap();
            index
                .upsert(
                    &path,
                    &[1; 32],
                    "A",
                    &[chunk(0, &[1.0, 0.0]), chunk(5, &[0.0, 1.0])],
                )
                .unwrap();
            index
                .upsert(&path, &[1; 32], "A", &[chunk(0, &[1.0, 0.0])])
                .unwrap();
            assert_eq!(index.current_chunks(|_| Some([1; 32])).unwrap().len(), 1);
        }
        let reopened = SemanticIndex::open(home.path(), "m1", 2).unwrap();
        assert_eq!(reopened.indexed().unwrap().len(), 1);
        let other_model = SemanticIndex::open(home.path(), "m2", 2).unwrap();
        assert!(other_model.indexed().unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn the_index_is_owner_only_and_clear_removes_it() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        SemanticIndex::open(home.path(), "m", 2).unwrap();
        let dir = index_dir(home.path());
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        let file = dir.join(FILE_NAME);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        clear(home.path()).unwrap();
        assert!(!dir.exists());
        clear(home.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_index_directory_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), index_dir(home.path())).unwrap();
        assert!(matches!(
            SemanticIndex::open(home.path(), "m", 2),
            Err(IndexError::UnsafeLocation)
        ));
        assert!(matches!(
            clear(home.path()),
            Err(IndexError::UnsafeLocation)
        ));
    }

    #[test]
    fn quantize_maps_unit_components_to_i8() {
        assert_eq!(quantize(&[1.0, -1.0, 0.5, 2.0]), vec![127, -127, 64, 127]);
    }
}
