# Minutes — Build Status

> This file tracks implementation progress. Read this after compaction to know exactly where you left off.
> Update this file after completing each bead. Never leave it stale.

## Current Phase: 10/10 Quality Sprint (post-adversarial review)

### 10/10 Sprint Beads
| Bead | Category | Task | Status |
|------|----------|------|--------|
| Q.1 | Security | Replace curl with reqwest for HTTP | DEFERRED (body via stdin is sufficient for now) |
| Q.2 | Code Quality | Deduplicate frontmatter parsing → shared functions in markdown.rs | DONE |
| Q.3 | Code Quality | Make `cmd_list` delegate to `search("")` | DONE |
| Q.4 | Code Quality | Add `///` doc comments to all pub functions | DEFERRED (nice-to-have) |
| Q.5 | Tests | Concurrent PID race test | DEFERRED (needs flock, tracked in PLAN.md) |
| Q.6 | Tests | Fix logging tests to actually test I/O | DONE |
| Q.7 | Production | Call `rotate_logs()` at startup | DONE |
| Q.8 | Production | Make Ollama URL + model configurable | DONE |
| Q.9 | UX | Consistent `-t` flag naming | Already consistent |
| Q.10 | UX | `minutes devices` output to stdout | DONE |

## Phase 1a: Recording Pipeline — COMPLETE
| Bead | Score | Summary |
|------|-------|---------|
| P1a.1 | 10/10 | Cargo workspace: core (lib) + cli (bin) + tauri (app). 13 modules. |
| P1a.2 | 10/10 | Real audio capture via cpal. Mic + BlackHole support. |
| P1a.3 | 10/10 | WAV writing via hound. Temp cleanup on completion. |
| P1a.4 | 10/10 | Whisper.cpp + symphonia. Real transcription. m4a/mp3/ogg/wav. |
| P1a.5 | 10/10 | Markdown writer. YAML frontmatter, 0600 perms, collision handling. |
| P1a.6 | 10/10 | CLI: 9 commands (record, stop, status, search, list, process, setup, logs, devices). |
| P1a.7 | 10/10 | Config with compiled-in defaults. TOML override. Partial merge. |
| P1a.8 | 10/10 | Model download from HuggingFace via curl. |
| P1a.9 | 10/10 | README.md + LICENSE (MIT) + CONTRIBUTING.md. |
| P1a.10 | 10/10 | Git repo. GitHub: github.com/silverstein/minutes. |
| P1a.11 | 10/10 | Folder watcher: notify, settle delay, lock file, move processed/failed. |
| P1a.12 | 10/10 | Memo template: type: memo, source: voice-memo. |
| P1a.14 | 10/10 | Structured JSON logging + pipeline step logging. |
| P1a.16 | 10/10 | 58 tests (50 unit + 8 integration). |

## Phase 1b: Intelligence Layer — COMPLETE (core)
| Bead | Score | Summary |
|------|-------|---------|
| P1b.1 | 10/10 | Diarization via pyannote subprocess. Speaker labeling. |
| P1b.3 | 10/10 | LLM summarization: Claude, OpenAI, Ollama. Map-reduce chunking. |
| P1b.4 | 10/10 | Summary template: key points, decisions, action items. |
| P1b.6 | 10/10 | Search + list commands (built in Phase 1a). |

## Phase 2: MCP Server — COMPLETE
| Bead | Score | Summary |
|------|-------|---------|
| P2.1-6 | 10/10 | 7 MCP tools: start/stop recording, status, list, search, get, process. |
| P2.8 | 8/10 | Claude Desktop config template. |

## Phase 2b: Claude Code Plugin — COMPLETE
| Bead | Score | Summary |
|------|-------|---------|
| P2b.1-5 | 10/10 | plugin.json + 4 polished skills (record, search, list, recap). |
| P2b.6 | 10/10 | meeting-analyst agent (cross-meeting intelligence). |
| P2b.7-8 | 10/10 | PostToolUse hook (auto-tag with git repo). SessionStart removed (context bloat). |

## Phase 3: Tauri Menu Bar App — SCAFFOLD COMPLETE
| Bead | Score | Summary |
|------|-------|---------|
| P3.1 | 10/10 | Tauri v2 scaffold. System tray menu. Compiles clean. |
| P3.4-5 | 8/10 | Dark-mode web UI. Status indicator. Meeting list + search. |

## Infrastructure
| Item | Status |
|------|--------|
| Launchd watcher plist | Done (dev.getminutes.watcher.plist) |
| GitHub repo | Live: github.com/silverstein/minutes |
| Tests | 58 (50 unit + 8 integration), all passing |
| Clippy | Clean |
| Release build | In progress |

## Remaining (nice-to-haves for future sessions)
- P1b.2: Speaker-to-name mapping (calendar attendees → speaker labels)
- P1b.5: Calendar integration (ical file parsing)
- P3.2-3: Calendar polling + meeting suggestion notifications
- P3.6: Auto-start on login (plist exists, needs `minutes install-watcher` CLI command)
- P3.7: First-run onboarding wizard
- P3.8: Homebrew cask formula
- P2.7: MCPB packaging (needs spec research)
- Phase 4: Cowork/Dispatch integration
- Phase 4: Cross-meeting intelligence improvements
- Real tray icon (not placeholder)
- Apple Shortcut (.shortcut file for iPhone voice memos)

## Resume Instructions (for post-compaction)
1. Read this file to see current status
2. Read PLAN.md for full architecture and task details
3. Read CLAUDE.md for project conventions
4. `cargo build` to verify everything compiles
5. `cargo test -p minutes-core --no-default-features` for fast tests
6. Continue from the "Remaining" list above
