//! Install the optional Orukeet release for the existing sherpa plugin.

use fs2::FileExt;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const BASE_URL: &str = "https://huggingface.co/oruk/orukeet/resolve/eac739d754bb171287930e6e63386f5b88f8179e/onnx/sherpa-v0.1.0-int8";
const MANIFEST_SHA256: &str = "7e80f93f0e9b923c392424b0f85d28a717feee0a4d2a6aa9bfa723693868e727";
const MANIFEST_BYTES: u64 = 1867;
const FILES: &[&str] = &[
    "encoder.int8.onnx",
    "decoder.int8.onnx",
    "joiner.int8.onnx",
    "tokens.txt",
    "LICENSE-WEIGHTS",
    "NOTICE.md",
];

#[derive(serde::Deserialize)]
struct Entry {
    #[serde(default)]
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(serde::Deserialize)]
struct Manifest {
    files: Vec<Entry>,
}

fn verified(path: &Path, entry: &Entry) -> Result<bool, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|e| e.to_string())?.len() != entry.bytes {
        return Ok(false);
    }
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()) == entry.sha256)
}

fn ensure_file(
    dir: &Path,
    filename: &str,
    entry: &Entry,
    fetch: &mut impl FnMut(&str, &mut File) -> Result<(), String>,
) -> Result<(), String> {
    let dest = dir.join(filename);
    if verified(&dest, entry)? {
        return Ok(());
    }
    eprintln!("Downloading Orukeet {filename} ...");
    let mut staging = tempfile::NamedTempFile::new_in(dir).map_err(|e| e.to_string())?;
    fetch(filename, staging.as_file_mut())?;
    staging.flush().map_err(|e| e.to_string())?;
    if !verified(staging.path(), entry)? {
        return Err(format!("Orukeet checksum mismatch: {filename}"));
    }
    staging.persist(dest).map_err(|e| e.to_string())?;
    Ok(())
}

fn install_with(
    dir: &Path,
    manifest_entry: &Entry,
    mut fetch: impl FnMut(&str, &mut File) -> Result<(), String>,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".install.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock_exclusive().map_err(|e| e.to_string())?;
    ensure_file(dir, "manifest.json", manifest_entry, &mut fetch)?;
    let manifest: Manifest =
        serde_json::from_reader(File::open(dir.join("manifest.json")).map_err(|e| e.to_string())?)
            .map_err(|e| format!("Invalid Orukeet manifest: {e}"))?;
    // Select fixed filenames; never use a manifest path as an install target.
    for filename in FILES {
        let entry = manifest
            .files
            .iter()
            .find(|entry| entry.path == *filename)
            .ok_or_else(|| format!("Orukeet manifest is missing {filename}"))?;
        ensure_file(dir, filename, entry, &mut fetch)?;
    }
    Ok(())
}

/// Download and verify Orukeet without modifying another model or the config.
///
/// All URLs are pinned to one Hugging Face release. The genuine JSON manifest
/// is used to verify model files and participates in normal Hub download stats.
/// Verified cache hits perform no network requests.
pub fn install(model_root: &Path) -> Result<PathBuf, String> {
    let dir = model_root.join("sherpa").join("orukeet-v0.1.0-int8");
    let manifest = Entry {
        path: "manifest.json".to_string(),
        bytes: MANIFEST_BYTES,
        sha256: MANIFEST_SHA256.to_string(),
    };
    install_with(&dir, &manifest, |filename, file| {
        let response = ureq::get(&format!("{BASE_URL}/{filename}"))
            .call()
            .map_err(|e| format!("Orukeet download failed for {filename}: {e}"))?;
        std::io::copy(&mut response.into_body().into_reader(), file).map_err(|e| e.to_string())?;
        Ok(())
    })?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(bytes: &[u8]) -> Entry {
        Entry {
            path: String::new(),
            bytes: bytes.len() as u64,
            sha256: format!("{:x}", Sha256::digest(bytes)),
        }
    }

    fn fixture() -> (Vec<u8>, Entry) {
        let mut files = Vec::new();
        for name in FILES {
            let value = entry(name.as_bytes());
            files.push(
                serde_json::json!({"path": name, "bytes": value.bytes, "sha256": value.sha256}),
            );
        }
        let bytes = serde_json::to_vec(&serde_json::json!({"files": files})).unwrap();
        let expected = entry(&bytes);
        (bytes, expected)
    }

    #[test]
    fn installation_verifies_all_files_and_reuses_cache_without_fetching() {
        let temp = tempfile::tempdir().unwrap();
        let (manifest, expected) = fixture();
        let mut calls = Vec::new();
        install_with(temp.path(), &expected, |name, file| {
            calls.push(name.to_string());
            let bytes = if name == "manifest.json" {
                &manifest
            } else {
                name.as_bytes()
            };
            file.write_all(bytes).map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(calls.len(), FILES.len() + 1);
        install_with(temp.path(), &expected, |_, _| {
            panic!("cached install fetched data")
        })
        .unwrap();
    }

    #[test]
    fn failed_download_preserves_old_file_and_removes_staging() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("model.onnx");
        std::fs::write(&dest, b"old").unwrap();
        let result = ensure_file(temp.path(), "model.onnx", &entry(b"new"), &mut |_, file| {
            file.write_all(b"partial").unwrap();
            Err("network interrupted".to_string())
        });
        assert!(result.is_err());
        assert_eq!(std::fs::read(dest).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn checksum_failure_preserves_old_file() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("model.onnx");
        std::fs::write(&dest, b"old").unwrap();
        let error = ensure_file(temp.path(), "model.onnx", &entry(b"new"), &mut |_, file| {
            file.write_all(b"bad").map_err(|e| e.to_string())
        })
        .unwrap_err();
        assert!(error.contains("checksum mismatch"));
        assert_eq!(std::fs::read(dest).unwrap(), b"old");
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn invalid_manifest_never_downloads_weights() {
        let temp = tempfile::tempdir().unwrap();
        let (_, expected) = fixture();
        let mut calls = 0;
        let result = install_with(temp.path(), &expected, |_, file| {
            calls += 1;
            file.write_all(b"{}").map_err(|e| e.to_string())
        });
        assert!(result.unwrap_err().contains("checksum mismatch"));
        assert_eq!(calls, 1);
        assert!(!temp.path().join("manifest.json").exists());
    }
}
