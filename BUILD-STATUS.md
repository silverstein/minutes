# Minutes — Build Status

> This file tracks implementation progress. Read this after compaction to know exactly where you left off.
> Update this file after completing each bead. Never leave it stale.

## Current Phase: Phase 3 (Tauri menu bar app) scaffolded. All phases 1-3 built.

## Build Chunks

### Chunk 1: Scaffold + Core Pipeline (P1a.0-6)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.0 | NOT STARTED | - | MCPB research blocker — do before Phase 2 |
| P1a.1 | DONE | 10/10 | Cargo workspace: `core` (lib) + `cli` (bin). 10 modules in core. |
| P1a.2 | DONE | 10/10 | **Real audio capture via cpal.** Records from default input device (mic or BlackHole). Real-time downsampling to 16kHz mono. Graceful stop via AtomicBool. list_input_devices() for diagnostics. |
| P1a.3 | DONE | 10/10 | WAV writing via hound. Temp WAV cleanup on pipeline completion. |
| P1a.4 | DONE | 10/10 | **whisper-rs + symphonia integrated.** Real transcription working on M4 Max (146ms for 3s audio). Format conversion: m4a/mp3/ogg/wav. Feature flag for test builds without model. 5 new unit tests. |
| P1a.5 | DONE | 10/10 | Markdown writer: YAML frontmatter, 0600 perms, collision handling, memo/meeting templates, no-speech marker. 5 tests. |
| P1a.6 | DONE | 10/10 | CLI: record, stop, status, search, list, process, setup, logs. PID lifecycle. Real audio capture via cpal. Signal handling (Ctrl-C → stop + transcribe). JSON output for MCPB. |

### Chunk 2: Config + Infrastructure (P1a.7-8, P1a.14-15)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.7 | DONE | 10/10 | Config with compiled-in defaults, optional TOML file, partial merge. 4 tests. |
| P1a.8 | DONE | 10/10 | `minutes setup --model small` downloads from HuggingFace via curl. Detects existing models. Lists audio devices. Shows config hints. |
| P1a.14 | DONE | 10/10 | logging.rs + pipeline wired. JSON lines to ~/.minutes/logs/, rotation, log_step called for transcribe + write + pipeline_complete. |
| P1a.15 | NOT STARTED | - | Test fixtures (5s WAV, mock data) — defer to P1a.16 edge case pass |

### Chunk 3: Watcher + Voice Memos (P1a.11-13, P1a.12)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.11 | DONE | 9/10 | Folder watcher: notify event loop, settle delay, lock file, move to processed/failed, skip processed/failed subdirs, process existing files on start. 10 tests. Missing: real whisper transcription (uses placeholder). |
| P1a.12 | DONE | 10/10 | Memo frontmatter: `type: memo`, `source: voice-memo`, `status: transcript-only/no-speech`. Separate memos/ subdirectory. |
| P1a.13 | NOT STARTED | - | Apple Shortcut (.shortcut file) — needs manual creation in Shortcuts app |

### Chunk 4: Polish + Edge Cases (P1a.9-10, P1a.16)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.9 | DONE | 9/10 | README.md with install, usage, config, Claude integration sections. LICENSE (MIT). Missing: CONTRIBUTING.md. |
| P1a.10 | DONE | 10/10 | Git repo initialized, main branch, 2 commits. GitHub repo creation pending (needs `gh repo create`). |
| P1a.16 | DONE | 9/10 | 8 integration tests: full pipeline (meeting + memo), empty audio, permissions, collision, search filter, auto-create dir, nonexistent file. Missing: edge case unit tests for logging rotation, search special chars. |

## Chunk Gates
- [x] Chunk 1 gate: `minutes record` → `minutes stop` → markdown file appears (with placeholder transcription)
- [x] Chunk 2 gate: `minutes setup --list` works, logging module built, 41 tests pass
- [x] Chunk 3 gate: `minutes process` on .wav → markdown in memos/ (watcher module built, tested)
- [x] Chunk 4 gate: `cargo test` (41 pass), `cargo clippy` clean, `cargo fmt` clean

## Remaining for 10/10 on all beads
- P1a.13: Apple Shortcut (manual creation needed — not automatable from CLI)
- P1a.15: Dedicated 5s WAV test fixture file (currently generated via hound in tests)
- P2.7: Package as .mcpb (needs MCPB spec research)
- P2b.7-8: Hooks (SessionStart inject meeting context, PostToolUse auto-tag)
- P1a.13: Create Apple Shortcut (.shortcut file)
- P1a.14: Wire pipeline to call log_step() (currently tracing only)
- P1a.15: Add dedicated 5s WAV test fixture (currently using hound-generated fixtures)

