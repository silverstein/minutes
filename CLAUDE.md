# CLAUDE.md — Minutes

> Every meeting, every idea, every voice note — searchable by your AI.

## Project Overview

**Minutes** — open-source, privacy-first conversation memory layer for AI assistants. Captures any audio (meetings, voice memos, brain dumps), transcribes locally with whisper.cpp, diarizes speakers, summarizes with any LLM, and outputs searchable markdown. Built with Rust + Tauri v2 + Node.js (MCPB).

**Two input modes, one pipeline:**
- **Live recording**: `minutes record` / `minutes stop` — for meetings, calls, conversations
- **Folder watcher**: `minutes watch <dir>` — auto-processes voice memos, audio files dropped into a folder. Zero-friction iPhone pipeline via iCloud Voice Memos sync.

## Quick Start

```bash
cd ~/Sites/minutes
cargo build                          # Build Rust workspace
cargo test                           # Run tests
cargo run --bin minutes -- record    # Start recording a meeting
cargo run --bin minutes -- stop      # Stop and process
cargo run --bin minutes -- watch ~/path/to/voice-memos  # Watch for new audio files
cargo run --bin minutes -- search "pricing"             # Search all meetings + memos
```

## Project Structure

```
minutes/
├── PLAN.md                    # Master plan (survives compaction — read this first)
├── CLAUDE.md                  # This file
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── core/                  # Audio capture, transcription, diarization, summarization
│   ├── cli/                   # CLI binary (minutes record/stop/status/watch/list/search/logs)
│   └── mcp/                   # Node.js MCPB wrapper for Claude Desktop
├── tauri/                     # Tauri v2 menu bar app
├── tests/
│   ├── fixtures/              # test-5s.wav, mock data for transcript/diarization/summary
│   ├── unit/                  # Markdown writer, config parser, search, PID lifecycle
│   └── integration/           # Full pipeline, folder watcher
└── docs/                      # User-facing docs
```

## Development Commands

```bash
# Build
cargo build                          # Debug build
cargo build --release                # Release build

# Test
cargo test                           # All tests
cargo test --package minutes-core    # Core tests only

# Lint
cargo clippy -- -D warnings         # Lint
cargo fmt --check                    # Format check

# MCPB
cd crates/mcp && npm install && npm run build
```

## Architecture Decisions

- **Rust** for audio engine, transcription, diarization — cross-platform, fast, single binary
- **Tauri v2** for desktop app — Rust backend shared with CLI, web frontend, ~10MB
- **Node.js** for MCPB only — required by Claude Desktop extension format
- **Markdown + YAML frontmatter** for storage — universal, works with QMD/Obsidian/grep
- **Pluggable LLM** for summarization — Claude, Ollama, OpenAI via config
- **BlackHole** for Phase 1 CLI audio capture (ScreenCaptureKit in Phase 3 Tauri app)
- **pyannote via subprocess** for diarization (AGPL-safe, best quality). sherpa-onnx as native fallback.

## Key Patterns

- All audio processing is local (whisper.cpp + pyannote/sherpa-onnx)
- Only LLM summarization optionally touches the network
- Config lives at `~/.config/minutes/config.toml`
- Meetings stored at configurable path (default: `~/meetings/`)
- Voice memos stored at `~/meetings/memos/` (configurable)
- Two content types: `type: meeting` (multi-speaker, calendar-linked) and `type: memo` (single-speaker, no calendar)
- PARA-compatible output format for QMD/Obsidian users
- iPhone voice memos auto-process via Apple Shortcut → iCloud Drive → `~/.minutes/inbox/` (no FDA needed)
- Recording lifecycle: PID file at `~/.minutes/recording.pid` + signals
- Watcher lifecycle: settle delay, move to `processed/`/`failed/`, lock file
- All output files written with `0600` permissions (sensitive content)
- Structured logging: JSON lines to `~/.minutes/logs/minutes.log`

## Beads Tracking

Tasks tracked with `bd` (beads). See PLAN.md for full task breakdown (~59 tasks across 7 sub-phases).

## Testing Loop

1. Write implementation
2. Write tests
3. Manual test with real audio
4. Code review agent
5. `cargo build --release` passes
6. Close bead

## Claude Ecosystem Integration Strategy

This project has a unique strategic position in the Claude ecosystem:

### MCPB (Claude Desktop Extension)
- Packages as a .mcpb file for one-click install
- MCP tools: start_recording, stop_recording, list_meetings, search_meetings, get_transcript, process_audio
- Claude Desktop can query meeting history AND voice memos mid-conversation

### Cowork Integration
- Works as a Cowork tool — Claude can manage recordings autonomously
- Meeting notes + voice memos appear in Claude's context for follow-up conversations

### Dispatch (Mobile)
- Phone → Dispatch → "Start recording my meeting" → Mac captures audio
- Phone → Dispatch → "Stop recording" → Claude processes and summarizes
- Voice memos recorded on iPhone auto-sync and process without Dispatch

### Claude Code Plugin Potential
- Could ship as a Claude Code plugin (.claude/plugins/)
- Skills: `/minutes record`, `/minutes search`, `/minutes list`, `/minutes recap`
- Hooks: PostToolUse could auto-tag meetings with current project context
- Agent: `meeting-analyst` for cross-meeting intelligence queries

### Private Strategy Notes
- Business-specific strategy lives in gitignored `.claude/` directory
