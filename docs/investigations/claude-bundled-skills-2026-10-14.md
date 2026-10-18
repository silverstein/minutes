# Bundled Minutes skills in embedded Claude Code

Production 0.29.1 includes the 25-skill generated portable pack but only
refreshes `.agents` and `.opencode` in the assistant workspace. The embedded
Claude process receives neither project skills nor a plugin directory, so the
Minutes slash commands are absent even though their files ship inside the app.

## Integration choice

Use the standard Agent Skills files as project skills in an app-managed
additional directory, passed with `--add-dir` only to the interactive Claude
Terminal. Claude documents that this loads `.claude/skills/<name>/SKILL.md`
from the additional directory and provides ordinary slash commands. Skill
bodies load when invoked rather than being copied into every system prompt.

Sources reviewed October 14, 2026:

- [Project skills and additional-directory discovery](https://code.claude.com/docs/en/skills#load-skills-from-a-directory-outside-the-project)
- [Skill locations and symlink handling](https://code.claude.com/docs/en/skills#choose-where-skills-load)
- [Plugin distribution](https://code.claude.com/docs/en/plugins)

The desktop already owns a versioned, offline bundle compiled from canonical
sources. Reuse that portable output, including its scripts, references and
runtime helpers. Its skill path notes resolve helpers from the assistant
workspace's `.agents/skills/minutes` mirror. No second compiler or runtime text
rewriter is needed. The external Claude plugin continues to serve users who
install Minutes into their own Claude projects.

Keep the managed pack at `assistant/.minutes-skill-pack/.claude/skills`.
Do not overwrite the workspace `.claude`: older workspaces can symlink it to
user-global settings. The additional root contains only app-owned skills,
without a plugin manifest, MCP registration, hooks, or settings. It is nested
inside the existing assistant workspace, so the added file-access root does
not expose a new corpus. Private Recall Chat's isolation is unchanged.

A managed ownership marker protects an unexpected existing directory. Reject
symlink targets before writes. Refresh changed files and retire removed skills
only within that managed namespace. Preserve user-configured CLI arguments,
including existing additional directories. Claude settings that intentionally
exclude project skills retain their documented behavior.

## Qualification

- Four Rust app regression tests passed: refresh/assets/retirement and user
  configuration preservation; unowned path refusal; platform-specific Claude
  names and paths with spaces; legacy global configuration and escaping
  symlink protection.
- The existing workspace mirror test now proves the Claude skill root receives
  the same generated skill content. It passed in the desktop unit-test binary.
- Real Claude Code 2.1.299 on Silvercloud loaded the managed directory in an
  isolated synthetic Git workspace. `/skills` displayed **25 skills**, marked
  `project`, including Minutes Brief, Cleanup, Copilot, Debrief, Graph, Ideas,
  Ingest and Lint. `/minutes-` autocomplete showed the commands. No skill was
  executed; the session log contained zero assistant messages/provider turns.
- All **61 files** in the managed pack matched the generated portable source
  byte for byte. No meeting corpus was present in the test profile.
- The serial core suite passed: **1,938 passed, zero failed, one ignored**.
- Rust format checks and focused new-module Clippy with denied warnings passed.
- Full-workspace Linux Clippy encountered 59 diagnostics in unchanged
  macOS-only desktop code. No diagnostic points to the new module; every
  diagnostic anchor exists in base `ac58840a`. CI intentionally runs this lint
  on macOS, where the desktop ships. This is not a full-workspace lint pass.
- Signed Minutes Dev / embedded macOS Claude verification remains pending:
  Silverbook was offline on the tailnet during implementation.

After an upgrade, an already-running assistant needs one new Terminal session
so it receives the added directory. Reopening an existing session cannot
retroactively change its launch arguments.