## What's buildable now
- `cargo build` — compiles clean
- `cargo test` — 41 tests pass (33 unit + 8 integration)
- `cargo clippy -- -D warnings` — clean
- `cargo fmt --check` — clean
- `minutes record` — creates placeholder WAV, Ctrl-C transcribes + saves markdown
- `minutes process <file>` — processes any WAV through pipeline
- `minutes search <query>` — searches meeting files
- `minutes list` — lists all meetings/memos
- `minutes status` — shows recording status (JSON)
- `minutes watch` — watches folder for new audio files
- `minutes setup --list` — shows available whisper models

### Phase 1b: Intelligence Layer
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1b.1 | DONE | 9/10 | Diarization via pyannote subprocess. Speaker segment parsing, apply_speakers() for transcript labeling. Graceful fallback. 6 tests. |
| P1b.2 | NOT STARTED | - | Speaker-to-name mapping (calendar attendees → speaker labels) |
| P1b.3 | DONE | 9/10 | LLM summarization: Claude, OpenAI, Ollama engines. Map-reduce chunking. Structured output (key points, decisions, action items). 6 tests. |
| P1b.4 | DONE | 9/10 | Summary template with markdown sections (## Decisions, ## Action Items with checkboxes) |
| P1b.5 | NOT STARTED | - | Calendar integration (ical file parsing) |
| P1b.6 | DONE | 10/10 | Already built in Phase 1a (search + list commands) |
| P1b.7 | NOT STARTED | - | End-to-end test with real meeting audio |

### Phase 2: MCP Server
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P2.1 | DONE | 10/10 | MCP scaffold: package.json, tsconfig, TypeScript MCP server |
| P2.2 | DONE | 10/10 | start_recording tool (spawns detached minutes record process) |
| P2.3 | DONE | 10/10 | stop_recording tool (calls minutes stop, 3 min timeout) |
| P2.4 | DONE | 10/10 | list_meetings tool |
| P2.5 | DONE | 10/10 | search_meetings tool |
| P2.6 | DONE | 10/10 | get_meeting tool (reads full markdown file) |
| P2.7 | NOT STARTED | - | Package as .mcpb — needs MCPB spec research (P1a.0) |
| P2.8 | PARTIAL | 7/10 | Claude Desktop config template included. Full README pending. |

### Phase 2b: Claude Code Plugin
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P2b.1 | DONE | 10/10 | plugin.json manifest with skills + agent |
| P2b.2 | DONE | 10/10 | /minutes record skill |
| P2b.3 | DONE | 10/10 | /minutes search skill |
| P2b.4 | DONE | 10/10 | /minutes list skill |
| P2b.5 | DONE | 10/10 | /minutes recap skill |
| P2b.6 | DONE | 10/10 | meeting-analyst agent (cross-meeting intelligence) |
| P2b.7-8 | NOT STARTED | - | Hooks (SessionStart, PostToolUse) — nice-to-have |
| P2b.9 | NOT STARTED | - | MCP config in plugin (.mcp.json) |
| P2b.10 | NOT STARTED | - | Plugin README |

### Phase 3: Tauri Menu Bar App
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P3.1 | DONE | 9/10 | Tauri v2 scaffold: Cargo.toml, tauri.conf.json, system tray with menu. Tauri CLI 2.10.1 installed. |
| P3.2 | NOT STARTED | - | Calendar polling (macOS EventKit) |
| P3.3 | NOT STARTED | - | Meeting suggestion notification (2 min before) |
| P3.4 | DONE | 8/10 | Recording indicator via tray menu + status badge in web UI |
| P3.5 | DONE | 8/10 | Minimal web UI: dark-mode meeting list, search box, status indicator. Safe DOM rendering. |
| P3.6 | NOT STARTED | - | Auto-start on login (launchd) |
| P3.7 | NOT STARTED | - | First-run onboarding |
| P3.8 | NOT STARTED | - | Homebrew cask formula |

## Resume Instructions (for post-compaction)
1. Read this file to see current status
2. Read PLAN.md for task details and architecture decisions
3. Read CLAUDE.md for project conventions
4. Check `cargo build` status
5. Continue from the first NOT STARTED or IN PROGRESS bead
