# Minutes — Build Status

> This file tracks implementation progress. Read this after compaction to know exactly where you left off.
> Update this file after completing each bead. Never leave it stale.

## Current Phase: 10/10 Quality Sprint (post-adversarial review)

### 10/10 Sprint Beads
| Bead | Category | Task | Status |
|------|----------|------|--------|
| Q.1 | Security | Replace curl with ureq (pure Rust HTTP client) | DONE |
| Q.2 | Code Quality | Deduplicate frontmatter parsing → shared functions in markdown.rs | DONE |
| Q.3 | Code Quality | Make `cmd_list` delegate to `search("")` | DONE |
| Q.4 | Code Quality | Add `///` doc comments to all pub functions | DONE (100% coverage) |
| Q.5 | Tests | Atomic PID with flock (fs2) — prevents TOCTOU race | DONE |
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
| P1a.4 | 10/10 | Whisper.cpp + bounded hound/ffmpeg decoding. Real transcription. m4a/mp3/ogg/wav. |
| P1a.5 | 10/10 | Markdown writer. YAML frontmatter, 0600 perms, collision handling. |
| P1a.6 | 10/10 | CLI: 9 commands (record, stop, status, search, list, process, setup, logs, devices). |
| P1a.7 | 10/10 | Config with compiled-in defaults. TOML override. Partial merge. |
| P1a.8 | 10/10 | Model download from HuggingFace. |
| P1a.9 | 10/10 | README.md + LICENSE (MIT) + CONTRIBUTING.md. |
| P1a.10 | 10/10 | Git repo. GitHub: github.com/silverstein/minutes. |
| P1a.11 | 10/10 | Folder watcher: notify, settle delay, lock file, move processed/failed. |
| P1a.12 | 10/10 | Memo template: type: memo, source: voice-memo. |
| P1a.14 | 10/10 | Structured JSON logging + pipeline step logging. |
| P1a.16 | 10/10 | 73 tests (57 unit + 8 integration + 2 whisper + 8 MCP). |

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

## Phase 3: Tauri Desktop App — FUNCTIONAL
| Bead | Score | Summary |
|------|-------|---------|
| P3.1 | 10/10 | Tauri v2 scaffold. System tray menu. Compiles clean. |
| P3.2 | 10/10 | Main app window: meeting list, search, recording controls, date grouping. |
| P3.3 | 10/10 | Audio visualizer: real-time RMS level bars during recording. |
| P3.4 | 10/10 | Note taking: inline quick-note during recording + standalone popup window. |
| P3.5 | 10/10 | Tray state: red dot icon when recording, menu items gray out appropriately. |
| P3.6 | 10/10 | macOS entitlements: mic permission via Info.plist + entitlements.plist. |
| P3.7 | 10/10 | .app bundle: `cargo tauri build --bundles app` produces Minutes.app. |

## Infrastructure
| Item | Status |
|------|--------|
| Launchd watcher plist | Done (dev.getminutes.watcher.plist) |
| GitHub repo | Live: github.com/silverstein/minutes |
| Tests | 73 (57 unit + 8 integration + 2 whisper + 8 MCP), all passing |
| Clippy | Clean |
| Release build | 6.7MB binary |
| GitHub | github.com/silverstein/minutes — v0.1.0 tagged |
| Homebrew | brew tap silverstein/tap && brew install minutes |
| CI | GitHub Actions (local commit, needs workflow-scoped push) |
| Signed macOS release pipeline | Done (`.github/workflows/release-macos.yml` + `docs/release/platform-macos.md`) |
| Release channel policy | Done (`docs/release/channels.md`) |
| Reproducible release notes | Done (`scripts/release_notes.sh` + `docs/release/notes-index.md`) |
| Auto-update evaluation | Done (`docs/investigations/auto-update-evaluation.md`) |

## Bugs Fixed (2026-03-18 session)
| Bug | Impact | Fix |
|-----|--------|-----|
| WAV normalization: 16-bit samples divided by i32::MAX | Whisper saw silence (65,000x too quiet) | Divide by actual bit-depth max in transcribe.rs |
| No mic permission in Tauri app | Recording captured all zeros | Added Info.plist + entitlements.plist |
| Duplicate tray icons | Two icons in menu bar | Removed trayIcon from tauri.conf.json, use TrayIconBuilder only |
| Stop flag inverted | Recording exited immediately | Separate stop_flag AtomicBool from recording state |
| Whisper disabled in Tauri build | Got placeholder text instead of transcription | Removed default-features = false from Cargo.toml |
| min_words threshold too high (10) | Short recordings marked no-speech despite valid transcript | Lowered to 3 |

## Remaining (nice-to-haves for future sessions)
- P1b.2: Speaker-to-name mapping (calendar attendees → speaker labels)
- P1b.5: Calendar integration (ical file parsing)
- Calendar polling + meeting suggestion notifications
- Auto-start on login (plist exists, needs `minutes install-watcher` CLI command)
- First-run onboarding wizard
- Homebrew cask formula for Minutes.app
- P2.7: MCPB packaging (needs spec research)
- Phase 4: Cowork/Dispatch integration
- P4a.3: Structured intent extraction (decisions/actions as queryable YAML frontmatter + MCP filter)
- P4a.4: Decision consistency tracking (meeting-analyst flags contradictions and stale commitments)
- Phase 4: Cross-meeting intelligence (remaining P4a tasks)
- Apple Shortcut (.shortcut file for iPhone voice memos)
- Auto-gain calibration (measure ambient noise, adjust gain dynamically)
- Window close → hide to tray (instead of quitting)

## Resume Instructions (for post-compaction)
1. Read this file to see current status
2. Read PLAN.md for full architecture and task details
3. Read CLAUDE.md for project conventions
4. `cargo build` to verify everything compiles
5. `cargo test -p minutes-core --no-default-features` for fast tests
6. Continue from the "Remaining" list above
