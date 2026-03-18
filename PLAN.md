# Minutes — Project Plan

> **Name**: `minutes`
> **Tagline**: Every meeting, every idea, every voice note — searchable by your AI
> **Domain**: getminutes.dev (to register)
> **Registries**: crates.io (available), PyPI (available), npm (@minutes/cli or scoped)
> **Created**: 2026-03-17
> **Author**: Mat Silverstein
> **License**: MIT

---

## Vision

An open-source, privacy-first tool that turns any audio — meetings, voice memos, brain dumps — into searchable, AI-queryable memory. Not a meeting notes app — a **conversation memory layer** that integrates natively with the Claude ecosystem (MCPB, Cowork, Dispatch) while supporting any LLM.

### Core Insight

Build the intelligence **inside the AI**, not next to it. Granola and Meetily are standalone apps that produce notes from meetings. This produces memory from *any conversation* — including the ones you have with yourself — that your AI assistant can recall mid-conversation.

The pipeline is the product, not the meeting. The same transcribe → summarize → store → search pipeline works on a 45-minute team standup and a 30-second voice memo recorded while walking the dog. Meetings are episodic (3-5/week); voice memos are constant. This turns Minutes from "a tool I use during meetings" into "a tool I think with every day."

> "Claude, what did Sarah say about pricing in last Tuesday's call?" just works.
> "Claude, what was that idea I had about onboarding while driving yesterday?" also works.

### Why Now

- Granola went paid ($18/mo) + stores data on their servers
- Meetily (10K stars) has no diarization, no knowledge graph, no AI agent integration, no mobile
- MCPB is brand new — first meeting tool as a Claude extension wins mindshare
- Claude Cowork + Dispatch enables mobile meeting recording (phone → Mac pipeline)
- QMD growing fast — knowledge graph integration is a differentiator
- Tauri v2 + Rust are mature for cross-platform native apps

---

## Competitive Landscape

| | Granola | Meetily | Otter.ai | **This Project** |
|--|---------|---------|----------|-------------------|
| Local transcription | No (cloud) | Yes | No | **Yes** |
| Speaker diarization | Yes | **No** | Yes | **Yes** (pyannote / sherpa-onnx) |
| Knowledge graph | No | No | No | **Yes** (QMD/Obsidian/PARA) |
| AI agent integration | No | No | No | **Yes** (MCPB → Claude Desktop) |
| Mobile trigger | No | No | Yes (app) | **Yes** (Dispatch) |
| Calendar-aware | Yes | No | Yes | **Yes** |
| BYO-LLM | No | Partial | No | **Yes** |
| Open source | No | Yes | No | **Yes** (MIT) |
| Free | No ($18/mo) | Freemium | Freemium | **Yes** |
| Data ownership | Their servers | Local | Their servers | **Local** |
| Cross-meeting intelligence | No | No | No | **Yes** |
| People memory | No | No | No | **Yes** |
| Voice memos / any audio | No | No | No | **Yes** (folder watcher, iPhone sync) |
| Platform | macOS | Win/Mac/Linux | Web/mobile | **macOS first, then cross-platform** |

---

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                    minutes                                      │
│              "conversation memory for AI assistants"                   │
│                                                                       │
│  ┌──────────┐                                                       │
│  │ Capture   │   Live recording (meetings, calls)                    │
│  │ BlackHole │                                                       │
│  │ /ScreenCap│──┐                                                    │
│  └──────────┘  │                                                     │
│                 │  ┌───────────┐   ┌───────────┐   ┌──────────────┐ │
│  ┌──────────┐  ├─▶│Transcribe │──▶│ Diarize   │──▶│ Summarize    │ │
│  │ Watch     │  │  │           │   │ (optional)│   │              │ │
│  │ Folder    │──┘  │whisper.cpp│   │ pyannote /│   │ Claude /     │ │
│  │           │     │(local,    │   │ sherpa-   │   │ Ollama /     │ │
│  │ Voice     │     │ Apple Si  │   │ onnx      │   │ OpenAI /     │ │
│  │ Memos,    │     │ optimized)│   │ (skip for │   │ any LLM      │ │
│  │ any .m4a/ │     │           │   │  memos)   │   │ (pluggable)  │ │
│  │ .wav file │     │           │   │           │   │              │ │
│  └──────────┘     └───────────┘   └───────────┘   └──────┬───────┘ │
│                                                          │           │
│  ┌───────────────────────────────────────────────────────▼─────────┐ │
│  │ Memory Store (local markdown, YAML frontmatter)                 │ │
│  │                                                                 │ │
│  │ ~/meetings/2026-03-17-advisor-pricing-call.md                   │ │
│  │ ┌────────────────────────────────────────────────────────────┐  │ │
│  │ │ ---                                                        │  │ │
│  │ │ title: Advisor Pricing Discussion                          │  │ │
│  │ │ date: 2026-03-17T14:00:00                                  │  │ │
│  │ │ duration: 42m                                               │  │ │
│  │ │ attendees: [Logan G., Mat S.]                               │  │ │
│  │ │ calendar_event: X1 Weekly Sync                              │  │ │
│  │ │ tags: [pricing, advisor, x1]                                │  │ │
│  │ │ people: [[logan-gunderson], [[mat]]]                        │  │ │
│  │ │ ---                                                         │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Summary                                                  │  │ │
│  │ │ - Agreed to price advisor platform at $499/mo minimum       │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Decisions                                                │  │ │
│  │ │ - [x] Advisor pricing must pass Garrett fairness test       │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Action Items                                             │  │ │
│  │ │ - [ ] @mat: Send pricing doc to Sarah by Friday             │  │ │
│  │ │ - [ ] @logan: Review competitor pricing grid                │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Transcript                                               │  │ │
│  │ │ [LOGAN 0:00] So I think the pricing for advisors should...  │  │ │
│  │ │ [MAT 0:45] Right, but the fairness test says...             │  │ │
│  │ └────────────────────────────────────────────────────────────┘  │ │
│  │                                                                 │ │
│  │                                                                 │ │
│  │ ~/meetings/memos/2026-03-17-onboarding-idea.md                  │ │
│  │ ┌────────────────────────────────────────────────────────────┐  │ │
│  │ │ ---                                                        │  │ │
│  │ │ title: Onboarding flow idea                                │  │ │
│  │ │ type: memo                                                  │  │ │
│  │ │ date: 2026-03-17T08:15:00                                  │  │ │
│  │ │ duration: 1m 22s                                            │  │ │
│  │ │ source: voice-memos                                         │  │ │
│  │ │ tags: [onboarding, product, idea]                           │  │ │
│  │ │ ---                                                         │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Summary                                                  │  │ │
│  │ │ - Skip the wizard. Drop users into a pre-populated demo    │  │ │
│  │ │   workspace, let them poke around, then ask "ready to      │  │ │
│  │ │   connect your own data?"                                   │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Transcript                                               │  │ │
│  │ │ [0:00] Okay so I just had an idea about the onboarding...  │  │ │
│  │ └────────────────────────────────────────────────────────────┘  │ │
│  │                                                                 │ │
│  │ Indexed by: QMD, Obsidian, Logseq, any markdown tool           │ │
│  └─────────────────────────────────────────────────────────────────┘ │
│                                                                       │
│  Distribution layers (all optional, each adds value):                 │
│                                                                       │
│  ┌──────────────┐  ┌───────────────┐  ┌──────────────────────────┐   │
│  │ CLI           │  │ MCPB          │  │ Menu Bar (Tauri v2)      │   │
│  │ minutes record│  │ Claude        │  │ Calendar-aware           │   │
│  │ minutes stop  │  │ Desktop       │  │ "Record" at meeting      │   │
│  │ minutes status│  │ extension     │  │ time, like Granola       │   │
│  │ minutes watch │  │ (one-click)   │  │                          │   │
│  │ minutes search│  │               │  │ Voice memo watcher       │   │
│  │ minutes list  │  │ For: Claude   │  │ built into settings      │   │
│  │ minutes setup │  │ Desktop,      │  │                          │   │
│  │ minutes logs  │  │ Cowork,       │  │ For: Everyone            │   │
│  │               │  │ Dispatch      │  │                          │   │
│  │ For: Claude   │  │               │  │                          │   │
│  │ Code, termi-  │  │               │  │                          │   │
│  │ nal users     │  │               │  │                          │   │
│  └──────────────┘  └───────────────┘  └──────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────┘
```

---

## Tech Stack

| Component | Technology | Rationale |
|-----------|-----------|-----------|
| **Audio engine** | Rust | Cross-platform, fast, memory-safe. Single binary. |
| **Transcription** | whisper.cpp (via Rust bindings) | Local, Apple Silicon optimized, best open-source STT |
| **Diarization** | pyannote (subprocess) or sherpa-onnx (native) | Pluggable: pyannote for best quality, sherpa-onnx for no-Python mode |
| **Menu bar app** | Tauri v2 | Rust backend + web frontend, ~10MB vs Electron's 150MB |
| **CLI** | Rust (clap) | Same binary as engine, zero extra deps |
| **MCPB wrapper** | Node.js (TypeScript) | Required by Claude Desktop extension format |
| **Summarization** | Pluggable (Claude API, Ollama, OpenAI, etc.) | BYO-LLM, no vendor lock-in |
| **Meeting store** | Markdown + YAML frontmatter | Universal, works with QMD/Obsidian/grep |
| **Calendar** | iCal file / Google API (optional) | Auto-suggest recording, enrich with attendees |

### Why Tauri over Swift or Electron

| | Swift | Tauri v2 | Electron |
|--|-------|----------|----------|
| Cross-platform | macOS only | macOS + Windows + Linux | All |
| Binary size | ~2MB | ~10MB | ~150MB |
| Audio capture | Native (ScreenCaptureKit) | Via Rust plugin (ScreenCaptureKit) | Via native module |
| Contributor base | Apple devs only | Rust + web devs | Web devs |
| Language consistency | Swift + Node.js (for MCPB) | Rust + TypeScript (shared with MCPB) | JS everywhere but bloated |
| Open source traction | Lower | **High and growing** | Declining |

**Decision**: Tauri v2 with Rust plugins for audio capture. The Rust backend is shared between CLI, Tauri app, and the native engine — one codebase, three distribution formats.

### Rust Module Structure

Single `minutes-core` library crate with internal module boundaries. Thin `minutes-cli` binary crate on top.

```
crates/core/src/
├── lib.rs              # Re-exports public API
├── capture.rs          # Audio capture (BlackHole/cpal)
├── transcribe.rs       # whisper-rs + audio format conversion (symphonia: m4a/mp3/ogg → WAV)
├── pipeline.rs         # Orchestrates capture → transcribe → [diarize] → [summarize] → write
├── watch.rs            # Folder watcher (notify + settle delay + dedup)
├── search.rs           # Walk-dir + regex search (builtin engine)
├── config.rs           # TOML config with compiled-in defaults (Config::default())
├── markdown.rs         # YAML frontmatter + markdown writer (meeting + memo templates)
├── pid.rs              # PID file lifecycle (create → check → stale recovery → clean)
├── logging.rs          # JSON structured logging (daily rotation, 7 days)
└── error.rs            # Per-module error enums unified via MinutesError (thiserror)

