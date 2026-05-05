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

## Portable Agent Skills (`.agents/skills/minutes/`, `.opencode/skills/`)

This repo maintains skill outputs in **three locations**:

- `.claude/plugins/minutes/` — Claude Code plugin (uses `${CLAUDE_PLUGIN_ROOT}`)
- `.agents/skills/minutes/` — Agent-agnostic mirror for Codex, Gemini, Pi, and other agents (uses `$MINUTES_SKILLS_ROOT`)
- `.opencode/skills/` — OpenCode-native mirror (one-level discovery path + matching `.opencode/commands/`)

**What lives where:**
- `SKILL.md` files are mirrored 1:1. Content is identical except for path variables and platform-specific references (e.g., "open in desktop app" in the plugin version becomes a CLI command in the agents version).
- `_runtime/hooks/lib/` contains `minutes-learn.mjs` and `minutes-learn-cli.mjs` — the behavioral learning system. These must stay byte-identical across `.agents/skills/minutes/_runtime/hooks/lib/` and `.opencode/skills/_runtime/hooks/lib/`.
- Bundled scripts (`scripts/tag_apply.py`, `scripts/graph_build.py`, etc.) are mirrored into both portable trees.
- `.opencode/commands/*.md` provides native `/minutes-*` slash commands for OpenCode and is generated from the same canonical skill sources.

**When you modify a skill or runtime hook:**
```bash
# Preferred workflow: edit the canonical source under tooling/skills/sources/<name>/skill.md
# then regenerate every host surface from one place.
cd tooling/skills
npm run build
npm run compile

# Verify generated outputs are current:
npm run compile:dry
npm run check
```

**Why multiple trees?** Claude Code plugins use `${CLAUDE_PLUGIN_ROOT}` and plugin metadata. Codex/Gemini/Pi consume the `.agents/skills/minutes/` mirror. OpenCode only auto-discovers `skills/*/SKILL.md` one directory deep and has its own `.opencode/commands/` surface, so it needs a flattened generated tree.

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

## Pre-Commit Discipline

The full pre-commit checklist is the canonical surface in `CLAUDE.md`'s **Pre-Commit Checklist** section. It covers MCP manifest sync, MCPB bundle guards, cargo fmt/clippy/test, Unix-only-API gating, feature-stub parity, site release constants, skill compiler outputs, and the new toolchain + UI items below. **Read it before any commit that touches Rust, MCP server, frontend, or release surfaces** — the table is the single source of truth and is kept up to date as failure modes get caught.

Two items added after PR #206 that bear repeating here because they bit hard:

1. **Rust toolchain pin (`rust-toolchain.toml`).** The repo pins rustc/clippy/rustfmt to a specific version. CI uses `dtolnay/rust-toolchain@stable` for the system default, but rustup's cargo proxy reads the pin file when invoked from inside the repo and routes through the pinned toolchain. **The pin is honored locally only when cargo runs through the rustup proxy** — verify with `command -v cargo` matching `rustup which cargo` (typically `~/.cargo/bin/cargo`, but `CARGO_HOME` overrides relocate it). If `which cargo` resolves to `/opt/homebrew/bin/cargo` or another non-rustup path, the pin is silently ignored and your local clippy/rustfmt drift from CI's. Two prior commits (`4954de2`, `21cd699`) are clippy-fix-only commits that landed because of exactly this drift. Fix once: prepend rustup's bin dir to your shell PATH (`export PATH="$(dirname "$(rustup which cargo)"):$PATH"` in your shell rc, or just `~/.cargo/bin` if you haven't overridden `CARGO_HOME`), or `brew uninstall rust`. The build scripts (`scripts/build.sh`, `scripts/install-dev-app.sh`) detect it via `rustup which cargo` themselves so script-driven builds are immune; only interactive `cargo` invocations need the shell PATH fix.

2. **UI render verification.** Any change to `tauri/src/index.html`, any new Tauri `cmd_*` exposed to the frontend, or any modal/overlay/panel layout shift requires building the dev app via `./scripts/install-dev-app.sh` and click-testing the affected surface in `~/Applications/Minutes Dev.app` before commit. Type checks and Rust unit tests do not catch UI render bugs. PR #206 surfaced four such bugs (path candidate dedup, build-artifact bundles polluting the picker, default selection, ad-hoc detection logic) only via click-testing — none would have failed any test or CI job.

## Independent-cadence crate: `whisper-guard`

`crates/whisper-guard/` is published to crates.io on its own cadence — separate from the main Minutes release.
It is NOT in the main Release Checklist's "all 6 versions must match" list.

**When you change anything under `crates/whisper-guard/src/`:**

1. Bump `crates/whisper-guard/Cargo.toml` `version` (semver).
2. Publish independently:
   ```bash
   cd crates/whisper-guard
   cargo publish --dry-run && cargo publish
   ```
3. Do NOT bump the main Minutes version just because whisper-guard changed.

**Before cutting a Minutes release, verify whisper-guard is in sync:**

```bash
PUBLISHED=$(curl -s https://crates.io/api/v1/crates/whisper-guard | jq -r '.crate.max_stable_version')
LAST_PUBLISH_COMMIT=$(git log --grep="whisper-guard $PUBLISHED" --format="%H" | head -1)
git log "$LAST_PUBLISH_COMMIT"..HEAD -- crates/whisper-guard/
```

If that diff is non-empty, publish whisper-guard first. The full procedure lives in `CLAUDE.md` Release Checklist Step 11.5.

**Why this matters:** whisper-guard has external consumers (~277 downloads at last check). Repo state drifting ahead of crates.io means downstream users silently miss anti-hallucination fixes shipped here.

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
