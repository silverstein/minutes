# CLAUDE.md — Minutes

> Your AI remembers every conversation you've had.

## Project Overview

**Minutes** — open-source, privacy-first conversation memory layer for AI assistants. Captures any audio (meetings, voice memos, brain dumps), transcribes locally with whisper.cpp, diarizes speakers, and outputs searchable markdown with structured action items and decisions. Built with Rust + Tauri v2 + Node.js (MCP).

**Three input modes, one pipeline:**
- **Live recording**: `minutes record` / `minutes stop` — for meetings, calls, conversations
- **Notetaking**: `minutes note "important point"` — timestamped annotations during recording
- **Folder watcher**: `minutes watch` — auto-processes voice memos from iPhone/iCloud

## Quick Start

```bash
cd ~/Sites/minutes
cargo build                          # Build Rust workspace
cargo test -p minutes-core --no-default-features  # Fast tests (no whisper model)
cargo run --bin minutes -- setup --model tiny      # Download whisper model
cargo run --bin minutes -- record    # Start recording
cargo run --bin minutes -- stop      # Stop and process
```

## Full Build (CLI + Tauri App)

```bash
./scripts/build.sh                   # Builds everything and installs CLI
./scripts/build.sh --install         # Same + copies .app to /Applications
# Or manually:
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build --release -p minutes-cli           # CLI binary
cargo tauri build --bundles app                # Tauri .app bundle
cp target/release/minutes ~/.local/bin/minutes # Install CLI
open target/release/bundle/macos/Minutes.app   # Launch app
```

**IMPORTANT**: After any code change, you must rebuild ALL affected targets:
- CLI changes: `cargo build --release -p minutes-cli && cp target/release/minutes ~/.local/bin/minutes`
- Tauri changes: `cargo tauri build --bundles app` then relaunch Minutes.app
- MCP server changes: `cd crates/mcp && npm run build` (compiles TS server + builds UI, then restart MCP client sessions)
- MCP App UI only: `cd crates/mcp && npm run build:ui` (rebuild just the dashboard HTML)
- All Rust + app: `./scripts/build.sh` (add `--install` to copy .app to /Applications)
- **Don't forget the MCP server** — it's TypeScript, not Rust. `./scripts/build.sh` does NOT rebuild it. Always run `cd crates/mcp && npm run build` after touching `crates/mcp/src/index.ts` or `crates/mcp/ui/`.

## Release Process

When shipping a new version:
1. Bump version in: `Cargo.toml` (workspace), `tauri/src-tauri/tauri.conf.json`, `crates/mcp/package.json`, `crates/sdk/package.json`
2. Verify all 4 match: `grep version Cargo.toml tauri/src-tauri/tauri.conf.json crates/mcp/package.json crates/sdk/package.json`
3. Rebuild MCP: `cd crates/mcp && npm run build`
4. Commit, tag, push: `git tag vX.Y.Z && git push origin main --tags`
5. Create GitHub release: `gh release create vX.Y.Z -t "title" -F notes.md` (triggers signed DMG + CLI binary CI)
6. **Publish npm packages** (required for `npx minutes-mcp` users):
   ```bash
   cd crates/sdk && npm publish --access public --registry https://registry.npmjs.org
   cd crates/mcp && npm publish --access public --registry https://registry.npmjs.org
   ```
   If 2FA blocks publish, use a granular access token with "Bypass 2FA" enabled.
   **IMPORTANT**: `crates/mcp/package.json` must depend on `"minutes-sdk": "^X.Y.Z"` (npm version), NOT `"file:../sdk"` (local path). Check before publishing.
7. Redeploy landing page: `cd site && vercel deploy --yes --prod --scope evil-genius-laboratory`
8. Update Homebrew tap formula if CLI changed

## Project Structure