crates/cli/src/
└── main.rs             # clap arg parsing → calls core:: functions
                        # Commands: record, stop, status, watch, search, list, setup, logs
```

**Design decisions (from eng review):**
- **Single crate, internal modules** — split into separate crates only if compile times or Tauri's dependency subset forces it
- **Simple pipeline function** — `pipeline::process()` calls each step with if-guards for optional steps (diarize, summarize). No trait-based step abstraction. Explicit > clever.
- **Per-module error enums** — `CaptureError`, `TranscribeError`, `WatchError`, etc. unified at crate level via `MinutesError` with `#[from]` conversions. CLI matches for user-facing messages.
- **Config::default()** — compiled-in defaults, config file optional. `minutes record` works without a config file if BlackHole is installed and model is downloaded.
- **Audio format conversion** — `symphonia` crate decodes m4a/mp3/ogg to WAV in-process before transcription. No ffmpeg dependency.

---

## PARA Integration (for QMD/Obsidian users)

Meetings and memos become first-class PARA entities:

```
~/Documents/life/           (or configurable path)
├── areas/
│   └── meetings/                              # QMD collection
│       ├── 2026-03-17-x1-weekly-standup.md
│       ├── 2026-03-17-advisor-demo-call.md
│       ├── memos/                             # Voice memos subfolder
│       │   ├── 2026-03-17-onboarding-idea.md
│       │   ├── 2026-03-17-pricing-thought.md
│       │   └── ...
│       └── ...
├── areas/people/
│   └── logan-gunderson/
│       └── summary.md    # Auto-linked from meeting attendees
└── memory/
    └── 2026-03-17.md     # Daily note gets backlinks:
                           # "## Meetings\n- [[meetings/...]]"
                           # "## Voice Memos\n- [[meetings/memos/...]]"
```

**QMD integration** (optional — zero required deps):
```bash
qmd collection add meetings ~/meetings    # Indexes both meetings and memos
qmd search "what did we decide about pricing" -c meetings
qmd search "that idea about onboarding" -c meetings
```

---

## Phase Plan

### Phase 1: CLI — "It records and transcribes"

**Revised timeline**: 2 weeks (was 1 — see adversarial review below).

Phase 1 is split into two milestones to de-risk the hardest part (audio capture) before layering intelligence.

#### Phase 1a: Recording Pipeline (Week 1) — "Capture → Transcribe → Save"

**Goal**: `minutes record` / `minutes stop` — records system audio, transcribes locally, saves raw transcript as markdown. No diarization, no LLM summary. Get the pipeline solid first.

