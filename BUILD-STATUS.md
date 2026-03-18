# Minutes — Build Status

> This file tracks implementation progress. Read this after compaction to know exactly where you left off.
> Update this file after completing each bead. Never leave it stale.

## Current Phase: 1a — Recording Pipeline

## Build Chunks

### Chunk 1: Scaffold + Core Pipeline (P1a.0-6)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.0 | NOT STARTED | - | MCPB research blocker |
| P1a.1 | NOT STARTED | - | Rust workspace scaffold |
| P1a.2 | NOT STARTED | - | Audio capture (BlackHole + cpal) |
| P1a.3 | NOT STARTED | - | WAV file writing |
| P1a.4 | NOT STARTED | - | whisper.cpp + symphonia format conversion |
| P1a.5 | NOT STARTED | - | Markdown output (0600 perms) |
| P1a.6 | NOT STARTED | - | CLI (record, stop, status) + PID + signal handling |

### Chunk 2: Config + Infrastructure (P1a.7-8, P1a.14-15)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.7 | NOT STARTED | - | Config file with defaults |
| P1a.8 | NOT STARTED | - | Model download (minutes setup) |
| P1a.14 | NOT STARTED | - | Structured logging |
| P1a.15 | NOT STARTED | - | Test fixtures |

### Chunk 3: Watcher + Voice Memos (P1a.11-13, P1a.12)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.11 | NOT STARTED | - | Folder watcher (notify + settle + dedup) |
| P1a.12 | NOT STARTED | - | Memo-specific frontmatter template |
| P1a.13 | NOT STARTED | - | Apple Shortcut (.shortcut file) |

### Chunk 4: Polish + Edge Cases (P1a.9-10, P1a.16)
| Bead | Status | Score | Notes |
|------|--------|-------|-------|
| P1a.9 | NOT STARTED | - | README, LICENSE, CONTRIBUTING |
| P1a.10 | NOT STARTED | - | Git init, GitHub repo |
| P1a.16 | NOT STARTED | - | Edge case test pass |

## Chunk Gates
- [ ] Chunk 1 gate: `minutes record` → `minutes stop` → markdown file appears
- [ ] Chunk 2 gate: `minutes setup` works, structured logs appear, tests pass
- [ ] Chunk 3 gate: drop .m4a in inbox → markdown appears in memos/
- [ ] Chunk 4 gate: `cargo test` passes, `cargo clippy` clean, `cargo build --release` clean

## Resume Instructions (for post-compaction)
1. Read this file to see current status
2. Read PLAN.md for task details and architecture decisions
3. Read CLAUDE.md for project conventions
4. Check `cargo build` status
5. Continue from the first NOT STARTED or IN PROGRESS bead
