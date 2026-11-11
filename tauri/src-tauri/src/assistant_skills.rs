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
            // A user's own --settings wins; that session keeps typed notices.
            let user_settings = args
                .iter()
                .any(|arg| arg == "--settings" || arg.starts_with("--settings="));
            args.push("--add-dir".into());
            args.push(root.to_string_lossy().into_owned());
            if cfg!(unix) && !user_settings {
                if let Ok(path) = write_focus_settings(workspace) {
                    args.push("--settings".into());
                    args.push(path.to_string_lossy().into_owned());
                }
            }
        }
    }
    args
}

/// Settings file passed to Claude with `--settings`, holding only the focus
/// hook. It lives in the managed pack so the user's `.claude` stays untouched.
const FOCUS_SETTINGS_FILE: &str = "claude-focus-settings.json";
const FOCUS_DIR: &str = "focus";

/// A change of focus waiting for the agent's next prompt. Each kind has its
/// own file, so switching meeting and then document before the next prompt
/// delivers both; a later change of the same kind replaces the earlier one.
#[derive(Clone, Copy, Debug)]
pub(crate) enum FocusNotice {
    Meeting,
    Artifact,
}

impl FocusNotice {
    const ALL: [Self; 2] = [Self::Meeting, Self::Artifact];

    fn file_name(self) -> &'static str {
        match self {
            Self::Meeting => "meeting.md",
            Self::Artifact => "artifact.md",
        }
    }
}

fn focus_dir(workspace: &Path) -> PathBuf {
    claude_skill_directory(workspace).join(FOCUS_DIR)
}

fn focus_settings_path(workspace: &Path) -> PathBuf {
    claude_skill_directory(workspace).join(FOCUS_SETTINGS_FILE)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Claude Code adds a successful `UserPromptSubmit` hook's stdout to that
/// prompt's context. Each queued notice is renamed before it is printed, so
/// exactly one prompt receives it, and an empty queue prints nothing.
fn focus_hook_command(workspace: &Path) -> String {
    let dir = shell_quote(&focus_dir(workspace).to_string_lossy());
    let files = FocusNotice::ALL.map(FocusNotice::file_name).join(" ");
    format!(
        "cd {dir} 2>/dev/null || exit 0; for f in {files}; do \
         if mv \"$f\" \".$f.$$\" 2>/dev/null; then cat \".$f.$$\"; printf '\\n'; rm -f \".$f.$$\"; fi; \
         done; exit 0"
    )
}

fn write_focus_settings(workspace: &Path) -> Result<PathBuf, String> {
    let root = claude_skill_directory(workspace);
    reject_symlink(&root)?;
    if fs::read_to_string(root.join(OWNER_FILE)).ok().as_deref() != Some(OWNER) {
        return Err(format!("{} is not Minutes-managed", root.display()));
    }
    let path = focus_settings_path(workspace);
    reject_symlink(&path)?;
    let settings = serde_json::json!({
        "hooks": {
            "UserPromptSubmit": [
                { "hooks": [ { "type": "command", "command": focus_hook_command(workspace) } ] }
            ]
        }
    });
    let body = serde_json::to_string_pretty(&settings).map_err(|e| e.to_string())?;
    if fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
        fs::write(&path, body).map_err(|e| e.to_string())?;
    }
    Ok(path)
}

/// Whether these spawn arguments wire Minutes' focus hook into Claude.
#[cfg(test)]
pub(crate) fn has_focus_hook(args: &[String], workspace: &Path) -> bool {
    let path = focus_settings_path(workspace)
        .to_string_lossy()
        .into_owned();
    args.windows(2)
        .any(|pair| pair[0] == "--settings" && pair[1] == path)
}

/// Queue a focus notice for the agent's next prompt, replacing any earlier
/// notice of the same kind. Written to a temporary name and renamed so the
/// hook never reads half a file.
pub(crate) fn queue_focus_notice(
    workspace: &Path,
    kind: FocusNotice,
    text: &str,
) -> Result<(), String> {
    let dir = focus_dir(workspace);
    reject_symlink(&claude_skill_directory(workspace))?;
    reject_symlink(&dir)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let target = dir.join(kind.file_name());
    reject_symlink(&target)?;
    let staging = dir.join(format!(".{}.queue", kind.file_name()));
    fs::write(&staging, text).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
    }
    fs::rename(&staging, &target).map_err(|e| e.to_string())
}