| Task | Description | Beads ID |
|------|-------------|----------|
| **P1a.0** | **BLOCKER: MCPB native binary research.** Can an MCPB package bundle a Rust binary? Is there a postinstall hook? Test with a hello-world `.mcpb` that shells out to a native binary. If MCPB can't bundle binaries, Phase 2 architecture must change. **Spend 2 hours on this in week 1, not week 2.** | TBD |
| P1a.1 | Rust project scaffold (cargo workspace: `core`, `cli`) | TBD |
| P1a.2 | Audio capture via BlackHole virtual audio device + `cpal` crate (NOT ScreenCaptureKit — see note below) | TBD |
| P1a.3 | WAV file writing (capture → save to temp .wav, clean up temp WAV after transcription) | TBD |
| P1a.4 | whisper.cpp integration via `whisper-rs` crate (batch transcription of .wav → text). **Audio format conversion**: use `symphonia` crate to decode .m4a/.mp3/.ogg → WAV before transcription (whisper-rs only reads WAV natively). Handle empty transcripts: save markdown with `[No speech detected]` marker + `status: no-speech` in frontmatter. Minimum word threshold: 10 (configurable). | TBD |
| P1a.5 | Markdown output with YAML frontmatter (title, date, duration, raw transcript). **File permissions: `0600`** (owner read/write only — transcripts contain sensitive content). | TBD |
| P1a.6 | CLI interface: `minutes record` (start), `minutes stop` (stop + transcribe + save), **`minutes status`** (is recording in progress? duration so far?). See IPC Protocol below. | TBD |
| P1a.7 | Config file (`~/.config/minutes/config.toml` — output dir, whisper model path, search engine, watch settings) | TBD |
| P1a.8 | Model download helper: `minutes setup` — downloads whisper `small` model by default (466MB, best quality/size tradeoff). `minutes setup --model large-v3` for best quality (3.1GB). `minutes setup --list` shows all available models with sizes. | TBD |
| P1a.9 | README, LICENSE (MIT), .gitignore, basic docs, CONTRIBUTING.md | TBD |
| P1a.10 | Git init, GitHub repo creation | TBD |
| P1a.11 | Folder watcher mode: `minutes watch <dir>` — watches a folder for new audio files (.m4a, .wav, .mp3), runs each through the transcription pipeline automatically. See Watch Protocol below for dedup, settle delay, and locking. | TBD |
| P1a.12 | Memo-specific frontmatter template (`type: memo`, no attendees/calendar, `source:` field for origin tracking) | TBD |
| P1a.13 | Apple Shortcut: "Save to Minutes" — downloadable `.shortcut` file that adds a share sheet action on iPhone to save audio to `iCloud Drive/minutes-inbox/`, which syncs to Mac and gets picked up by `minutes watch` | TBD |
| P1a.14 | Structured logging: JSON lines to `~/.minutes/logs/minutes.log`, daily rotation (7 days). Every pipeline step logs file, step, duration, outcome. `minutes logs` command to tail. `minutes logs --errors` to filter. `--verbose` CLI flag for stderr debug output. | TBD |
| P1a.15 | Test fixtures: 5-second WAV in `tests/fixtures/` (~800KB), mock data for transcript/diarization/summary parsing. Integration test runs full pipeline on fixture. | TBD |
| P1a.16 | Edge case test pass: every error variant in `error.rs` has at least one test. Covers: partial config merge, filename collisions, settle delay, lock files, special chars in search, no-speech template, 0600 permissions, auto-create output dir, move-to-failed, wrong extension skip. | TBD |

**Exit criteria**: `minutes record` → talk for 2 minutes → `minutes stop` → markdown file appears in `~/meetings/` with raw transcript. AND: drop a voice memo .m4a into a watched folder → markdown appears in `~/meetings/memos/`. No AI, no diarization — just reliable local capture + transcription.

> **Voice Memos — iPhone → Mac pipeline (macOS permissions reality):**
>
> The obvious path — watching Apple's Voice Memos sync folder at `~/Library/Group Containers/group.com.apple.VoiceMemos.shared/Recordings/` — **requires Full Disk Access** for the `minutes` binary. This is a TCC-protected path on modern macOS. Asking open-source users to grant FDA to a binary from GitHub is a tough sell and a legitimate security concern.
>
> **Instead, use an unprotected inbox folder:**
>
> ```
> ~/.minutes/inbox/          ← default, no FDA needed
> ```
>
> Getting audio from iPhone to this folder:
>
> | Method | Friction | How it works |
> |--------|----------|-------------|
> | **Apple Shortcut** (recommended) | One tap | We ship a `.shortcut` file. User installs once. Voice Memos → Share → "Save to Minutes" → syncs via iCloud Drive to `~/Library/Mobile Documents/com~apple~CloudDocs/minutes-inbox/` (user-created iCloud Drive folders are accessible without FDA) |
> | **Shortcuts Automation** | ~One tap | iPhone Shortcuts app: "When Voice Memos closes" → save last recording to iCloud Drive/minutes-inbox. **Caveat:** iOS still requires a notification tap to confirm most app-trigger automations — not truly silent. Marginally better than the Share Sheet shortcut |
> | **AirDrop** | Two taps | AirDrop to Mac → lands in `~/Downloads/`. Configure `minutes watch ~/Downloads --filter "*.m4a"` |
> | **Files app** | Two taps | Voice Memos → Share → Save to Files → minutes-inbox folder |
> | **Direct FDA** (power users) | One-time setup | Grant FDA to `minutes` binary in System Settings. Then watch the Voice Memos container directly |
>
> The Apple Shortcut approach is shipped as part of the project (P1a.13). It's one-time install, and then every voice memo is one tap away from being transcribed.

#### IPC Protocol: Recording Lifecycle (PID file + signals)

All interfaces (CLI, MCPB, Tauri) use the same protocol to manage recording state.

**Key design decision:** `minutes record` runs as a **foreground process** (not a daemon). The recording process itself runs the transcription pipeline on shutdown (SIGTERM/SIGINT/Ctrl-C). The `minutes stop` command just signals and waits. This keeps the pipeline in-process — no cross-process data transfer needed.

```
# Start recording (foreground, blocks terminal)
minutes record
  → Checks ~/.minutes/recording.pid — if exists AND process alive: error "Already recording"
  → If PID file exists but process dead: stale recovery (clean up, log warning)
  → Starts audio capture, writes PID to ~/.minutes/recording.pid
  → Writes audio to ~/.minutes/current.wav
  → Prints: "Recording... (press Ctrl-C or run `minutes stop`)"

# Check status (from another terminal or MCPB)
minutes status
  → Reads PID file, checks if process alive
  → stdout: {"recording": true, "duration": "4m23s", "pid": 12345}
  → (or: {"recording": false})

# Stop recording (from another terminal or MCPB)
minutes stop
  → Reads ~/.minutes/recording.pid
  → Sends SIGTERM to PID
  → Polls for PID file removal (timeout: 120s for transcription to finish)
  → Reads ~/.minutes/last-result.json (written by record process on completion)
  → stdout: {"status": "done", "file": "~/meetings/2026-03-17-...md"}

# What the record process does on SIGTERM / Ctrl-C:
  → Catches signal, stops audio capture
  → Flushes WAV file
  → Runs pipeline: transcribe → [diarize] → [summarize] → write markdown
  → Writes result to ~/.minutes/last-result.json
  → Cleans up PID file and temp WAV (on success; keeps WAV on failure for retry)
  → Exits 0

# Crash recovery
minutes record (PID file exists but process dead)
  → Detects stale PID, cleans up PID file
  → If current.wav exists, offers to process it: "Found unprocessed recording. Process it? [Y/n]"
  → Starts new recording
```

**Signal handling note:** Ctrl-C (SIGINT) and `minutes stop` (SIGTERM) trigger the **same** shutdown path — stop capture, run pipeline, write result, clean up. The record process must register signal handlers that set an atomic flag, which the capture loop checks to break gracefully.

#### Watch Protocol: Folder Watcher Lifecycle

