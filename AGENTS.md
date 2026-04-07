# Agent Instructions

This project uses **bd** (beads) for issue tracking. Run `bd onboard` to get started.
In this repo, beads is **local-only**: use it for structured issue tracking on your machine, but do not expect a shared Dolt remote.

## Quick Reference

```bash
bd ready              # Find available work
bd show <id>          # View issue details
bd update <id> --claim  # Claim work atomically
bd close <id>         # Complete work
```

## GitHub Discussions

This repo has GitHub Discussions enabled (`silverstein/minutes`). Issues are for bugs and feature requests. Discussions are for usage questions, setup help, and community show-and-tell.

**Agent guidelines:**
- When triaging an issue that's really a "how do I...?" question, suggest converting it to a Discussion rather than closing it
- When a user's bug report turns out to be a config/setup issue, answer it and note that Discussions is the better venue for follow-ups
- After shipping a feature or fix, check if any open Q&A discussions are resolved by the change — post a reply pointing to the release
- When writing user-facing error messages or help text, link to Discussions (not Issues) for support: `https://github.com/silverstein/minutes/discussions`
- Don't file Discussions as work items — they're community conversations, not tracked tasks

## Non-Interactive Shell Commands

**ALWAYS use non-interactive flags** with file operations to avoid hanging on confirmation prompts.

Shell commands like `cp`, `mv`, and `rm` may be aliased to include `-i` (interactive) mode on some systems, causing the agent to hang indefinitely waiting for y/n input.

**Use these forms instead:**
```bash
# Force overwrite without prompting
cp -f source dest           # NOT: cp source dest
mv -f source dest           # NOT: mv source dest
rm -f file                  # NOT: rm file

# For recursive operations
rm -rf directory            # NOT: rm -r directory
cp -rf source dest          # NOT: cp -r source dest
```

**Other commands that may prompt:**
- `scp` - use `-o BatchMode=yes` for non-interactive
- `ssh` - use `-o BatchMode=yes` to fail instead of prompting
- `apt-get` - use `-y` flag
- `brew` - use `HOMEBREW_NO_AUTO_UPDATE=1` env var

## macOS Desktop Identity Rule

For any local desktop work that touches macOS privacy / TCC-sensitive features
(Microphone, Screen Recording, Input Monitoring, Accessibility, call capture,
global hotkeys), do **not** dogfood by repeatedly replacing
`/Applications/Minutes.app` with ad-hoc local rebuilds.

Use the dedicated development app identity instead:

```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Mathieu Silverstein (63TMLKT8HN)"
./scripts/install-dev-app.sh
```

Canonical dogfood target:

- `~/Applications/Minutes Dev.app`

Why:

- macOS TCC permissions attach to the effective app identity and signature
- ad-hoc local rebuilds of `/Applications/Minutes.app` can trigger repeated or misleading permission prompts
- the signed dev app is the stable local identity for permission-sensitive testing

<!-- BEGIN BEADS INTEGRATION profile:full hash:d4f96305 -->
## Issue Tracking with bd (beads)

**IMPORTANT**: This project uses **bd (beads)** for ALL issue tracking. Do NOT use markdown TODOs, task lists, or other tracking methods.

### Why bd?

- Dependency-aware: Track blockers and relationships between issues
- Git-friendly: Dolt-backed local history for issue state
- Agent-optimized: JSON output, ready work detection, discovered-from links
- Prevents duplicate tracking systems and confusion

### Quick Start

**Check for ready work:**

```bash
bd ready --json
```

**Create new issues:**

```bash
bd create "Issue title" --description="Detailed context" -t bug|feature|task -p 0-4 --json
bd create "Issue title" --description="What this issue is about" -p 1 --deps discovered-from:bd-123 --json
```

**Claim and update:**

```bash
bd update <id> --claim --json
bd update bd-42 --priority 1 --json
```

**Complete work:**

```bash
bd close bd-42 --reason "Completed" --json
```

### Issue Types

- `bug` - Something broken
- `feature` - New functionality
- `task` - Work item (tests, docs, refactoring)
- `epic` - Large feature with subtasks
- `chore` - Maintenance (dependencies, tooling)

### Priorities

- `0` - Critical (security, data loss, broken builds)
- `1` - High (major features, important bugs)
- `2` - Medium (default, nice-to-have)
- `3` - Low (polish, optimization)
- `4` - Backlog (future ideas)

### Workflow for AI Agents

1. **Check ready work**: `bd ready` shows unblocked issues
2. **Claim your task atomically**: `bd update <id> --claim`
3. **Work on it**: Implement, test, document
4. **Discover new work?** Create linked issue:
   - `bd create "Found bug" --description="Details about what was found" -p 1 --deps discovered-from:<parent-id>`
5. **Complete**: `bd close <id> --reason "Done"`

### Local-Only Storage

bd writes issue state into the local beads/Dolt store for this repo.

- Each write auto-commits to local Dolt history
- This repo does **not** use a configured Dolt remote
- Do not require `bd dolt push`/`bd dolt pull` in landing workflows unless the repo is explicitly reconfigured later

### Important Rules

- ✅ Use bd for ALL task tracking
- ✅ Always use `--json` flag for programmatic use
- ✅ Link discovered work with `discovered-from` dependencies
- ✅ Check `bd ready` before asking "what should I work on?"
- ❌ Do NOT create markdown TODO lists
- ❌ Do NOT use external issue trackers
- ❌ Do NOT duplicate tracking systems

For more details, see README.md and docs/QUICKSTART.md.

## Landing the Plane (Session Completion)

**When ending a work session**, you MUST complete ALL steps below. Work is NOT complete until `git push` succeeds.

**MANDATORY WORKFLOW:**

1. **File issues for remaining work** - Create issues for anything that needs follow-up
2. **Run quality gates** (if code changed) - Tests, linters, builds
3. **Update issue status** - Close finished work, update in-progress items
4. **PUSH TO REMOTE** - This is MANDATORY:
   ```bash
   git pull --rebase
   git push
   git status  # MUST show "up to date with origin"
   ```
5. **Clean up** - Clear stashes, prune remote branches
6. **Verify** - All changes committed AND pushed
7. **Hand off** - Provide context for next session

**CRITICAL RULES:**
- Work is NOT complete until `git push` succeeds
- NEVER stop before pushing - that leaves work stranded locally
- NEVER say "ready to push when you are" - YOU must push
- If push fails, resolve and retry until it succeeds

<!-- END BEADS INTEGRATION -->
