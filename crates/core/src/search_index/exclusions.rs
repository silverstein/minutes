//! Shared path-exclusion predicate.
//!
//! Used by:
//! - the legacy walkdir-based search path in [`crate::search`]
//! - sync (full per-file mtime scan) in [`crate::search_index`]
//! - the watcher coalescer that feeds incremental upserts to the index
//!
//! Without a single source of truth, the watcher would silently breach the
//! existing `archive/` / `processed/` / `failed/` / `failed-captures/` exclusion
//! that the rest of Minutes relies on.

use std::path::{Component, Path};

/// Directory names anywhere along the relative path that disqualify a file.
const EXCLUDED_DIR_NAMES: &[&str] = &["archive", "processed", "failed", "failed-captures", ".git"];

/// Returns true if the path lives under an excluded directory or is outside
/// `output_dir` entirely. A `.md` file that hits this predicate must not enter
/// the index from any path (walker, watcher, sync).
pub fn is_excluded_path(path: &Path, output_dir: &Path) -> bool {
    let rel = match path.strip_prefix(output_dir) {
        Ok(r) => r,
        Err(_) => return true,
    };
    rel.components().any(|c| {
        if let Component::Normal(name) = c {
            if let Some(name) = name.to_str() {
                return EXCLUDED_DIR_NAMES.contains(&name);
            }
        }
        false
    })
}

/// Editor-temp / dotfile guard. Used by the watcher coalescer so atomic-save
/// patterns don't briefly index a partially-written tempfile.
pub fn is_temp_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| {
            n.starts_with('.') || n.ends_with(".swp") || n.ends_with('~') || n.ends_with(".tmp")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn archive_excluded() {
        assert!(is_excluded_path(
            &p("/u/meetings/archive/2024-01-01.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn processed_excluded() {
        assert!(is_excluded_path(
            &p("/u/meetings/processed/foo.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn failed_captures_excluded() {
        assert!(is_excluded_path(
            &p("/u/meetings/failed-captures/x.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn dotgit_excluded() {
        assert!(is_excluded_path(
            &p("/u/meetings/.git/config"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn nested_excluded() {
        assert!(is_excluded_path(
            &p("/u/meetings/nested/archive/foo.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn normal_path_included() {
        assert!(!is_excluded_path(
            &p("/u/meetings/2026-04-29-call.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn memos_subdir_included() {
        assert!(!is_excluded_path(
            &p("/u/meetings/memos/voice-memo.md"),
            &p("/u/meetings"),
        ));
    }

    #[test]
    fn path_outside_output_dir_excluded() {
        assert!(is_excluded_path(&p("/etc/passwd"), &p("/u/meetings")));
    }

    #[test]
    fn temp_swp_file() {
        assert!(is_temp_file(&p("/u/meetings/.foo.md.swp")));
    }

    #[test]
    fn temp_tilde_file() {
        assert!(is_temp_file(&p("/u/meetings/foo.md~")));
    }

    #[test]
    fn temp_tmp_file() {
        assert!(is_temp_file(&p("/u/meetings/foo.tmp")));
    }

    #[test]
    fn temp_dotfile() {
        assert!(is_temp_file(&p("/u/meetings/.DS_Store")));
    }

    #[test]
    fn normal_file_not_temp() {
        assert!(!is_temp_file(&p("/u/meetings/2026-04-29.md")));
    }
}