```
minutes/
├── PLAN.md                    # Master plan (survives compaction — read this first)
├── CLAUDE.md                  # This file
├── BUILD-STATUS.md            # Build progress tracker
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── core/src/              # 17 Rust modules — the engine
│   │   ├── capture.rs         # Audio capture (cpal)
│   │   ├── transcribe.rs      # Whisper.cpp + symphonia format conversion
│   │   ├── diarize.rs         # Pyannote subprocess
│   │   ├── summarize.rs       # LLM summarization (ureq HTTP client)
│   │   ├── pipeline.rs        # Orchestrates the full flow + structured extraction
│   │   ├── notes.rs           # Timestamped notetaking during/after recordings
│   │   ├── watch.rs           # Folder watcher (settle delay, dedup, lock)
│   │   ├── markdown.rs        # YAML frontmatter + shared parsing utilities
│   │   ├── search.rs          # Walk-dir search + action item queries
│   │   ├── config.rs          # TOML config with compiled defaults
│   │   ├── pid.rs             # PID file lifecycle (flock atomic)
│   │   ├── events.rs          # Append-only JSONL event log for agent reactivity
│   │   ├── logging.rs         # Structured JSON logging
│   │   └── error.rs           # Per-module error types (thiserror)
│   ├── cli/                   # CLI binary — 15 commands
│   ├── reader/                # Lightweight read-only meeting parser (no audio deps)
│   └── mcp/                   # MCP server — 13 tools + 6 resources + MCP App dashboard
│       ├── src/reader.ts      # Pure-TS meeting file reader (npx fallback, SDK foundation)
│       └── ui/                # Interactive dashboard (vanilla TS, builds to single-file HTML)
├── tauri/                     # Tauri v2 menu bar app + singleton AI Assistant
├── .claude/plugins/minutes/   # Claude Code plugin — 11 skills + 1 agent + 2 hooks
└── tests/integration/         # Integration tests (including real whisper tests)
```

## Development Commands

```bash
# Build (macOS 26 needs C++ include path for whisper.cpp)
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build

# Test
cargo test -p minutes-core --no-default-features   # Fast (no whisper model)
cargo test -p minutes-core                          # Full (needs tiny model)

# Lint
cargo clippy --all --no-default-features -- -D warnings
cargo fmt --all -- --check

# MCP server (TS server + interactive dashboard UI)
cd crates/mcp && npm install && npm run build       # tsc + vite single-file build
npx vitest run                                      # 30 reader.ts unit tests
node test/mcp_tools_test.mjs                        # 8 MCP integration tests
```

## Key Architecture Decisions

- **Rust** for the engine — single 6.7MB binary, cross-platform, fast
- **whisper-rs** (whisper.cpp) for transcription — local, Apple Silicon optimized
- **symphonia** for audio format conversion — m4a/mp3/ogg → WAV, pure Rust
- **ureq** for HTTP — pure Rust, no secrets in process args (replaced curl)
- **fs2 flock** for PID files — atomic check-and-write, prevents TOCTOU races
- **Tauri v2** for desktop app — shares `minutes-core` with CLI, ~10MB
- **Markdown + YAML frontmatter** for storage — universal, works with everything
- **Structured extraction** — action items + decisions in frontmatter as queryable YAML
- **No API keys needed** — Claude summarizes conversationally via MCP tools

## Key Patterns

- All audio processing is local (whisper.cpp + pyannote)
- Claude summarizes via MCP when the user asks (no API key needed)
- Optional automated summarization via Ollama (local) or cloud LLMs
- Config at `~/.config/minutes/config.toml` (optional, compiled defaults work)
- Tauri assistant uses a singleton workspace at `~/.minutes/assistant/`
- `CLAUDE.md` holds general assistant instructions; `CURRENT_MEETING.md` is the active meeting focus for "Discuss with AI"
- Meetings: `~/meetings/` | Voice memos: `~/meetings/memos/`
- `0600` permissions on all output (sensitive content)
- PID file + flock for recording state (`~/.minutes/recording.pid`)
- Watcher: settle delay, move to `processed/`/`failed/`, lock file
- JSON structured logging: `~/.minutes/logs/minutes.log`
- 100% doc comment coverage on all pub functions

## Test Coverage

124 tests total:
- 81 unit tests (all core modules including screen, calendar, config)
- 8 integration tests (pipeline, permissions, collisions, search filters)
- 2 real whisper tests (transcription + no-speech detection with tiny model)
- 30 reader.ts unit tests (vitest — frontmatter parsing, listing, search, actions, profiles)
- 8 MCP integration tests (CLI JSON output, TypeScript compilation)
- 4 hook unit tests (post-record hook: routing, edge cases, error handling)
- 1 screen context test (screenshot listing and sorting)

## Claude Ecosystem Integration

- **MCP Server**: 13 tools + 6 resources for Claude Desktop / Cowork / Dispatch (`npx minutes-mcp` for zero-install)
- **Claude Code Plugin**: 11 skills (8 core + 3 interactive lifecycle) + meeting-analyst agent + PostToolUse hook
- **Interactive meeting lifecycle**: `/minutes prep` → record → `/minutes debrief` → `/minutes weekly` with skill chaining via `.prep.md` files
- **Conversational summarization**: Claude reads transcripts via MCP, no API key needed
- **Auto-tagging + alerts**: PostToolUse hook tags meetings with git repo, checks for decision conflicts, surfaces overdue action items
- **Proactive reminders**: SessionStart hook checks calendar for upcoming meetings and nudges `/minutes prep`
- **Desktop assistant**: Tauri AI Assistant is a singleton session that can switch focus into a selected meeting without spawning parallel assistant workspaces