```
~/.minutes/inbox/              ← watched folder (default, no FDA needed)
├── new-voice-memo.m4a         ← pending (just arrived)
├── processed/                 ← successfully processed
│   ├── 2026-03-17-idea.m4a
│   └── 2026-03-16-note.m4a
└── failed/                    ← processing failed (not retried automatically)
    └── corrupted-file.m4a

~/.minutes/watch.lock          ← prevents two watchers running simultaneously
```

**Settle delay** (handles iCloud sync race condition): When a new file is detected, wait `settle_delay_ms` (default: 2000ms), then check file size. Wait again, check again. Only process when size is stable across two consecutive checks AND file size > 0. This prevents processing partially-synced iCloud/AirDrop files.

**Dedup**: After successful processing → move source to `inbox/processed/`. On failure → move to `inbox/failed/`. Files in `processed/` and `failed/` are never reprocessed automatically. User can manually retry with `minutes process <path>`.

**Lock file**: `minutes watch` acquires `~/.minutes/watch.lock` on start. If lock already held → error: "Another watcher is running (PID: X)". Prevents race conditions from two watchers processing the same file.

**Model memory**: Whisper model is lazy-loaded on first file detection, kept in memory for subsequent files. If no files arrive for 5+ minutes, model is unloaded to free ~1GB RAM. Re-loaded on next file.

> **Why BlackHole, not ScreenCaptureKit for Phase 1:**
> ScreenCaptureKit requires an **app bundle with entitlements** — a standalone CLI binary can't use it without being signed and notarized by Apple. For a Phase 1 that's CLI-only, this is a blocking constraint. BlackHole is a virtual audio device that any process can read from via standard audio APIs (`cpal` crate). The trade-off: users must install BlackHole and set up a Multi-Output Device in Audio MIDI Setup (one-time, ~3 min). Phase 3's Tauri app gets ScreenCaptureKit since it has an app bundle.

#### Phase 1b: Intelligence Layer (Week 2) — "Diarize + Summarize"

**Goal**: Layer speaker diarization and LLM summarization on top of the working pipeline.

| Task | Description | Beads ID |
|------|-------------|----------|
| P1b.1 | Speaker diarization integration (see Diarization Decision below) | TBD |
| P1b.2 | Speaker-to-name mapping (calendar attendees → speaker labels) | TBD |
| P1b.3 | LLM summarization module — pluggable: Claude API, Ollama, OpenAI. **Map-reduce chunking** for transcripts exceeding context window: chunk by time/speaker turn, summarize each chunk, produce final summary from chunk summaries. If no LLM configured → skip gracefully, save transcript-only markdown. | TBD |
| P1b.4 | Summary template system (configurable output: decisions, action items, key points) | TBD |
| P1b.5 | Calendar integration (ical file parsing for meeting context + attendees) | TBD |
| P1b.6 | CLI: `minutes list` (list recent meetings) and `minutes search <query>` (full-text search) | TBD |
| P1b.7 | End-to-end test: record real meeting → diarized transcript + AI summary → markdown | TBD |

**Exit criteria**: Record a real meeting → get diarized transcript with speaker names + AI-generated summary with decisions and action items → saved as searchable markdown.

#### Diarization Decision (MUST RESOLVE BEFORE P1b.1)

**Falcon is NOT viable for MIT open-source distribution.** Picovoice Falcon is free for personal use but requires a commercial license for redistribution. This is a hard conflict with MIT licensing.

**Viable alternatives (in priority order):**

| Option | License | Language | Quality | Speed | Integration |
|--------|---------|----------|---------|-------|-------------|
| **pyannote (community-1)** | MIT (model), AGPL (code) | Python | Best (DER ~11%) | Slow | Subprocess call — AGPL is fine since we don't link, just exec |
| **WhisperX** | BSD-4 | Python | Good (uses pyannote internally) | Fast (batched) | Subprocess — bundles whisper + diarization in one call |
| **speechbrain** | Apache 2.0 | Python | Decent | Medium | Subprocess — fully MIT-compatible |
| **sherpa-onnx** | Apache 2.0 | C++/Rust | Good | Fast | Native Rust FFI — no Python dependency |

**Recommended path**: Start with **pyannote via subprocess** (best quality, AGPL-safe as subprocess). If users don't want Python, offer **sherpa-onnx** as a pure-native alternative. Document both paths in config.

```toml
# ~/.config/minutes/config.toml

[transcription]
model = "small"                  # Default: small (466MB). Options: tiny, base, small, medium, large-v3
model_path = "~/.minutes/models" # Where whisper models are stored
min_words = 10                   # Below this threshold, mark as "no-speech"

[diarization]
engine = "pyannote"  # or "sherpa-onnx" for no-Python mode
# engine = "none"    # skip diarization entirely

[summarization]
# engine = "claude"             # Claude API (requires ANTHROPIC_API_KEY env var)
# engine = "ollama"             # Local Ollama (requires ollama running)
# engine = "openai"             # OpenAI API (requires OPENAI_API_KEY env var)
engine = "none"                  # Default: no summarization (transcript only)
chunk_max_tokens = 4000          # Max tokens per chunk for map-reduce summarization

[search]
engine = "builtin"               # Default: walk dir + regex (zero dependencies)
# engine = "qmd"                 # Use QMD for semantic search (requires qmd installed)
# qmd_collection = "meetings"

[security]
# Directories allowed for process_audio MCP tool (prevents path traversal)
allowed_audio_dirs = [
  "~/.minutes/inbox",
  "~/meetings",
]

[watch]
# Folders to watch for new audio files (voice memos, recordings, etc.)
# Processed files go to output_dir/memos/
paths = [
  "~/.minutes/inbox",                    # Default inbox — drop any audio here
  # "~/Library/Mobile Documents/com~apple~CloudDocs/minutes-inbox",  # iCloud Drive (syncs from iPhone Shortcut, no FDA needed)
  # "~/Downloads",                       # Watch Downloads for AirDrop'd audio
  #
  # ⚠️  The path below requires Full Disk Access for the minutes binary.
  # Only uncomment if you've granted FDA in System Settings > Privacy & Security.
  # "~/Library/Group Containers/group.com.apple.VoiceMemos.shared/Recordings",
]
extensions = ["m4a", "wav", "mp3", "ogg", "webm"]  # Only process these file types
type = "memo"                # Default type for watched files (memo vs meeting)
diarize = false              # Skip diarization for single-speaker memos
delete_source = false        # Keep original audio (moved to processed/, not deleted)
settle_delay_ms = 2000       # Wait for file size to stabilize before processing (iCloud sync safety)
```

### Phase 2: MCPB (Week 2) — "Claude remembers your meetings"

**Goal**: Claude Desktop extension. One-click install. "What did we discuss last Tuesday?" works.

| Task | Description | Beads ID |
|------|-------------|----------|
| P2.1 | MCPB scaffold (manifest.json, Node.js MCP server) | TBD |
| P2.2 | MCP tool: `start_recording` (spawns Rust binary) | TBD |
| P2.3 | MCP tool: `stop_recording` (triggers pipeline) | TBD |
| P2.4 | MCP tool: `list_meetings` (reads meeting store) | TBD |
| P2.5 | MCP tool: `search_meetings` (full-text + frontmatter query) | TBD |
| P2.6 | MCP tool: `get_transcript` (returns specific meeting) | TBD |
| P2.7 | Package as .mcpb, test install in Claude Desktop | TBD |
| P2.8 | README for MCPB distribution | TBD |

