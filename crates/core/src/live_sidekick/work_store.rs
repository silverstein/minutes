//! Private, immutable checkpoint persistence used by the desktop and CLI hosts.
//! Uses the existing capability-bound storage boundary, never a model path.
use super::work::{WorkCheckpoint, MAX_CHECKPOINT_BYTES};
use crate::policy_fs::BoundRecoveryDirectory;
use serde::Serialize;
use std::ffi::OsStr;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_ENTRIES: usize = 2048;

#[derive(Debug, Clone, Serialize)]
pub struct SavedWork {
    pub name: String,
    pub id: String,
    pub goal: String,
    pub revision: u64,
}

/// The root is selected by the trusted host; names are opaque handles, not paths.
pub struct WorkStore {
    root: BoundRecoveryDirectory,
}

impl WorkStore {
    pub fn open(path: &Path) -> Result<Self, String> {
        Ok(Self {
            root: BoundRecoveryDirectory::prepare_owner_private(path).map_err(|e| e.to_string())?,
        })
    }

    pub fn default_path() -> PathBuf {
        crate::config::Config::minutes_dir().join("work-checkpoints")
    }

    pub fn save(&self, checkpoint: &WorkCheckpoint) -> Result<SavedWork, String> {
        let body = checkpoint.to_markdown().map_err(|e| e.to_string())?;
        // Never overwrite an existing user artifact. The name contains no user/model text.
        let mut random = [0u8; 16];
        getrandom::fill(&mut random).map_err(|e| e.to_string())?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_micros();
        let name = format!("work-{time:020}-{token}.md");
        let stage = format!(".stage-{token}");
        let mut file = self
            .root
            .create_new_exact_file(OsStr::new(&stage))
            .map_err(|e| e.to_string())?;
        file.write_all(body.as_bytes())
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        let bound = self
            .root
            .bind_owner_private_exact_file(OsStr::new(&stage))
            .map_err(|e| e.to_string())?;
        bound
            .recovery_proof_for_exact_bytes_bounded(
                body.as_bytes(),
                MAX_CHECKPOINT_BYTES as u64,
                Instant::now() + Duration::from_secs(5),
            )
            .map_err(|e| e.to_string())?;
        self.root
            .rename_bound_no_replace(bound, OsStr::new(&name))
            .map_err(|e| e.to_string())?;
        Ok(SavedWork {
            name,
            id: checkpoint.id.clone(),
            goal: checkpoint.goal.clone(),
            revision: checkpoint.revision,
        })
    }

    pub fn load(&self, name: &str) -> Result<WorkCheckpoint, String> {
        validate_name(name)?;
        let bound = self
            .root
            .bind_owner_private_exact_file(OsStr::new(name))
            .map_err(|e| e.to_string())?;
        let proof = bound
            .recovery_proof_bounded(
                MAX_CHECKPOINT_BYTES as u64,
                Instant::now() + Duration::from_secs(5),
            )
            .map_err(|e| e.to_string())?;
        let mut body = String::new();
        let mut reader = bound.try_clone_exact_file().map_err(|e| e.to_string())?;
        reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        reader
            .take(MAX_CHECKPOINT_BYTES as u64 + 1)
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        bound
            .attest_recovery_proof_bounded(
                &proof,
                MAX_CHECKPOINT_BYTES as u64,
                Instant::now() + Duration::from_secs(5),
            )
            .map_err(|e| e.to_string())?;
        bound
            .recovery_proof_for_exact_bytes_bounded(
                body.as_bytes(),
                MAX_CHECKPOINT_BYTES as u64,
                Instant::now() + Duration::from_secs(5),
            )
            .map_err(|e| e.to_string())?;
        WorkCheckpoint::from_markdown(&body).map_err(|e| e.to_string())
    }

    pub fn list(&self) -> Result<Vec<SavedWork>, String> {
        self.root
            .attest_for_source_cleanup()
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut names = Vec::new();
        // Listing is discovery only; every actual read is through the retained capability.
        for (n, entry) in std::fs::read_dir(self.root.display_path())
            .map_err(|e| e.to_string())?
            .enumerate()
        {
            if n >= MAX_ENTRIES {
                return Err("Checkpoint directory is too large to list safely; archive older checkpoints locally.".into());
            }
            let entry = entry.map_err(|e| e.to_string())?;
            if let Some(name) = entry
                .file_name()
                .to_str()
                .filter(|s| validate_name(s).is_ok())
            {
                names.push(name.to_string());
            }
        }
        names.sort();
        names.reverse();
        let mut result = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for name in names {
            if Instant::now() >= deadline {
                return Err(
                    "Checkpoint listing exceeded its time budget; narrow the local archive".into(),
                );
            }
            let data = self.load(&name)?;
            if seen.insert(data.id.clone()) {
                result.push(SavedWork {
                    name,
                    id: data.id,
                    goal: data.goal,
                    revision: data.revision,
                });
            }
            if result.len() == 50 {
                break;
            }
        }
        Ok(result)
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if !name.is_ascii()
        || name.len() != 61
        || !name.starts_with("work-")
        || !name.ends_with(".md")
        || !name[5..25].bytes().all(|b| b.is_ascii_digit())
        || &name[25..26] != "-"
        || !name[26..58].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(
            "Use an exact checkpoint name returned by list; paths are not accepted.".into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::super::work::WorkSession;
    use super::*;
    #[test]
    fn private_checkpoint_roundtrip_and_revision_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = WorkStore::open(&dir.path().join("work")).unwrap();
        let mut work = WorkSession::new("test-work".into(), "Review a paragraph".into()).unwrap();
        let first = store.save(&work.checkpoint()).unwrap();
        work.add_host_note("Interested is not agreed".into(), vec![], false)
            .unwrap();
        let second = store.save(&work.checkpoint()).unwrap();
        assert_ne!(first.name, second.name);
        assert_eq!(store.load(&first.name).unwrap().memory.len(), 0);
        assert_eq!(store.load(&second.name).unwrap().memory.len(), 1);
        assert_eq!(store.list().unwrap().len(), 1);
    }
    #[test]
    fn names_are_not_paths_and_malformed_unicode_does_not_panic() {
        for name in ["../notes.md", "/tmp/file", "work-💥.md", "foo"] {
            assert!(validate_name(name).is_err());
        }
        let hostile = format!("work-{}", "💥".repeat(14));
        assert!(validate_name(&hostile).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_checkpoint_is_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        let store = WorkStore::open(&root).unwrap();
        let name = "work-00000000000000000000-00000000000000000000000000000000.md";
        std::os::unix::fs::symlink("/etc/passwd", root.join(name)).unwrap();
        assert!(store.load(name).is_err());
    }
}
