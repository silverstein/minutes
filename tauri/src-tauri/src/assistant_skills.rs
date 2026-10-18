//! Claude project skills in a Minutes-owned additional directory. The user's
//! `.claude` can be a symlink to their global configuration; never write there.
use std::fs;
use std::path::{Path, PathBuf};

const PACK_DIR: &str = ".minutes-skill-pack";
const OWNER_FILE: &str = ".minutes-managed";
const OWNER: &str = "Minutes assistant skill pack v1\n";

pub(crate) fn claude_skill_directory(workspace: &Path) -> PathBuf {
    workspace.join(PACK_DIR)
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!(
            "Refusing to write through skill-pack symlink: {}",
            path.display()
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Cannot inspect {}: {error}", path.display())),
    }
}

// Refresh only changed bytes so reopening Recall doesn't cause a skill reload.
// This entire tree is app-managed. User skills live in the untouched .claude.
fn sync_tree(source: &Path, target: &Path) -> Result<(), String> {
    reject_symlink(target)?;
    fs::create_dir_all(target).map_err(|e| e.to_string())?;
    let entries: Vec<_> = fs::read_dir(source)
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    for entry in &entries {
        let from = entry.path();
        let to = target.join(entry.file_name());
        reject_symlink(&to)?;
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            sync_tree(&from, &to)?;
        } else {
            let bytes = fs::read(&from).map_err(|e| e.to_string())?;
            if fs::read(&to).ok().as_deref() != Some(bytes.as_slice()) {
                fs::copy(&from, &to).map_err(|e| e.to_string())?;
            }
        }
    }
    for entry in fs::read_dir(target).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entries
            .iter()
            .any(|source| source.file_name() == entry.file_name())
        {
            if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                fs::remove_dir_all(entry.path()).map_err(|e| e.to_string())?;
            } else {
                fs::remove_file(entry.path()).map_err(|e| e.to_string())?;
            }
        }
    }
    Ok(())
}

/// The portable pack is already generated from canonical sources, including
/// helper assets. Its path note resolves helpers from workspace .agents, so
/// loading it as a Claude project skill needs no plugin variable substitution,
/// duplicate MCP registration, or automatic hooks.
pub(crate) fn sync_claude_skills(workspace: &Path, source: &Path) -> Result<(), String> {
    let root = claude_skill_directory(workspace);
    reject_symlink(&root)?;
    reject_symlink(&root.join(OWNER_FILE))?;
    if root.exists() && fs::read_to_string(root.join(OWNER_FILE)).ok().as_deref() != Some(OWNER) {
        return Err(format!(
            "{} is not a Minutes-managed skill pack; refusing to replace it",
            root.display()
        ));
    }
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    fs::write(root.join(OWNER_FILE), OWNER).map_err(|e| e.to_string())?;
    reject_symlink(&root.join(".claude"))?;
    sync_tree(source, &root.join(".claude/skills"))
}

/// Preserve configured arguments and add project skills only for Claude's
/// interactive terminal. Private Recall Chat deliberately keeps its isolation.
pub(crate) fn terminal_args(agent: &str, mut args: Vec<String>, workspace: &Path) -> Vec<String> {
    if Path::new(agent)
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("claude"))
    {
        let root = claude_skill_directory(workspace);
        if root.join(".claude/skills").is_dir() {
            args.push("--add-dir".into());
            args.push(root.to_string_lossy().into_owned());
        }
    }
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("assistant with spaces");
        let source = temp.path().join("generated");
        fs::create_dir_all(source.join("minutes-prep/scripts")).unwrap();
        fs::write(source.join("minutes-prep/SKILL.md"), "prep v1").unwrap();
        fs::write(source.join("minutes-prep/scripts/helper.mjs"), "helper").unwrap();
        fs::create_dir_all(workspace.join(".claude/skills/my-skill")).unwrap();
        fs::write(workspace.join(".claude/settings.json"), "user settings").unwrap();
        fs::write(workspace.join(".claude/skills/my-skill/SKILL.md"), "custom").unwrap();
        (temp, workspace, source)
    }

    #[test]
    fn refreshes_assets_and_removes_retired_skills_without_touching_user_configuration() {
        let (_temp, workspace, source) = fixture();
        sync_claude_skills(&workspace, &source).unwrap();
        let skills = claude_skill_directory(&workspace).join(".claude/skills");
        assert_eq!(
            fs::read_to_string(skills.join("minutes-prep/scripts/helper.mjs")).unwrap(),
            "helper"
        );
        fs::write(source.join("minutes-prep/SKILL.md"), "prep v2").unwrap();
        fs::create_dir_all(skills.join("minutes-retired")).unwrap();
        fs::write(skills.join("minutes-retired/SKILL.md"), "retired").unwrap();
        sync_claude_skills(&workspace, &source).unwrap();
        assert_eq!(
            fs::read_to_string(skills.join("minutes-prep/SKILL.md")).unwrap(),
            "prep v2"
        );
        assert!(!skills.join("minutes-retired").exists());
        assert_eq!(
            fs::read_to_string(workspace.join(".claude/settings.json")).unwrap(),
            "user settings"
        );
        assert_eq!(
            fs::read_to_string(workspace.join(".claude/skills/my-skill/SKILL.md")).unwrap(),
            "custom"
        );
    }

    #[test]
    fn refuses_unowned_pack_directory() {
        let (_temp, workspace, source) = fixture();
        let root = claude_skill_directory(&workspace);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("personal.txt"), "keep").unwrap();
        assert!(sync_claude_skills(&workspace, &source).is_err());
        assert_eq!(
            fs::read_to_string(root.join("personal.txt")).unwrap(),
            "keep"
        );
    }

    #[test]
    fn claude_arguments_keep_user_flags_and_separate_paths_with_spaces() {
        let (_temp, workspace, source) = fixture();
        let user_args = vec!["--add-dir".into(), "/my other project".into()];
        assert_eq!(
            terminal_args("claude", user_args.clone(), &workspace),
            user_args
        );
        sync_claude_skills(&workspace, &source).unwrap();
        for agent in [
            "claude",
            "Claude",
            "/bin/claude",
            "claude.cmd",
            "claude.exe",
        ] {
            let args = terminal_args(agent, user_args.clone(), &workspace);
            assert_eq!(&args[..2], &user_args);
            assert_eq!(args[2], "--add-dir");
            assert_eq!(
                args[3],
                claude_skill_directory(&workspace).to_string_lossy()
            );
        }
        for agent in ["codex", "opencode", "gemini", "bash"] {
            assert_eq!(
                terminal_args(agent, user_args.clone(), &workspace),
                user_args
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn global_claude_symlink_is_preserved_and_managed_symlink_cannot_escape() {
        let (temp, workspace, source) = fixture();
        let global = temp.path().join("global-claude");
        fs::rename(workspace.join(".claude"), &global).unwrap();
        std::os::unix::fs::symlink(&global, workspace.join(".claude")).unwrap();
        sync_claude_skills(&workspace, &source).unwrap();
        assert!(workspace.join(".claude").is_symlink());
        assert_eq!(
            fs::read_to_string(global.join("settings.json")).unwrap(),
            "user settings"
        );
        let managed = claude_skill_directory(&workspace).join(".claude/skills/minutes-prep");
        fs::remove_dir_all(&managed).unwrap();
        std::os::unix::fs::symlink(&global, &managed).unwrap();
        assert!(sync_claude_skills(&workspace, &source).is_err());
        assert_eq!(
            fs::read_to_string(global.join("settings.json")).unwrap(),
            "user settings"
        );
    }
}