**Exit criteria**: Install extension in Claude Desktop → record meeting → ask Claude about it → Claude answers from transcript.

### Phase 2b: Claude Code Plugin (Week 2, parallel with MCPB) — "Meeting skills in your terminal"

**Goal**: Claude Code users get `/minutes record`, `/minutes search`, `/minutes list` as skills. Meeting context enriches coding sessions. Publishable as a Claude Code plugin.

| Task | Description | Beads ID |
|------|-------------|----------|
| P2b.1 | Plugin scaffold: `plugin.json` manifest with name, version, description, components | TBD |
| P2b.2 | Skill: `/minutes record` — start/stop recording with hotkey awareness | TBD |
| P2b.3 | Skill: `/minutes search <query>` — search past meetings from terminal, render results in chat | TBD |
| P2b.4 | Skill: `/minutes list` — list recent meetings with summaries, attendees, dates | TBD |
| P2b.5 | Skill: `/minutes recap` — summarize today's meetings into a digest | TBD |
| P2b.6 | Agent: `meeting-analyst` — subagent for cross-meeting intelligence queries ("what did X say about Y?") | TBD |
| P2b.7 | Hook: `SessionStart` — inject recent meeting context if meetings exist from today | TBD |
| P2b.8 | Hook: `PostToolUse` — auto-tag meetings with current project/repo context when recording stops | TBD |
| P2b.9 | MCP server config in plugin (`.mcp.json`) — reuse MCPB's MCP tools within Claude Code | TBD |
| P2b.10 | Plugin README + install instructions (`claude plugin add minutes`) | TBD |

**Plugin structure:**
```
.claude/plugins/minutes/
├── plugin.json              # Manifest: skills, agents, hooks, mcp
├── skills/
│   ├── record/SKILL.md      # Start/stop recording
│   ├── search/SKILL.md      # Search meetings
│   ├── list/SKILL.md        # List meetings
│   └── recap/SKILL.md       # Daily digest
├── agents/
│   └── meeting-analyst.md   # Cross-meeting intelligence
├── hooks/
│   ├── session-start.mjs    # Inject meeting context
│   └── post-record.mjs      # Auto-tag with project context
└── .mcp.json                # MCP server (same as MCPB)
```

**Key design decisions:**
- Skills call the same Rust CLI binary (`minutes record`, `minutes search`) — no duplication
- The MCP server in `.mcp.json` is identical to the MCPB — one MCP server, two distribution formats
- `SessionStart` hook reads `~/meetings/` and injects a "Today's meetings" summary if any exist
- `PostToolUse` hook fires when `minutes stop` completes — reads the current git repo and adds `project: x1-wealth` (or whatever) to the meeting's YAML frontmatter
- The `meeting-analyst` agent has access to all meeting files and can answer cross-meeting questions autonomously

**Exit criteria**: `claude plugin add minutes` → `/minutes record` works → meeting context appears in Claude Code sessions → `/minutes search "pricing"` returns results.

### Phase 3: Tauri Menu Bar App (Week 3-4) — "Granola-like UX"

**Goal**: Calendar-aware menu bar app. Suggests recording 2 min before meetings. Granola UX, open-source.

| Task | Description | Beads ID |
|------|-------------|----------|
| P3.1 | Tauri v2 project setup (menu bar / system tray mode) | TBD |
| P3.2 | Calendar polling (macOS EventKit or ical) | TBD |
| P3.3 | Meeting suggestion notification (2 min before) | TBD |
| P3.4 | Recording indicator (menu bar icon glow/badge) | TBD |
| P3.5 | Minimal web UI: meeting list, transcript viewer, settings | TBD |
| P3.6 | Auto-start on login (launchd integration) | TBD |
| P3.7 | First-run onboarding (permissions, model download, LLM config) | TBD |
| P3.8 | Homebrew cask formula | TBD |

**Exit criteria**: Install via `brew install --cask minutes` → app sits in menu bar → suggests recording → produces notes like Granola.

### Phase 4: Intelligence + Cowork (Week 5+) — "Meeting memory, not meeting notes"

**Goal**: Cross-meeting intelligence, people memory, and full Claude Cowork/Dispatch integration.

#### 4a: Intelligence Layer

| Task | Description | Beads ID |
|------|-------------|----------|
| P4a.1 | Cross-meeting search ("what did we decide about X across all meetings?") | TBD |
| P4a.2 | People profiles — build attendee context over time (decisions, commitments, topics they care about) | TBD |
| P4a.3 | Decision tracking — extract decisions into a queryable index, track follow-through | TBD |
| P4a.4 | PARA entity auto-linking (meetings → people → projects) | TBD |
| P4a.5 | QMD collection auto-registration (`qmd collection add minutes ~/meetings`) | TBD |
| P4a.6 | Daily note backlinks (append meeting summaries to daily notes) | TBD |

#### 4b: Claude Cowork + Dispatch Integration

| Task | Description | Beads ID |
|------|-------------|----------|
| P4b.1 | **Cowork connector research** — investigate how Cowork connectors work, what APIs/protocols are available, whether MCPB tools are automatically available in Cowork | TBD |
| P4b.2 | **Dispatch recording flow** — "Start recording" from phone → Dispatch → Mac captures audio. Test end-to-end with Dispatch research preview | TBD |
| P4b.3 | **Cowork meeting brief** — when user starts a Cowork session, auto-surface "You had 3 meetings today, here's what happened" | TBD |
| P4b.4 | **Dispatch meeting summary** — after recording stops, push structured summary back to phone via Dispatch ("Done. 3 action items, 2 decisions.") | TBD |
| P4b.5 | **Cowork follow-up automation** — Claude autonomously drafts follow-up emails based on action items, sends via Cowork connectors (Gmail, Slack) | TBD |
| P4b.6 | **Multi-meeting synthesis in Cowork** — "Prepare me for my call with Sarah" → Cowork pulls all past meetings with Sarah, summarizes themes, open action items, relationship context | TBD |
| P4b.7 | **Cowork persistent memory** — meeting intelligence persists across Cowork sessions. Claude remembers what was discussed even weeks later | TBD |
| P4b.8 | **Dispatch quick commands** — from phone: "What did we decide yesterday?" / "Any action items for me?" / "Who mentioned the budget issue?" | TBD |

#### 4c: Platform Expansion

| Task | Description | Beads ID |
|------|-------------|----------|
| P4c.1 | Windows support (WASAPI audio capture) | TBD |
| P4c.2 | Linux support (PulseAudio/PipeWire capture) | TBD |
| P4c.3 | Obsidian community plugin (thin wrapper around CLI) | TBD |