/// Consume focus context once for a tab's next real prompt, on every platform.
pub(crate) fn take_focus_notices(workspace: &Path) -> Result<String, String> {
    let dir = focus_dir(workspace);
    let mut result = String::new();
    for kind in FocusNotice::ALL {
        let path = dir.join(kind.file_name());
        let claimed = dir.join(format!(".{}.{}", kind.file_name(), std::process::id()));
        if fs::rename(&path, &claimed).is_ok() {
            if let Ok(text) = fs::read_to_string(&claimed) {
                result.push_str(&text);
                result.push('\n');
            }
            let _ = fs::remove_file(claimed);
        }
    }
    Ok(result)
}

/// Drop queued notices, so a new session never receives a previous
/// session's focus changes with its first prompt.
pub(crate) fn clear_focus_notices(workspace: &Path) {
    let dir = focus_dir(workspace);
    for kind in FocusNotice::ALL {
        let _ = fs::remove_file(dir.join(kind.file_name()));
    }
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
    fn claude_gets_the_focus_hook_unless_the_user_passes_settings() {
        let (_temp, workspace, source) = fixture();
        sync_claude_skills(&workspace, &source).unwrap();
        let args = terminal_args("claude", vec![], &workspace);
        assert!(has_focus_hook(&args, &workspace), "{args:?}");
        let settings: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(focus_settings_path(&workspace)).unwrap())
                .unwrap();
        assert_eq!(
            settings["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"],
            focus_hook_command(&workspace)
        );
        assert!(
            !fs::read_dir(workspace.join(".claude"))
                .unwrap()
                .any(|entry| entry.unwrap().file_name() == FOCUS_SETTINGS_FILE),
            "the user's .claude stays untouched"
        );

        for user_args in [
            vec!["--settings".to_string(), "/mine.json".to_string()],
            vec!["--settings=/mine.json".to_string()],
        ] {
            let args = terminal_args("claude", user_args, &workspace);
            assert!(!has_focus_hook(&args, &workspace), "{args:?}");
        }
        for agent in ["codex", "opencode", "zsh"] {
            assert!(!has_focus_hook(
                &terminal_args(agent, vec![], &workspace),
                &workspace
            ));
        }
    }

    #[cfg(unix)]
    #[test]
    fn hook_prints_each_queued_notice_once() {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("Mat's assistant with spaces");
        let source = temp.path().join("generated");
        fs::create_dir_all(source.join("minutes-prep")).unwrap();
        fs::write(source.join("minutes-prep/SKILL.md"), "prep").unwrap();
        sync_claude_skills(&workspace, &source).unwrap();
        let run = || {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(focus_hook_command(&workspace))
                .current_dir(temp.path())
                .output()
                .unwrap();
            assert!(out.status.success());
            String::from_utf8(out.stdout).unwrap()
        };

        assert_eq!(run(), "", "nothing queued, nothing printed");

        queue_focus_notice(&workspace, FocusNotice::Artifact, "opened a.md").unwrap();
        queue_focus_notice(&workspace, FocusNotice::Meeting, "meeting X").unwrap();
        queue_focus_notice(&workspace, FocusNotice::Artifact, "opened b.md").unwrap();
        let first = run();
        assert!(first.contains("meeting X"), "{first}");
        assert!(first.contains("opened b.md"), "{first}");
        assert!(
            !first.contains("opened a.md"),
            "a later change replaces an earlier one"
        );
        assert_eq!(run(), "", "a notice reaches one prompt only");

        queue_focus_notice(&workspace, FocusNotice::Meeting, "stale").unwrap();
        clear_focus_notices(&workspace);
        assert_eq!(run(), "");
        let leftovers: Vec<_> = fs::read_dir(focus_dir(&workspace))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert!(leftovers.is_empty(), "{leftovers:?}");
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