**Cowork integration architecture:**
```
┌─────────────────────────────────────────────────────────────┐
│ User's Phone                                                 │
│                                                              │
│ Claude iOS/Android App                                       │
│ ├── "Start recording my meeting"     ──────┐                │
│ ├── "What did we decide yesterday?"  ──────┤  Dispatch       │
│ ├── "Prepare me for the Sarah call"  ──────┤  (sends to Mac) │
│ └── "Any action items for me?"       ──────┘                │
└──────────────────────────────┬──────────────────────────────┘
                               │
                               ▼
┌──────────────────────────────────────────────────────────────┐
│ User's Mac (Claude Desktop / Cowork)                          │
│                                                               │
│ ┌─────────────────────────────────────────────────────────┐  │
│ │ Minutes MCPB Extension                                   │  │
│ │                                                          │  │
│ │ MCP Tools (available to Cowork + Dispatch):              │  │
│ │ ├── start_recording  → spawns minutes binary             │  │
│ │ ├── stop_recording   → triggers pipeline                 │  │
│ │ ├── list_meetings    → reads ~/meetings/                 │  │
│ │ ├── search_meetings  → full-text + semantic search       │  │
│ │ ├── get_meeting      → full transcript + metadata        │  │
│ │ └── get_person_context → aggregated person profile       │  │
│ └─────────────────────────────────────────────────────────┘  │
│                                                               │
│ Cowork can also use:                                          │
│ ├── Gmail connector   → draft follow-up emails               │
│ ├── Calendar connector → check upcoming meetings              │
│ ├── Slack connector   → post meeting summaries to channels    │
│ └── File system       → read/write meeting markdown files     │
└──────────────────────────────────────────────────────────────┘
```

**Key Cowork insight:** MCPB tools are automatically available in Cowork. This means if Phase 2 (MCPB) is done well, Phase 4b is mostly about **crafting the right Cowork workflows** — the tool infrastructure is already there. The work is:
1. Testing that MCPB tools work reliably through Dispatch (which has ~50% success rate currently)
2. Building smart compound workflows ("prepare me for Sarah" = search_meetings + get_person_context + calendar lookup)
3. Ensuring meeting context persists across Cowork sessions (may need a session-start injection pattern)

**Exit criteria for Phase 4**: From phone: "Prepare me for my 2pm with Sarah" → Claude surfaces past meeting history, open action items, her key concerns, and suggested talking points — all from local meeting data, no cloud required.

---

## Cowork/Dispatch User Stories (Detailed)

These ground the Cowork integration in real scenarios:

### Story 1: Pre-Meeting Prep (from phone)
```
User (on phone, heading to a meeting):
  → Opens Claude app → Dispatch
  → "Prepare me for my meeting with the Johnsons at 2pm"

Claude (on Mac, via Cowork):
  → Calls list_meetings(attendee: "Johnson")
  → Calls get_person_context(name: "Johnson")
  → Calls calendar(event: "2pm today")
  → Synthesizes: "You've met the Johnsons 4 times. Last meeting (March 3):
    they discussed estate planning and asked about trusts.
    Open action items: you committed to sending a comparison doc.
    Their priorities: education funding, tax efficiency."

User receives prep brief on phone before arriving.
```

### Story 2: Post-Meeting Processing (from phone)
```
User (on phone, leaving a Zoom):
  → "Stop recording and summarize"

Claude (on Mac):
  → Calls stop_recording()
  → Pipeline: transcribe → diarize → summarize
  → Saves to ~/meetings/2026-03-17-team-sync.md
  → Responds on phone: "Meeting saved. 42 minutes, 3 speakers.
    Key decisions: launch date set for April 1.
    3 action items: you need to send the spec doc to Sarah by Friday."
```

### Story 3: Cross-Meeting Intelligence (in Claude Code)
```
Developer (in Claude Code, working on a feature):
  → /minutes search "API redesign"

Claude Code (meeting-analyst agent):
  → Searches all meetings
  → "Found 5 meetings mentioning the API redesign:
    - March 17: Decided on REST over GraphQL
    - March 10: Sarah raised pagination concerns
    - March 3: Team agreed on v2 prefix for new endpoints
    Consensus: REST with cursor-based pagination, /v2/ prefix."
```

### Story 4: Voice Memo Recall (from anywhere)
```
User (in Claude Code, writing a feature spec):
  → "What was that idea I had about onboarding? I recorded a voice memo
     about it a few days ago"

Claude:
  → Calls search_meetings(query: "onboarding", type: "memo")
  → "Found a voice memo from March 15 (1m 22s):
    You suggested skipping the onboarding wizard entirely.
    Instead, drop users into a pre-populated demo workspace,
    let them explore, then ask 'ready to connect your own data?'
    You also mentioned looking at how Figma does this."
```

### Story 5: iPhone → Automatic Processing
```
User (walking the dog, has an idea):
  → Opens Voice Memos on iPhone
  → Records 45 seconds about a pricing strategy
  → Taps Share → "Save to Minutes" (Apple Shortcut, installed once)

Meanwhile:
  → Audio file syncs to iCloud Drive/minutes-inbox/ on Mac
  → minutes watch detects new file in ~/.minutes/inbox/
  → Pipeline: transcribe → summarize → save
  → ~/meetings/memos/2026-03-17-pricing-strategy.md appears

Later (in Cowork):
  → "What were my recent ideas?"
  → Claude surfaces the pricing memo alongside today's meetings
```

> **Slightly more automatic:** With iPhone Shortcuts Automation ("When Voice Memos
> closes → save last recording to minutes-inbox"), iOS will prompt a notification
> to confirm — one tap instead of the share sheet flow. Not truly silent (Apple
> doesn't allow fully silent app-trigger automations), but marginally less friction.

### Story 6: Relationship Intelligence (in Cowork)
```
User (in Cowork):
  → "What does Sarah usually bring up in our meetings?"

Claude:
  → Calls get_person_context(name: "Sarah")
  → "You've had 23 meetings with Sarah over 4 months.
    Topics she cares most about: API design (8 mentions),
    performance (6), documentation (5).
    Her style: detail-oriented, often raises edge cases.
    Last interaction: March 17, she committed to the pagination spec.
    Open commitments from her: review competitor API docs (due March 21)."
```

---

## Development Workflow

### Beads Tracking

Every task gets a bead. Epic structure:

```
Epic: Minutes — Open Source Conversation Memory
├── Phase 1a: Recording Pipeline (17 tasks, week 1)
│   ├── P1a.0: MCPB native binary research (BLOCKER)
│   ├── P1a.1: Rust scaffold
│   ├── P1a.2: BlackHole audio capture
│   ├── P1a.3-4: WAV writing + whisper.cpp transcription
│   ├── P1a.5-6: Markdown output + CLI (record, stop, status)
│   ├── P1a.7-10: Config, model download, README, git init
│   ├── P1a.11-12: Voice memo watcher + memo template
│   ├── P1a.13: Apple Shortcut for iPhone
│   ├── P1a.14: Structured logging
│   └── P1a.15: Test fixtures
├── Phase 1b: Intelligence (7 tasks, week 2)
│   ├── P1b.1: Diarization (pyannote subprocess)
│   ├── P1b.2: Speaker-to-name mapping
│   ├── P1b.3-4: LLM summarization + templates
│   └── P1b.5-7: Calendar, search, e2e test
├── Phase 2: MCPB (8 tasks)
│   ├── P2.1: MCPB scaffold
│   └── ...
├── Phase 2b: Claude Code Plugin (10 tasks, parallel with Phase 2)
│   ├── P2b.1: Plugin scaffold
│   ├── P2b.2-5: Skills (/minutes record, search, list, recap)
│   ├── P2b.6: meeting-analyst agent
│   ├── P2b.7-8: Hooks (SessionStart, PostToolUse)
│   └── P2b.9-10: MCP config + README
├── Phase 3: Tauri Menu Bar (8 tasks)
├── Phase 4a: Intelligence Layer (6 tasks)
├── Phase 4b: Cowork + Dispatch (8 tasks)
│   ├── P4b.1: Cowork connector research
│   ├── P4b.2: Dispatch recording flow
│   ├── P4b.3-4: Cowork meeting brief + Dispatch summary
│   ├── P4b.5: Follow-up automation
│   ├── P4b.6: Multi-meeting synthesis
│   ├── P4b.7: Persistent memory across sessions
│   └── P4b.8: Dispatch quick commands
└── Phase 4c: Platform Expansion (3 tasks)
```

**Total: ~60 tasks across 7 sub-phases.**

### Testing Loop

```
For each feature:
1. Write implementation
2. Write test (unit + integration where applicable)
3. Manual test (record a real meeting segment)
4. Adversarial review (spawn code-reviewer agent)
5. Build check (cargo build --release)
6. Close bead
```

### Review Structure

- **Pre-implementation**: Plan review (adversarial — challenge assumptions)
- **Post-implementation**: Code review agent (quality, security, logic)
- **Pre-merge**: Silent failure hunter (error handling audit)
- **Pre-release**: Smoke test guardian (critical paths)

### Subagent Strategy

| Agent | When to Use |
|-------|-------------|
| `Explore` | Codebase navigation, finding patterns |
| `code-reviewer` | After writing each feature |
| `silent-failure-hunter` | After error handling code |
| `Plan` | Before starting each phase |
| `codex` | Second opinion on architecture decisions |
| `smoke-test-guardian` | Before each release |

### Skills to Leverage

| Skill | When |
|-------|------|
| `/ship` | Version bumps, changelog, releases |
| `/review` | Pre-merge code review |
| `/bd-issue-tracking` | Beads epic management |
| `/plan-eng-review` | Phase kickoff architecture review |
| `/plan-ceo-review` | Scope check at each phase gate |

---

## Adversarial Review (Captured)

Issues identified and mitigations:

| # | Risk | Severity | Mitigation | Status |
|---|------|----------|------------|--------|
| 1 | macOS audio capture: ScreenCaptureKit needs app bundle + entitlements | **High** | **Phase 1 CLI uses BlackHole (virtual audio device). Phase 3 Tauri app uses ScreenCaptureKit.** | **RESOLVED** |
| 2 | Two languages (Rust + Node.js for MCPB) | Low | Rust is the engine; Node.js is thin MCPB wrapper (~300 lines) | Accepted |
| 3 | Dispatch 50% reliable | Low | Dispatch is bonus, not requirement. Core works without it | Accepted |
| 4 | QMD dependency limits adoption | Low | QMD strictly optional. Core output is markdown files | Accepted |
| 5 | Meetily has 10K stars — why compete? | Medium | Different positioning: "conversation memory for AI" vs "open source Granola" | Accepted |
| 6 | BYO-LLM dilutes Claude advantage | Low | MCPB integration IS the moat. BYO-LLM is summarization only | Accepted |
| 7 | Speaker diarization quality varies | Medium | pyannote subprocess + calendar attendee mapping compensates | Planned |
| 8 | Scope creep (CLI + MCPB + menu bar + intelligence) | High | **Phase 1 split into 1a/1b. 2 weeks, not 1. MVP = capture + transcribe only.** | **RESOLVED** |
| 9 | Name availability | Low | **RESOLVED: `minutes` — crates.io + PyPI available** | **RESOLVED** |
| 10 | Maintenance sustainability | Medium | Keep core tiny: ~1000 lines Rust + ~300 lines Node.js | Active |
| 11 | **Falcon licensing blocks MIT distribution** | **High** | **RESOLVED: Falcon is NOT viable. Use pyannote via subprocess (AGPL-safe) or sherpa-onnx (Apache 2.0).** See Diarization Decision in Phase 1b. | **RESOLVED** |
| 12 | Phase 1 timeline too aggressive (was 1 week) | Medium | **RESOLVED: Split into Phase 1a (pipeline, week 1) + Phase 1b (intelligence, week 2)** | **RESOLVED** |
| 13 | X1 synergy content in public repo kills trust | Medium | **RESOLVED: Moved to `.claude/x1-strategy.md` (gitignored). Public plan is pure open-source story.** | **RESOLVED** |
| 14 | macOS TCC blocks Voice Memos iCloud path without Full Disk Access | **High** | **RESOLVED: Default to unprotected `~/.minutes/inbox/`. Ship Apple Shortcut for iPhone → iCloud Drive → inbox pipeline. FDA path documented as power-user option only.** | **RESOLVED** |
| 15 | MCPB ↔ Rust IPC undefined — how does Node.js start/stop recordings? | **High** | **RESOLVED: PID file (`~/.minutes/recording.pid`) + signals. `minutes status` for state queries. Stale PID recovery on crash.** | **RESOLVED** |
| 16 | Folder watcher reprocesses files / race with iCloud sync | **High** | **RESOLVED: Move to `processed/` after success, `failed/` on error. 2-second settle delay for size stability. Lock file prevents concurrent watchers.** | **RESOLVED** |
| 17 | `process_audio` MCP tool accepts arbitrary file paths (path traversal) | **High** | **RESOLVED: Allowlist directories + extension check. Canonicalize paths to defeat symlink traversal.** | **RESOLVED** |
| 18 | No logging — can't debug "my recording didn't work" reports | Medium | **RESOLVED: JSON lines to `~/.minutes/logs/`, 7-day rotation, `minutes logs` command.** | **RESOLVED** |
| 19 | Whisper model choice affects first-run experience | Medium | **RESOLVED: Default `small` (466MB, ~1 min download). `minutes setup --list` for alternatives.** | **RESOLVED** |
| 20 | LLM transcript exceeds context window for long meetings | Medium | **RESOLVED: Map-reduce chunking — chunk by time/speaker, summarize chunks, synthesize final summary.** | **RESOLVED** |
| 21 | Meeting markdown world-readable by default (umask 022) | Medium | **RESOLVED: Write files with `0600` permissions (owner read/write only).** | **RESOLVED** |
| 22 | MCPB may not support bundled native binaries | **High** | **P1a.0 blocker task added. Research in week 1 before Phase 2 architecture is finalized.** | **SCHEDULED** |

---

## Open Questions

- [x] ~~**Name**: `minutes` — crates.io + PyPI available, npm scoped~~ **RESOLVED**
- [x] ~~**Falcon licensing**: NOT MIT-compatible. Using pyannote subprocess or sherpa-onnx~~ **RESOLVED**
- [x] ~~**ScreenCaptureKit vs BlackHole**: Phase 1 CLI = BlackHole. Phase 3 Tauri = ScreenCaptureKit~~ **RESOLVED**
- [ ] **Tauri v2 system tray**: Verify Tauri v2 supports menu-bar-only apps (no main window) on macOS
- [ ] **whisper-rs crate maturity**: Check if whisper-rs is production-ready or if we should use whisper.cpp via C FFI directly
- [ ] **MCPB format**: Verify current MCPB packaging spec — the format may have evolved since initial research
- [ ] **pyannote subprocess protocol**: Design the IPC between Rust CLI and Python pyannote subprocess (JSON over stdout? Temp file handoff?)
- [ ] **BlackHole setup UX**: How to make the Multi-Output Device setup painless? Auto-detect? `minutes setup` command? Include a diagram?
- [ ] **Domain registration**: Register `getminutes.dev` before someone else does
- [x] ~~**IPC protocol (record/stop/status)**: PID file + signals. See IPC Protocol section in Phase 1a.~~ **RESOLVED**
- [x] ~~**Watch dedup strategy**: Move to `processed/` on success, `failed/` on error. Lock file prevents concurrent watchers.~~ **RESOLVED**
- [x] ~~**iCloud sync race condition**: Settle delay (2s size-stability check) before processing watched files.~~ **RESOLVED**
- [x] ~~**Whisper model default**: `small` (466MB). `minutes setup --list` for alternatives.~~ **RESOLVED**
- [x] ~~**Search implementation**: Built-in walk+regex default. QMD as optional engine via config.~~ **RESOLVED**
- [x] ~~**MCP path traversal**: Allowlist directories + extension check on `process_audio` tool.~~ **RESOLVED**
- [x] ~~**Logging strategy**: JSON lines to `~/.minutes/logs/`, 7-day rotation, `minutes logs` command.~~ **RESOLVED**
- [x] ~~**MCPB bundling**: P1a.0 blocker research added. Must verify before Phase 2 architecture.~~ **SCHEDULED**

---

## Claude Ecosystem Strategy (Critical Differentiator)

The Claude ecosystem is exploding. Cowork, Dispatch, MCPB, Claude Code plugins — this is becoming the primary interface for knowledge workers. Building native to this ecosystem isn't a nice-to-have, it's **the entire positioning**.

### Why Claude Ecosystem First

1. **MCPB is brand new** — there are almost no meeting/productivity extensions yet. First mover wins
2. **Cowork is becoming the OS** — knowledge workers are living in Claude Cowork all day. Meeting memory that lives inside Claude is orders of magnitude more useful than a standalone app
3. **Dispatch changes everything** — "Start recording" from your phone → your Mac captures → Claude processes → you get a summary on your phone. No other tool can do this
4. **Claude Code plugin potential** — developers using Claude Code could have `/meeting record` and `/meeting search` as skills. Meeting context enriches coding sessions ("what did the PM say about that feature?")

### Distribution Through the Claude Ecosystem

```
                    ┌──────────────────────────┐
                    │   Claude Ecosystem        │
                    │                           │
 ┌─────────────┐   │  ┌──────────┐             │
 │ MCPB        │───▶│  │ Claude   │             │
 │ Extension   │   │  │ Desktop  │  ┌────────┐ │
 └─────────────┘   │  └──────────┘  │Dispatch│ │
                    │       ↕        │(phone) │ │
 ┌─────────────┐   │  ┌──────────┐  └───┬────┘ │
 │ Claude Code │───▶│  │ Cowork   │◀─────┘      │
 │ Plugin      │   │  │ (desktop)│              │
 └─────────────┘   │  └──────────┘              │
                    │       ↕                    │
 ┌─────────────┐   │  ┌──────────┐              │
 │ Standalone  │───▶│  │ Claude   │              │
 │ CLI / App   │   │  │ API      │              │
 └─────────────┘   │  └──────────┘              │
                    └──────────────────────────┘
```

### Plugin/Skill Architecture (Claude Code)

```yaml
# Potential .claude/plugins/meeting-memory/plugin.json
{
  "name": "meeting-memory",
  "version": "1.0.0",
  "skills": [
    { "name": "meeting-record", "description": "Start/stop meeting recording" },
    { "name": "meeting-search", "description": "Search past meeting transcripts" },
    { "name": "meeting-list", "description": "List recent meetings" }
  ],
  "hooks": {
    "SessionStart": "inject meeting context if recent meetings exist",
    "PostToolUse": "auto-tag meetings with current project context"
  },
  "agents": [
    { "name": "meeting-analyst", "description": "Cross-meeting intelligence queries" }
  ]
}
```

### MCPB Tool Definitions

```typescript
// MCP tools exposed by the extension
const tools = {
  start_recording: {
    description: "Start recording the current meeting",
    inputSchema: {
      meetingTitle: "optional string",
      attendees: "optional string[]"
    }
  },
  stop_recording: {
    description: "Stop recording and process the meeting",
    inputSchema: {
      generateSummary: "boolean (default: true)",
      extractActionItems: "boolean (default: true)"
    }
  },
  list_meetings: {
    description: "List recent meetings with summaries",
    inputSchema: {
      limit: "number (default: 10)",
      since: "optional ISO date string",
      attendee: "optional string filter"
    }
  },
  search_meetings: {
    description: "Search meeting transcripts and summaries",
    inputSchema: {
      query: "string",
      dateRange: "optional { from, to }",
      attendee: "optional string"
    }
  },
  get_meeting: {
    description: "Get full transcript and details of a specific meeting",
    inputSchema: {
      meetingId: "string (filename or date-slug)"
    }
  },
  get_person_context: {
    description: "Get aggregated context about a person from all meetings",
    inputSchema: {
      name: "string",
      limit: "number (default: 5)"
    }
  },
  process_audio: {
    description: "Process an audio file (voice memo, recording) through the pipeline",
    inputSchema: {
      filePath: "string (path to .m4a, .wav, .mp3)",
      type: "'memo' | 'meeting' (default: 'memo')",
      title: "optional string",
      diarize: "boolean (default: false for memos, true for meetings)",
      summarize: "boolean (default: true)"
    }
  }
};
```

---

## Growth & Distribution Strategy

### Phase 1: Developer traction (GitHub stars)
- Launch on GitHub with polished README, demo GIF, clear install
- Post to Hacker News, r/selfhosted, r/productivity
- "Open-source Granola alternative with speaker diarization + voice memo processing" — clear positioning
- Voice memo angle appeals to a wider audience than meeting-only tools (r/PKM, r/ObsidianMD, r/ADHD)

### Phase 2: Claude ecosystem native
- List on Claude Desktop extension directory (when available)
- Claude Code plugin in plugin registry
- Blog post: "Building a meeting memory layer for Claude"

### Phase 3: Broader audience
- Homebrew cask: `brew install --cask minutes`
- Product Hunt launch
- YouTube demo: "Replace Granola with this free, private alternative"

### Phase 4: Ecosystem play
- QMD integration showcased in QMD docs
- Obsidian community plugin (wrapper around the CLI)
- Integration guides for other AI assistants (Cursor, Windsurf, etc.)

---

## References

- [OpenGranola](https://github.com/yazinsai/OpenGranola) — Swift, real-time suggestions from knowledge base
- [Meetily](https://meetily.ai/) — Tauri + Python, 10K stars, no diarization
- [whisper.cpp](https://github.com/ggerganov/whisper.cpp) — C/C++, Apple Silicon optimized
- [Falcon](https://picovoice.ai/platform/falcon/) — On-device speaker diarization
- [pyannote](https://www.pyannote.ai/) — Python, speaker diarization (community-1 model)
- [WhisperX](https://github.com/m-bain/whisperX) — Whisper + diarization + word timestamps
- [Tauri v2](https://v2.tauri.app/) — Rust + web frontend desktop apps
- [Claude Desktop MCPB](https://support.claude.com/en/articles/12922929) — Extension packaging format
- [Claude Cowork Dispatch](https://support.claude.com/en/articles/13947068) — Remote agent control from phone
- [MacStories Dispatch Review](https://www.macstories.net/stories/hands-on-with-claude-dispatch-for-cowork/) — Real-world testing (~50% success rate in preview)
