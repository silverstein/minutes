# Minutes — Project Plan

> **Name**: `minutes`
> **Tagline**: Every meeting, every idea, every voice note — searchable by your AI
> **Domain**: useminutes.app (primary) + useminutes.app (desktop landing page)
> **Registries**: crates.io (available), PyPI (available), npm (@minutes/cli or scoped)
> **Created**: 2026-03-17
> **Author**: Mat Silverstein
> **License**: MIT

---

## Vision

An open-source, privacy-first tool that turns any audio — meetings, voice memos, brain dumps — into searchable, AI-queryable memory. Not a meeting notes app — a **conversation memory layer** that integrates natively with the Claude ecosystem (MCPB, Cowork, Dispatch) while supporting any LLM.

Agents have run logs. Humans have conversations. Minutes captures the human side — the decisions, the intent, the context that agents need but can't observe — and makes it queryable. In a world where agents do the work but humans still set the direction, this is the missing input.

### Core Insight

Build the intelligence **inside the AI**, not next to it. Granola and Meetily are standalone apps that produce notes from meetings. This produces memory from *any conversation* — including the ones you have with yourself — that your AI assistant can recall mid-conversation.

The pipeline is the product, not the meeting. The same transcribe → summarize → store → search pipeline works on a 45-minute team standup and a 30-second voice memo recorded while walking the dog. Meetings are episodic (3-5/week); voice memos are constant. This turns Minutes from "a tool I use during meetings" into "a tool I think with every day."

> "Claude, what did Alex say about the timeline in last Tuesday's call?" just works.
> "Claude, what was that idea I had about onboarding while driving yesterday?" also works.

### Why Now

- Existing meeting tools are cloud-first and increasingly paywalled
- Open-source alternatives lack diarization, knowledge graph integration, and AI agent support
- MCP is brand new — first meeting memory tool as an AI extension wins mindshare
- Claude Desktop + Cowork + Dispatch enables mobile-triggered recording (phone → Mac pipeline)
- Knowledge graph integration (QMD/Obsidian/PARA) is a differentiator no one else has
- Tauri v2 + Rust are mature for cross-platform native apps

---

## Competitive Landscape

| | Granola | Meetily | Otter.ai | **This Project** |
|--|---------|---------|----------|-------------------|
| Local transcription | No (cloud) | Yes | No | **Yes** |
| Speaker diarization | Yes | **No** | Yes | **Yes** (pyannote-rs native, no Python) |
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
| Structured output for agents | No | No | No | **Yes** (decisions/intents as queryable YAML, MCP tools) |
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
│  │           │     │/ parakeet │   │ sherpa-   │   │ Ollama /     │ │
│  │ Voice     │     │(local,    │   │ onnx      │   │ OpenAI /     │ │
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
│  │ │ title: Q2 Planning Discussion                                │  │ │
│  │ │ date: 2026-03-17T14:00:00                                  │  │ │
│  │ │ duration: 42m                                               │  │ │
│  │ │ attendees: [Alex K., Jordan M.]                            │  │ │
│  │ │ calendar_event: Team Weekly Sync                              │  │ │
│  │ │ tags: [planning, roadmap]                               │  │ │
│  │ │ people: [[alex-k], [[jordan-m]]]                            │  │ │
│  │ │ ---                                                         │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Summary                                                  │  │ │
│  │ │ - Agreed on Q2 launch timeline for the new API              │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Decisions                                                │  │ │
│  │ │ - [x] Advisor pricing must pass Molly fairness test       │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Action Items                                             │  │ │
│  │ │ - [ ] @user: Send tech spec to Alex by Friday               │  │ │
│  │ │ - [ ] @case: Review competitor pricing grid                │  │ │
│  │ │                                                             │  │ │
│  │ │ ## Transcript                                               │  │ │
│  │ │ [CASE 0:00] So I think the pricing for advisors should...  │  │ │
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
| **Transcription** | whisper.cpp (default) or parakeet.cpp (opt-in) | Local, Apple Silicon optimized. Parakeet: lower WER, multilingual. |
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
- **Audio format conversion** — prefers `ffmpeg` when available (matches whisper-cli's pipeline, critical for non-English audio). Falls back to `symphonia` (pure Rust, in-process) when ffmpeg isn't installed. Silero VAD + post-transcription dedup provide additional protection against hallucination loops.

---

## Experience Strategy — Make Minutes Feel 10/10 Without Becoming A Clone

This section captures the product-quality patterns worth learning from polished desktop capture tools while staying true to what makes **Minutes** distinct:

- **Open source and inspectable**
- **Local-first and privacy-first**
- **CLI-friendly, scriptable, and automation-safe**
- **Agent-native across Claude Desktop, Claude Code, Cowork, Codex, and OpenAI-compatible tool surfaces**
- **Built for conversation memory, not just dictation or transient transcription**

### Respectful Inspiration Rule

We should borrow **interaction principles**, not assets, copy, or distinctive branded flourishes.

What we can borrow:
- Clear permission journeys
- Immediate confidence cues when recording starts/stops
- Thoughtful hotkey semantics
- Strong first-run onboarding
- Reliable menu bar behavior
- Sensible update/distribution polish

What we should explicitly not borrow:
- Their wording, animation assets, sound files, iconography, or UI composition
- Surveillance-heavy defaults that do not match our trust model
- Product framing that positions Minutes as a dictation overlay instead of a memory layer

### What Must Stay Uniquely Minutes

Minutes is not primarily "voice-to-text anywhere." Minutes is:

- A **conversation memory layer**
- A **local markdown memory store** that remains useful outside any one app
- A tool that works equally well from:
  - Tauri menu bar app
  - CLI
  - MCP tools
  - Claude Code plugin/skills
  - Cowork/Dispatch workflows
  - other agent/tool ecosystems that can call a CLI or MCP server

The user should feel that the desktop app is the most polished surface, but **not the only first-class surface**.

### Product Principles

#### 1. Capture Confidence

The user must always know:
- whether recording has started
- whether recording is still in progress
- whether the capture was discarded, saved, or failed
- where the output went
- whether anything needs their attention

Confidence cues must exist in every surface:

| Surface | Start cue | Stop cue | Failure cue | Status surface |
|--------|-----------|----------|-------------|----------------|
| Menu bar app | tray/icon state + sound + inline timer | sound + state transition + refresh list | inline error + preserved capture path | tray + window |
| CLI | immediate stderr text | completion JSON + saved path | explicit error + preserved WAV path | `minutes status` |
| MCP/agent | clear textual confirmation | structured success result | actionable failure result | `get_status` |
| Watcher/inbox | log entry + optional notification | saved markdown + source moved to processed | moved to failed + log | `minutes logs`, folder state |

Design requirement:
- Never silently lose a recording.
- Never make a user wonder whether the microphone is actually live.
- Never rely on one UI surface being open for state truth.

#### 2. Permission Honesty

Minutes should ask for only the permissions that directly support the selected workflow.

Permission categories:
- **Microphone**: required for live recording
- **Calendar**: optional, useful for meeting suggestions and attendee enrichment
- **Screen recording**: optional, future-facing, only if we truly add screen-context features
- **Accessibility**: optional, only if we add global hotkeys or inline paste automation that genuinely requires it
- **Full Disk Access**: avoid as a default strategy

User-facing rule:
- Every permission prompt must answer:
  - what feature needs this
  - whether it is optional
  - what still works if the user says no

Open-source trust rule:
- Prefer capabilities that can be explained in one sentence and audited in code.
- Avoid broad permissions unless the feature value is obvious and compelling.

#### 3. One State Machine, Many Interfaces

All capture entry points must hit the same underlying recording lifecycle:

- tray start
- CLI `minutes record`
- MCP `start_recording`
- future hotkey start
- future calendar prompt "Record"
- future shortcut or automation trigger

This is a core Minutes advantage: the desktop app is not a separate product. It is a polished view over the same engine.

Design rule:
- Feature parity matters more than UI parity.
- If a behavior exists in one surface, its state model and outputs must remain coherent in the others.

#### 4. Markdown Is The Source Of Truth

Minutes should continue to treat markdown output as the durable artifact, not a database-only internal representation.

That means the "10/10" desktop experience must still preserve:
- readable markdown
- YAML frontmatter that agents can query
- paths users can browse in Finder
- easy grep/qmd/Obsidian compatibility

The more polished the desktop app becomes, the more important it is to avoid locking critical meaning inside the app itself.

#### 5. Agent-Native By Design

Every user-facing workflow should be translatable to an agent/tool workflow.

Examples:
- "Start recording" → CLI + MCP + tray
- "What did we decide?" → markdown frontmatter + MCP search + agent summary
- "Any open action items for me?" → queryable structured output
- "Record my thought and surface it in tomorrow's planning session" → watcher + MCP + Claude/Codex workflow

Minutes should remain:
- **Claude-native** without being Claude-exclusive
- **Codex-friendly** because the CLI and markdown store are stable
- **OpenAI-compatible** because the integration boundary is tool calling/MCP/CLI, not product-specific APIs

### UX Opportunities Worth Adopting In A Minutes-Native Way

#### A. Original Earcon System

We should add a tiny, original Minutes sound system:
- recording started
- recording stopped and kept
- error / capture failed
- optional "processing complete"

Guidelines:
- subtle, short, utility-grade
- shippable under MIT-compatible terms
- fully optional in settings
- consistent with CLI/MCP semantics

Why this fits Minutes:
- It helps menu bar and future hotkey flows without requiring constant visual attention.
- It improves trust without changing the core data model.

#### B. Intentional First-Run Onboarding

Minutes should have an onboarding flow that feels distinctly like a local-first tool, not a SaaS funnel.

Recommended sequence:
1. What do you want to do first?
   - Record meetings live
   - Process voice memos
   - Both
2. Download transcription model
3. Grant microphone access if needed
4. Choose output folder / confirm default
5. Offer optional enhancements:
   - watch inbox folder
   - connect calendar
   - enable sounds
   - enable hotkey later
6. Run a 10-second test recording

Minutes-native requirement:
- The flow should end with a successful artifact, not just "setup complete."
- After onboarding, the user should have exactly one real saved note or meeting file.

#### C. Hotkeys, But Only If They Fit The Trust Model

Hotkeys are attractive, but they should be treated as a deliberate Phase 3/4 feature, not casual polish.

Rules:
- No default accessibility permission ask just because hotkeys are cool.
- Global hotkeys should be opt-in.
- We should document the behavior before implementing it.

Hotkey modes worth exploring:
- hold-to-record quick thought
- tap-to-lock hands-free recording
- double-tap promotion to locked recording

But the feature should be Minutes-native:
- optimized for capturing ideas and meetings into durable memory
- not optimized for rapid-fire inline dictation into arbitrary apps

If we do add hotkeys, we should write a product behavior spec similar to a protocol doc:
- minimum duration
- discard rules
- sounds or no sounds
- lock-state transitions
- what happens if a second interface stops the recording

#### D. Inline Capture Without Surveillance Creep

Future opportunity:
- "Paste last transcript"
- "Insert latest action items"
- "Attach meeting note to current app context"

But we should be careful:
- Accessibility and screen recording are trust-heavy permissions.
- These should be optional power-user flows, not part of the default narrative.

Recommended product stance:
- Phase 1-3: conversation memory first
- Later: context-aware insertion only after the core memory workflow is excellent

#### E. Distribution Polish For An Open-Source Mac App

The open-source version should still feel premium in its operational polish.

Recommended distribution plan:
- signed and notarized releases
- universal app bundle
- stable menu bar behavior
- sensible crash recovery
- optional auto-update after trust is established

Auto-update principle:
- We can adopt Sparkle-style polish later, but only with:
  - transparent release notes
  - deterministic artifacts
  - clear opt-in or clearly disclosed auto-update behavior
  - a documented manual update path for users who prefer reproducibility

### Explicit Non-Goals

To avoid becoming a confused imitation of other tools, Minutes should not prioritize:

- AI-screen-surveillance as a default feature
- "dictation into every app" before memory quality is solved
- flashy onboarding over reliable capture
- proprietary lock-in over markdown portability
- desktop-only features that break parity with CLI/MCP users

#### Anti-Goals In Practice

These examples are here so the plan keeps its shape under implementation pressure:

- If we are choosing between:
  - a beautiful onboarding animation
  - and a guaranteed test recording that writes a markdown artifact
  - we choose the artifact every time.

- If we are choosing between:
  - a clever desktop-only shortcut
  - and making the same workflow reliable from CLI + MCP
  - we choose parity first.

- If we are choosing between:
  - a feature that requires Accessibility or Screen Recording
  - and a simpler feature that improves capture confidence without new permissions
  - we choose the lower-permission path first.

- If we are choosing between:
  - hiding system details behind a "magic" UI
  - and clearly showing where files live, what failed, and how to retry
  - we choose clarity.

### Concrete Plan Additions

These are the product-quality tasks we should schedule after the current core roadmap items.

### Recommended Delivery Order

The experience work should ship in this order:

1. **Trust + clarity first**
   - permission center
   - output destination clarity
   - recovery center
   - completion notifications
2. **Confidence polish second**
   - capture cue system
   - parity audit across surfaces
   - onboarding that guarantees a first saved artifact
3. **Speed workflows third**
   - hotkey spec
   - opt-in hotkeys
   - quick thought mode
   - optional paste/insert workflows
4. **Distribution polish fourth**
   - signed releases
   - release-channel policy
   - reproducible notes
   - auto-update evaluation last

Reasoning:
- This order maximizes trust before delight.
- It preserves the open-source/local-first posture.
- It prevents us from polishing a shaky capture experience.

### Dependencies and Gates

These are the main execution dependencies for the new experience roadmap:

| Area | Depends on | Why |
|------|------------|-----|
| Permission center | Stable permission detection APIs in Tauri/core | Avoid UI that lies about system state |
| Capture cue system | Reliable recording lifecycle and failure states | Sounds are only useful if they match reality |
| First successful artifact onboarding | Model download, microphone flow, output writing | Onboarding should end in a real artifact, not a fake success state |
| Surface parity audit | Mature CLI + MCP + tray flows | Audit is meaningless before the surfaces stabilize |
| Recovery center | Failed capture preservation + retry paths | UI should expose actual recovery affordances, not placeholders |
| Hotkey implementation | Hotkey behavior spec + permission decision | Prevent ad hoc global shortcut behavior |
| Paste latest artifact | Stable artifact metadata + clear trust model | Avoid creeping into surveillance/automation prematurely |
| Auto-update evaluation | Signed/notarized releases + release channel policy | Updating before release discipline is established is backwards |

#### Phase 3a: Capture Experience Excellence — "Calm, trusted, agent-native"

**Goal**: make Minutes feel premium at the moment of capture without sacrificing portability or trust.

| Task | Description | Beads ID |
|------|-------------|----------|
| P3a.1 | **Capture cue system** — original Minutes earcons for start, stop, complete, and error. Settings toggle + volume preset. Sounds must be short, subtle, and MIT-compatible. | TBD |
| P3a.2 | **Permission center** — in-app status for microphone, calendar, watcher folder readiness, and future optional permissions. Every permission row explains value, optionality, and "what works without this". | TBD |
| P3a.3 | **First successful artifact onboarding** — onboarding ends only after a real test recording or processed memo is saved to markdown. | TBD |
| P3a.4 | **Surface parity audit** — ensure tray, CLI, MCP, and plugin flows all share the same success/failure semantics and user-facing messages. | TBD |
| P3a.5 | **Recovery center** — expose failed captures, stale recordings, and retry actions in the desktop UI without hiding the underlying files. | TBD |
| P3a.6 | **Completion notifications** — optional macOS notification when a long recording has finished processing and been written to disk. | TBD |
| P3a.7 | **Output destination clarity** — every successful flow points clearly to the saved markdown path and, when relevant, the preserved source capture path. | TBD |

**Recommended execution order within Phase 3a**

1. P3a.2 Permission center
2. P3a.7 Output destination clarity
3. P3a.5 Recovery center
4. P3a.6 Completion notifications
5. P3a.1 Capture cue system
6. P3a.4 Surface parity audit
7. P3a.3 First successful artifact onboarding

**Phase 3a exit criteria**

- New user reaches first saved artifact in under 3 minutes on a clean machine with a network connection.
- Every successful recording flow exposes the saved markdown path.
- Every failed live capture preserves either:
  - the raw capture
  - or an explicit reason no recoverable capture exists.
- Tray, CLI, and MCP start/stop/status semantics match for the same recording session.
- Permission center correctly distinguishes:
  - required now
  - optional
  - unavailable but non-blocking

**Phase 3a success metrics**

- Time to first saved artifact: median under 3 minutes
- Silent-loss incidents: zero tolerated
- Recovery visibility: 100% of failed captures produce a user-visible recovery path
- Surface parity: all core flows pass the compatibility checklist below
- Onboarding completion: target 80%+ for users who begin model download

**Issue stubs to create before implementation**

- P3a.2 permission center
- P3a.7 output destination clarity
- P3a.5 recovery center
- P3a.1 capture cue system
- P3a.4 surface parity audit
- P3a.3 artifact-first onboarding

#### Phase 3a+: Taste & Polish — "The details that make it feel real"

**Goal**: bridge the gap between "it works" and "I want to use this every day." These are the small things that compound into product love.

| Task | Description | Beads ID |
|------|-------------|----------|
| P3a+.1 | **Processing progress indicator** — after stopping a recording, show real-time pipeline status in the UI (transcribing... summarizing... saving). The dead silence between "stop" and "saved" is the #1 taste gap. Whisper progress via callback, spinner with stage label. | TBD |
| P3a+.2 | **Smart title generation** — after transcription, auto-generate a meaningful title from the first few sentences or pre-meeting context. "Q2 Pricing Discussion" > "Recording 2026-03-18 13:39". Falls back to timestamp if no speech detected. | TBD |
| P3a+.3 | **In-app meeting detail view** — clicking a meeting opens a proper view inside the app: transcript with timestamps, highlighted action items, speaker labels, notes section, "Open in editor" button. This is where 80% of the perceived quality lives. | TBD |
| P3a+.4 | **App icon and visual identity** — real app icon (not auto-generated microphone), considered typography choice, about screen. Doesn't need to be elaborate but needs to feel intentional. | TBD |
| P3a+.5 | **Empty state = onboarding** — merge the onboarding flow into the empty state of the meeting list. No separate overlay — the empty state IS the setup experience, becoming the meeting list once the first recording is saved. | TBD |
| P3a+.6 | **Transcript search highlighting** — when searching meetings, show highlighted match context in the meeting list items and in the detail view. Currently search returns results but doesn't show where the match was. | TBD |

**Why this matters more than it looks:**
- P3a+.1 (progress) directly addresses the "is it broken?" anxiety after every recording
- P3a+.2 (titles) is the difference between a useful meeting list and a wall of timestamps
- P3a+.3 (detail view) is the most-used screen after the first recording — it's currently `open` to a text editor
- P3a+.4 (identity) is what people screenshot when they share the tool
- P3a+.5 (empty=onboarding) eliminates the jarring overlay-to-empty-list transition

#### Phase 3b: Optional Power Capture — "Fast capture for people who want it"

**Goal**: add high-speed capture modes without making them the product's default identity.

| Task | Description | Beads ID |
|------|-------------|----------|
| P3b.1 | **Hotkey behavior spec** — write a protocol document for hotkey recording semantics before implementing any global shortcut support. | TBD |
| P3b.2 | **Opt-in global hotkey** — single configurable shortcut for quick capture, gated behind explicit permission flow only if needed. | TBD |
| P3b.3 | **Quick thought mode** — short-form memo capture path optimized for spontaneous idea capture, still ending in normal markdown output. | TBD |
| P3b.4 | **Paste latest artifact** — optional power-user command to paste the latest transcript/summary into the current app, clearly separated from core Minutes workflows. | TBD |

**Recommended execution order within Phase 3b**

1. P3b.1 Hotkey behavior spec
2. P3b.3 Quick thought mode
3. P3b.2 Opt-in global hotkey
4. P3b.4 Paste latest artifact

**Phase 3b gates**

- Do not start P3b.2 before P3b.1 is written and reviewed.
- Do not start P3b.4 before we are comfortable with the permission story and trust implications.
- Quick thought mode must still produce the same durable markdown artifact model as other capture paths.

**Phase 3b exit criteria**

- Hotkey behavior is documented before shipping.
- Global hotkey remains opt-in and clearly permission-scoped.
- Quick thought mode creates standard memo artifacts that remain searchable through CLI/MCP.
- No power-capture feature becomes required for core Minutes value.

**Phase 3b success metrics**

- Hotkey confusion reports stay low because behavior is documented and consistent.
- Quick-thought artifact creation succeeds at the same rate as standard memo capture.
- Accessibility-dependent workflows remain a minority, not a prerequisite.

#### Phase 3c: Open-Source Distribution Polish — "Trustworthy to install, boring to maintain"

**Goal**: make the app operationally polished enough for non-technical users without compromising auditability.

| Task | Description | Beads ID |
|------|-------------|----------|
| P3c.1 | Signed + notarized release pipeline for Minutes.app | TBD |
| P3c.2 | Release channel policy — stable vs preview, changelog quality bar, rollback plan | TBD |
| P3c.3 | Evaluate auto-update strategy (Sparkle or equivalent) with explicit OSS trust requirements and manual-update fallback | TBD |
| P3c.4 | Reproducible release notes: what changed for CLI, desktop, and MCP users in each release | TBD |

**Recommended execution order within Phase 3c**

1. P3c.1 Signed + notarized release pipeline
2. P3c.2 Release channel policy
3. P3c.4 Reproducible release notes
4. P3c.3 Auto-update evaluation

**Phase 3c gates**

- No auto-update rollout before signed/notarized releases are routine.
- No opaque update mechanism that hides the manual install path.
- Release notes must cover CLI, desktop, and MCP implications separately.

**Phase 3c exit criteria**

- A non-technical user can install a trusted release without bypassing scary macOS warnings.
- Every release explains what changed across surfaces.
- Auto-update, if adopted, is transparent, reversible, and optional or clearly disclosed.

**Phase 3c success metrics**

- Install friction drops measurably after signing/notarization.
- Release notes become consistent enough to support user trust and debugging.
- If auto-update ships, users can still choose a manual deterministic update path.

### Agent-Native Compatibility Checklist

Every experience feature should be evaluated against this checklist before shipping:

- Does it preserve CLI parity?
- Does it preserve MCP parity?
- Does it keep the markdown artifact as the durable truth?
- Can a Claude/Codex/OpenAI-style agent explain it to the user with a stable tool contract?
- Does it avoid forcing desktop-only workflows for core value?

If the answer is "no" to multiple items, it is likely a sidecar feature, not a core Minutes feature.

### The Bar For "10/10"

Minutes is 10/10 when:

- a first-time user can go from install → permission → first saved artifact in minutes
- a power user can rely on it from terminal, tray, or agent without mental model drift
- a recording never disappears silently
- the app feels calm and trustworthy at the moment of capture
- the markdown output remains useful even if the app disappeared tomorrow
- Claude/Cowork/Codex-style workflows feel like a natural extension of the product, not a bolted-on integration layer

### Working Definition Of Done For Experience Work

A capture-experience feature is not done when the UI exists. It is done when:

- the tray flow works
- the CLI/MCP semantics still make sense
- the saved artifact is clear
- permission behavior is honest
- failure/recovery behavior is explicit
- the docs and release notes tell users what changed
- the feature still feels like Minutes, not a separate mini-product

---

## Auto-Detect Calls & Privacy Hardening

### Call Detection — "Just tap to start"

**Goal**: When you join a Zoom, Teams, Meet, or FaceTime call, Minutes shows a macOS notification asking if you want to transcribe. Tap it to start recording. No setup, no remembering to hit record.

#### Detection Strategy

```
┌─────────────────────────────────────────────────────────┐
│                    DETECTION LOOP                         │
│                   (every 3 seconds)                       │
│                                                           │
│  1. Check running processes for call apps                 │
│     ┌──────────────────────────────────────┐              │
│     │ zoom.us, Microsoft Teams, FaceTime,  │              │
│     │ Google Chrome (meet.google.com tab),  │              │
│     │ Webex, Slack (huddle)                │              │
│     └──────────────────────────────────────┘              │
│                    │                                      │
│  2. Check if app is using microphone                      │
│     (via IOKit / CoreAudio device listener)               │
│                    │                                      │
│  3. Both true = CALL DETECTED                             │
│     ┌──────────────────────────────────────┐              │
│     │ Show notification:                   │              │
│     │ "Zoom call detected.                 │              │
│     │  Tap to start recording."            │              │
│     └──────────────────────────────────────┘              │
│                    │                                      │
│  4. User taps → cmd_start_recording(Meeting)              │
│     User ignores → cooldown 5 min for this app            │
│                                                           │
│  5. When call app releases mic → offer to stop            │
└─────────────────────────────────────────────────────────┘
```

#### Why Two Signals (Process + Mic)

- Process alone: too many false positives (Zoom open but no call)
- Mic alone: too many false positives (dictation, voice memos, music apps)
- Both together: very high confidence — an app known to do calls is actively using the mic

#### Implementation — Rust Side

**New file**: `tauri/src-tauri/src/call_detect.rs`

```
pub struct CallDetector {
    enabled: AtomicBool,
    last_notified_app: Mutex<Option<(String, Instant)>>,  // cooldown tracking
    poll_interval: Duration,  // default 3s
}

// Known call apps and their process names
const CALL_APPS: &[(&str, &str)] = &[
    ("zoom.us", "Zoom"),
    ("Microsoft Teams", "Teams"),
    ("FaceTime", "FaceTime"),
    ("Google Chrome", "Google Meet"),  // needs URL check via AppleScript
    ("Webex", "Webex"),
    ("Slack", "Slack Huddle"),
];

impl CallDetector {
    // Poll loop — runs in background thread, started from main.rs setup
    pub fn start(self: Arc<Self>, app: AppHandle, recording: Arc<AtomicBool>) {
        std::thread::spawn(move || {
            loop {
                if self.enabled.load(Ordering::Relaxed) && !recording.load(Ordering::Relaxed) {
                    if let Some(app_name) = self.detect_active_call() {
                        if !self.in_cooldown(&app_name) {
                            self.notify_call_detected(&app, &app_name);
                        }
                    }
                }
                std::thread::sleep(self.poll_interval);
            }
        });
    }

    fn detect_active_call(&self) -> Option<String> {
        // 1. Get running process names via sysctl/NSRunningApplication
        // 2. Check each against CALL_APPS
        // 3. For matching processes, check mic usage via CoreAudio
        // 4. Return first match that has mic active
    }

    fn check_mic_in_use() -> bool {
        // Use CoreAudio HAL: AudioObjectGetPropertyData
        // with kAudioDevicePropertyDeviceIsRunningSomewhere
        // on the default input device
    }

    fn notify_call_detected(&self, app: &AppHandle, app_name: &str) {
        // macOS notification with action button
        show_user_notification(
            &format!("{} call detected", app_name),
            "Tap to start recording with Minutes"
        );
        // Also emit event to frontend for in-app prompt
        app.emit("call:detected", app_name).ok();
        self.set_cooldown(app_name);
    }
}
```

#### Implementation — Frontend Side

In `index.html`, listen for `call:detected` event:
- Show a banner at the top of the main window: "[Zoom icon] Zoom call detected — **Start Recording**"
- Banner has "Start Recording" button and "Dismiss" (sets cooldown)
- If user clicks Start, calls `cmd_start_recording` with mode `Meeting`
- Banner auto-dismisses after 30s if not acted on

#### Config

```toml
[call_detection]
enabled = true                    # master toggle
poll_interval_secs = 3            # how often to check
cooldown_minutes = 5              # don't re-prompt for same app
apps = ["zoom.us", "Microsoft Teams", "FaceTime", "Google Chrome", "Webex", "Slack"]
```

#### End-of-Call Detection

When the call app releases the mic (CoreAudio property listener fires), and Minutes is recording:
1. Wait 10s grace period (in case of brief mic drops)
2. If mic still released, show notification: "Call ended. Stop recording?"
3. If user doesn't respond in 60s, auto-stop and process

#### What Makes This Better Than Competitors

- **No always-on mic listening** — we poll process list + mic state, never capture audio until user opts in
- **Per-call consent** — you choose each time, not a blanket "record everything" setting
- **Works with any call app** — config-driven list, not hardcoded to 3 vendors
- **CLI parity** — `minutes watch --calls` flag for headless/CLI users who want auto-detect without the Tauri app

#### Build Order

1. `call_detect.rs` — process detection + mic check (pure Rust, no UI)
2. Background thread in `main.rs` setup
3. macOS notification on detection
4. Frontend banner in `index.html`
5. Config integration
6. End-of-call detection
7. Tests — mock process list, verify cooldown logic

### Privacy Hardening — Spotlight Indexing Block

**Problem**: macOS Spotlight indexes `~/meetings/` by default, making transcripts searchable system-wide and potentially visible to other apps.

**Fix**: Drop a `.metadata_never_index` file in output directories on first run.

```rust
// In minutes-core config init or first recording
fn block_spotlight_indexing(dir: &Path) {
    let marker = dir.join(".metadata_never_index");
    if !marker.exists() {
        std::fs::write(&marker, "").ok();
    }
}
```

Directories to protect:
- `~/meetings/`
- `~/meetings/memos/`
- `~/.minutes/` (logs, assistant workspace, screens)

This is a one-liner to implement — should ship immediately.

---

## PARA Integration (for QMD/Obsidian users)

**Status**: Vault sync shipped (PR #4, v0.3.0). Three strategies: symlink, copy, direct. CLI: `minutes vault setup/status/unlink/sync`. Tauri NSOpenPanel integration tracked in issue #5.

Meetings and memos become first-class PARA entities:

```
~/Documents/life/           (or configurable path)
├── areas/
│   └── meetings/                              # QMD collection
│       ├── 2026-03-17-weekly-standup.md
│       ├── 2026-03-17-advisor-demo-call.md
│       ├── memos/                             # Voice memos subfolder
│       │   ├── 2026-03-17-onboarding-idea.md
│       │   ├── 2026-03-17-pricing-thought.md
│       │   └── ...
│       └── ...
├── areas/people/
│   └── jordan-m/
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
| **P1a.0** | **BLOCKER: MCPB native binary research.** Can an MCPB package bundle a Rust binary? Is there a postinstall hook? Test with a hello-world `.mcpb` that shells out to a native binary. If MCPB can't bundle binaries, Phase 2 architecture must change. **Spend 2 hours on this in week 1, not week 2.** | **DONE** |
| P1a.1 | Rust project scaffold (cargo workspace: `core`, `cli`) | **DONE** |
| P1a.2 | Audio capture via BlackHole virtual audio device + `cpal` crate (NOT ScreenCaptureKit — see note below) | **DONE** |
| P1a.3 | WAV file writing (capture → save to temp .wav, clean up temp WAV after transcription) | **DONE** |
| P1a.4 | whisper.cpp integration via `whisper-rs` crate (batch transcription of .wav → text). **Audio format conversion**: use `symphonia` crate to decode .m4a/.mp3/.ogg → WAV before transcription (whisper-rs only reads WAV natively). Handle empty transcripts: save markdown with `[No speech detected]` marker + `status: no-speech` in frontmatter. Minimum word threshold: 10 (configurable). | **DONE** |
| P1a.5 | Markdown output with YAML frontmatter (title, date, duration, raw transcript). **File permissions: `0600`** (owner read/write only — transcripts contain sensitive content). | **DONE** |
| P1a.6 | CLI interface: `minutes record` (start), `minutes stop` (stop + transcribe + save), **`minutes status`** (is recording in progress? duration so far?). See IPC Protocol below. | **DONE** |
| P1a.7 | Config file (`~/.config/minutes/config.toml` — output dir, whisper model path, search engine, watch settings) | **DONE** |
| P1a.8 | Model download helper: `minutes setup` — downloads whisper `small` model by default (466MB, best quality/size tradeoff). `minutes setup --model large-v3` for best quality (3.1GB). `minutes setup --list` shows all available models with sizes. | **DONE** |
| P1a.9 | README, LICENSE (MIT), .gitignore, basic docs, CONTRIBUTING.md | **DONE** |
| P1a.10 | Git init, GitHub repo creation | **DONE** |
| P1a.11 | Folder watcher mode: `minutes watch <dir>` — watches a folder for new audio files (.m4a, .wav, .mp3), runs each through the transcription pipeline automatically. See Watch Protocol below for dedup, settle delay, and locking. | **DONE** |
| P1a.12 | Memo-specific frontmatter template (`type: memo`, no attendees/calendar, `source:` field for origin tracking) | **DONE** |
| P1a.13 | Apple Shortcut: "Save to Minutes" — downloadable `.shortcut` file that adds a share sheet action on iPhone to save audio to `iCloud Drive/minutes-inbox/`, which syncs to Mac and gets picked up by `minutes watch` | TBD |
| P1a.14 | Structured logging: JSON lines to `~/.minutes/logs/minutes.log`, daily rotation (7 days). Every pipeline step logs file, step, duration, outcome. `minutes logs` command to tail. `minutes logs --errors` to filter. `--verbose` CLI flag for stderr debug output. | **DONE** |
| P1a.15 | Test fixtures: 5-second WAV in `tests/fixtures/` (~800KB), mock data for transcript/diarization/summary parsing. Integration test runs full pipeline on fixture. | **DONE** |
| P1a.16 | Edge case test pass: every error variant in `error.rs` has at least one test. Covers: partial config merge, filename collisions, settle delay, lock files, special chars in search, no-speech template, 0600 permissions, auto-create output dir, move-to-failed, wrong extension skip. | **DONE** |

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
| P1b.1 | Speaker diarization integration (see Diarization Decision below) | **DONE** (pyannote-rs native + pyannote subprocess legacy) |
| P1b.2 | Speaker-to-name mapping (calendar attendees → speaker labels) | **DONE** (calendar + LLM extraction) |
| P1b.3 | LLM summarization module — pluggable: Claude API, Ollama, OpenAI. **Map-reduce chunking** for transcripts exceeding context window: chunk by time/speaker turn, summarize each chunk, produce final summary from chunk summaries. If no LLM configured → skip gracefully, save transcript-only markdown. | **DONE** (agent/ollama/ureq) |
| P1b.4 | Summary template system (configurable output: decisions, action items, key points) | **DONE** (structured extraction) |
| P1b.5 | Calendar integration (ical file parsing for meeting context + attendees) | **DONE** (macOS EventKit via helper) |
| P1b.6 | CLI: `minutes list` (list recent meetings) and `minutes search <query>` (full-text search) | **DONE** |
| P1b.7 | End-to-end test: record real meeting → diarized transcript + AI summary → markdown | **DONE** |

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

**Resolved**: Native diarization via **pyannote-rs** (MIT, uses pyannote segmentation-3.0 + WeSpeaker ONNX models, no Python). Legacy pyannote subprocess kept for users who already have the Python setup. Setup: `minutes setup --diarization` downloads ~34 MB of models.

```toml
# ~/.config/minutes/config.toml

[transcription]
model = "small"                  # Default: small (466MB). Options: tiny, base, small, medium, large-v3
model_path = "~/.minutes/models" # Where whisper models are stored
min_words = 10                   # Below this threshold, mark as "no-speech"

[diarization]
engine = "pyannote-rs"  # Recommended: native Rust, no Python (~34MB models)
# engine = "pyannote"   # Legacy: Python subprocess (requires pip install pyannote.audio)
# engine = "none"       # Skip diarization entirely (default)

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
| P2.1 | MCPB scaffold (manifest.json, Node.js MCP server) | **DONE** |
| P2.2 | MCP tool: `start_recording` (spawns Rust binary) | **DONE** |
| P2.3 | MCP tool: `stop_recording` (triggers pipeline) | **DONE** |
| P2.4 | MCP tool: `list_meetings` (reads meeting store) | **DONE** |
| P2.5 | MCP tool: `search_meetings` (full-text + frontmatter query) | **DONE** |
| P2.6 | MCP tool: `get_transcript` (returns specific meeting) | **DONE** |
| P2.7 | Package as .mcpb, test install in Claude Desktop | **DONE** |
| P2.8 | README for MCPB distribution | **DONE** |

**Exit criteria**: Install extension in Claude Desktop → record meeting → ask Claude about it → Claude answers from transcript.

### Phase 2b: Claude Code Plugin (Week 2, parallel with MCPB) — "Meeting skills in your terminal"

**Goal**: Claude Code users get `/minutes record`, `/minutes search`, `/minutes list` as skills. Meeting context enriches coding sessions. Publishable as a Claude Code plugin.

| Task | Description | Beads ID |
|------|-------------|----------|
| P2b.1 | Plugin scaffold: `plugin.json` manifest with name, version, description, components | **DONE** |
| P2b.2 | Skill: `/minutes record` — start/stop recording with hotkey awareness | **DONE** |
| P2b.3 | Skill: `/minutes search <query>` — search past meetings from terminal, render results in chat | **DONE** |
| P2b.4 | Skill: `/minutes list` — list recent meetings with summaries, attendees, dates | **DONE** |
| P2b.5 | Skill: `/minutes recap` — summarize today's meetings into a digest | **DONE** (as `/minutes weekly`) |
| P2b.6 | Agent: `meeting-analyst` — subagent for cross-meeting intelligence queries ("what did X say about Y?") | **DONE** |
| P2b.7 | Hook: `SessionStart` — inject recent meeting context if meetings exist from today | **DONE** |
| P2b.8 | Hook: `PostToolUse` — auto-tag meetings with current project/repo context when recording stops | **DONE** |
| P2b.9 | MCP server config in plugin (`.mcp.json`) — reuse MCPB's MCP tools within Claude Code | **DONE** |
| P2b.10 | Plugin README + install instructions (`claude plugin add minutes`) | **DONE** |

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
- `PostToolUse` hook fires when `minutes stop` completes — reads the current git repo and adds `project: my-project` (or whatever) to the meeting's YAML frontmatter
- The `meeting-analyst` agent has access to all meeting files and can answer cross-meeting questions autonomously

**Exit criteria**: `claude plugin add minutes` → `/minutes record` works → meeting context appears in Claude Code sessions → `/minutes search "pricing"` returns results.

### Phase 2c: Notetaking — "What you thought was important"

**Goal**: Let users annotate recordings with plain-text notes from any interface. Notes feed into the LLM summarizer as high-signal context, producing better summaries. Users never need to know markdown — they just type.

**Core insight**: The transcript captures *what was said*. Notes capture *what mattered*. When the LLM sees both, the summary is dramatically better — it knows which parts of a 45-minute meeting the user actually cared about.

#### How it works

```
DURING RECORDING:

  User types/says:  "Alex prefers monthly billing not annual"
       │
       ├── CLI:      minutes note "Alex prefers monthly billing not annual"
       ├── Claude:   "note that Alex wants monthly"  →  add_note MCP tool
       ├── Tauri:    types in note field  →  calls minutes note
       └── Dispatch: "add a note about pricing"  →  add_note MCP tool
       │
       ▼
  ~/.minutes/current-notes.md:
       [4:23] Alex prefers monthly billing not annual
       [12:10] Jordan agreed with Alex

ON STOP:

  Pipeline reads:
    current.wav       →  transcript
    current-notes.md  →  user notes (timestamped)
    current-context   →  pre-meeting context (from --context flag)
       │
       ▼
  LLM prompt includes:
    "The user marked these moments as important during the meeting.
     Weight them heavily in the summary:
     [4:23] Alex prefers monthly billing not annual
     [12:10] Jordan agreed with Alex"
       │
       ▼
  Better summary. Notes appear in ## Notes section of output.
```

#### Pre-meeting context

```bash
minutes record --title "1:1 with Alex" \
  --context "Discuss Q2 roadmap. Follow up on API launch timeline. Alex was hesitant last time."
```

The `--context` flag stores text in `~/.minutes/current-context.txt`. The pipeline passes it to the LLM: "Before the meeting, the user noted this context: [text]". This produces summaries that understand *why* the meeting happened.

For voice memos: `minutes process idea.m4a --note "Had this idea while driving — about onboarding redesign"`

#### Post-meeting annotation

```bash
minutes note --meeting ~/meetings/2026-03-17-planning-call.md "Follow-up: Alex confirmed via email on Mar 18"
```

Appends to the existing file's `## Notes` section. Timestamped with the annotation time, not the recording time.

#### Plain-text input, always

Users type plain text. Never markdown. The system adds the timestamp prefix and formats into markdown behind the scenes. In the Tauri app, notes render visually (not as raw markdown). In Claude, notes render naturally in conversation.

#### Tasks

| Task | Description | Beads ID |
|------|-------------|----------|
| P2c.1 | `notes.rs` module: read/write `~/.minutes/current-notes.md`, timestamp calculation from recording start, append with atomic write | **DONE** |
| P2c.2 | `minutes note "text"` CLI command: check recording in progress, calculate timestamp, append to current-notes.md | **DONE** |
| P2c.3 | `--context "text"` flag on `minutes record`: saves to `~/.minutes/current-context.txt`, included in frontmatter | **DONE** |
| P2c.4 | `--note "text"` flag on `minutes process`: adds context for voice memos being processed | **DONE** |
| P2c.5 | Pipeline integration: read notes + context files, pass to LLM summarizer as high-priority context, include `## Notes` section in markdown output | **DONE** |
| P2c.6 | LLM prompt update: instruct summarizer to weight user notes heavily, cross-reference notes with transcript timestamps | **DONE** |
| P2c.7 | `--meeting <path>` flag on `minutes note`: append post-meeting annotations to existing files | **DONE** |
| P2c.8 | `add_note` MCP tool: calls `minutes note` for Claude Desktop/Cowork/Dispatch | **DONE** |
| P2c.9 | `/minutes note` Claude Code skill | **DONE** |
| P2c.10 | Tauri note input: text field visible during recording, lines auto-timestamped, rendered visually (not raw markdown) | **DONE** |

**Exit criteria**: `minutes record` → type `minutes note "important point"` in another terminal → `minutes stop` → markdown has `## Notes` section with timestamped notes → LLM summary references the noted moments.

#### Output example

```
---
title: Q2 Roadmap Planning with Alex
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
context: "Discuss Q2 roadmap, follow up on API launch timeline"
---

## Summary
- Alex proposed moving the API launch from April to May
- Jordan supported the later date for better testing
- Compromise: soft launch in April, GA in May

## Notes
- [4:23] Alex prefers monthly billing not annual
- [12:10] Jordan agreed with Alex
- [28:00] Compromise: soft launch April, GA May
- [Mar 18, post-meeting] Alex confirmed via email the timeline works

## Decisions
- [x] Soft launch API in April, GA in May

## Action Items
- [ ] @alex: Draft the migration guide by Friday
- [ ] @jordan: Set up staging environment for soft launch

## Transcript
[0:00] So let's talk about the timeline for the API launch...
```

### Phase 3: Tauri Desktop App (Week 3-4) — "Native menu bar experience"

**Goal**: Desktop app with system tray + main window. Record, transcribe, search from a native UI.

| Task | Description | Status |
|------|-------------|--------|
| P3.1 | Tauri v2 project setup (system tray + main window) | **DONE** |
| P3.2 | Main app window: meeting list, search, recording controls, date grouping | **DONE** |
| P3.3 | Audio visualizer: real-time RMS level bars during recording | **DONE** |
| P3.4 | Recording indicator: tray icon → red dot, menu items gray out | **DONE** |
| P3.5 | Note taking UI: inline quick-note during recording + standalone popup | **DONE** |
| P3.6 | macOS mic permission: Info.plist + entitlements.plist + .app bundle | **DONE** |
| P3.7 | .app bundle build: `cargo tauri build --bundles app` → Minutes.app | **DONE** |
| P3.8 | Calendar polling (macOS EventKit or ical) | **DONE** (calendar-events helper + tray menu) |
| P3.9 | Meeting suggestion notification (2 min before) | **DONE** (macOS notification via calendar poll) |
| P3.10 | Auto-start on login (launchd integration) | **DONE** (`minutes service install`) |
| P3.11 | First-run onboarding (permissions, model download, LLM config) | **DONE** (onboarding flow + readiness center) |
| P3.12 | Homebrew cask formula | **DONE** (`brew install --cask silverstein/tap/minutes`) |
| P3.13 | Window close → hide to tray (app keeps running) | **DONE** (prevent_close + hide) |

**Bugs fixed during P3 implementation (2026-03-18):**
- **Critical**: WAV normalization bug — 16-bit samples divided by i32::MAX made audio 65,000x too quiet for whisper. Fixed by dividing by actual bit-depth max.
- Separate stop_flag from recording state (inverted semantics caused immediate exit)
- macOS tmux users get silence (tmux server doesn't inherit terminal's mic permission)

**Exit criteria**: Install via `brew install --cask minutes` → app sits in menu bar → suggests recording → produces searchable meeting memory.

### Phase 4: Intelligence + Cowork (Week 5+) — "Meeting memory, not meeting notes"

**Goal**: Cross-meeting intelligence, people memory, and full Claude Cowork/Dispatch integration.

#### 4a: Intelligence Layer

| Task | Description | Beads ID |
|------|-------------|----------|
| P4a.1 | Cross-meeting search ("what did we decide about X across all meetings?") | **DONE** (`minutes research`, `minutes person`) |
| P4a.2 | People profiles — build attendee context over time (decisions, commitments, topics they care about) | **DONE** (`minutes person <name>`) |
| P4a.3 | **Structured intent extraction** — LLM summarization emits a machine-readable `intents:` block in YAML frontmatter alongside the human-readable summary. Decisions, action items, open questions, and commitments as typed entries with `who`, `what`, `status`, and `by_date` fields. The markdown stays readable; the frontmatter becomes agent-queryable. MCP `search_meetings` gains a `--intents-only` filter that returns structured data, not prose. | **DONE** |
| P4a.4 | **Decision consistency tracking** — the `meeting-analyst` agent compares new meeting intents against the existing intent index. Flags contradictions ("March 5: launch date April 1. March 12: launch date pushed to May.") and stale commitments ("Case committed to send spec by March 8 — no follow-up in 3 meetings since"). Outputs a `consistency_report` via MCP tool, not just a wall of text. | **DONE** (`minutes consistency`) |
| P4a.5 | PARA entity auto-linking (meetings → people → projects) | TBD |
| P4a.6 | QMD collection auto-registration (`qmd collection add minutes ~/meetings`) | **DONE** (`minutes qmd register`) |
| P4a.7 | Daily note backlinks (append meeting summaries to daily notes) | **DONE** (`daily_notes.rs`) |

#### 4b: Claude Cowork + Dispatch Integration

| Task | Description | Beads ID |
|------|-------------|----------|
| P4b.1 | **Cowork connector research** — investigate how Cowork connectors work, what APIs/protocols are available, whether MCPB tools are automatically available in Cowork | TBD |
| P4b.2 | **Dispatch recording flow** — "Start recording" from phone → Dispatch → Mac captures audio. Test end-to-end with Dispatch research preview | TBD |
| P4b.3 | **Cowork meeting brief** — when user starts a Cowork session, auto-surface "You had 3 meetings today, here's what happened" | TBD |
| P4b.4 | **Dispatch meeting summary** — after recording stops, push structured summary back to phone via Dispatch ("Done. 3 action items, 2 decisions.") | TBD |
| P4b.5 | **Cowork follow-up automation** — Claude autonomously drafts follow-up emails based on action items, sends via Cowork connectors (Gmail, Slack) | TBD |
| P4b.6 | **Multi-meeting synthesis in Cowork** — "Prepare me for my call with Alex" → Cowork pulls all past meetings with Alex, summarizes themes, open action items, relationship context | TBD |
| P4b.7 | **Cowork persistent memory** — meeting intelligence persists across Cowork sessions. Claude remembers what was discussed even weeks later | TBD |
| P4b.8 | **Dispatch quick commands** — from phone: "What did we decide yesterday?" / "Any action items for me?" / "Who mentioned the budget issue?" | TBD |

#### 4c: Platform Expansion

| Task | Description | Beads ID |
|------|-------------|----------|
| P4c.1 | Windows support (WASAPI audio capture) | **PARTIAL** (core builds + tests pass on Windows CI; audio capture untested) |
| P4c.2 | Linux support (PulseAudio/PipeWire capture) | **PARTIAL** (core builds + tests pass on Ubuntu CI; audio capture untested) |
| P4c.3 | Obsidian community plugin (thin wrapper around CLI) | TBD |

**Cowork integration architecture:**
```
┌─────────────────────────────────────────────────────────────┐
│ User's Phone                                                 │
│                                                              │
│ Claude iOS/Android App                                       │
│ ├── "Start recording my meeting"     ──────┐                │
│ ├── "What did we decide yesterday?"  ──────┤  Dispatch       │
│ ├── "Prepare me for the Alex call"  ──────┤  (sends to Mac) │
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
1. Testing that MCPB tools work reliably through Dispatch (still in preview, reliability may vary)
2. Building smart compound workflows ("prepare me for Alex" = search_meetings + get_person_context + calendar lookup)
3. Ensuring meeting context persists across Cowork sessions (may need a session-start injection pattern)

**Exit criteria for Phase 4**: From phone: "Prepare me for my 2pm with Alex" → Claude surfaces past meeting history, open action items, their key concerns, and suggested talking points — all from local meeting data, no cloud required.

---

## Cowork/Dispatch User Stories (Detailed)

These ground the Cowork integration in real scenarios:

### Story 1: Pre-Meeting Prep (from phone)
```
User (on phone, heading to a meeting):
  → Opens Claude app → Dispatch
  → "Prepare me for my meeting with the Acme team at 2pm"

Claude (on Mac, via Cowork):
  → Calls list_meetings(attendee: "Acme")
  → Calls get_person_context(name: "Acme")
  → Calls calendar(event: "2pm today")
  → Synthesizes: "You've met with Acme 4 times. Last meeting (March 3):
    they discussed the integration timeline and asked about API access.
    Open action items: you committed to sending a technical spec.
    Their priorities: faster onboarding, data portability."

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
    3 action items: you need to send the spec doc to Alex by Friday."
```

### Story 3: Cross-Meeting Intelligence (in Claude Code)
```
Developer (in Claude Code, working on a feature):
  → /minutes search "API redesign"

Claude Code (meeting-analyst agent):
  → Searches all meetings
  → "Found 5 meetings mentioning the API redesign:
    - March 17: Decided on REST over GraphQL
    - March 10: Alex raised pagination concerns
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
  → "What does Alex usually bring up in our meetings?"

Claude:
  → Calls get_person_context(name: "Alex")
  → "You've had 23 meetings with Alex over 4 months.
    Topics they care most about: API design (8 mentions),
    performance (6), documentation (5).
    Their style: detail-oriented, often raises edge cases.
    Last interaction: March 17, they committed to the migration guide.
    Open commitments: review competitor API docs (due March 21)."
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
├── Phase 4a: Intelligence Layer (7 tasks)
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
| 3 | Dispatch still in preview | Low | Dispatch is bonus, not requirement. Core works without it | Accepted |
| 4 | QMD dependency limits adoption | Low | QMD strictly optional. Core output is markdown files | Accepted |
| 5 | Meetily has 10K stars — why compete? | Medium | Different positioning: "conversation memory for AI" vs "open source Granola" | Accepted |
| 6 | BYO-LLM dilutes Claude advantage | Low | MCPB integration IS the moat. BYO-LLM is summarization only | Accepted |
| 7 | Speaker diarization quality varies | Medium | pyannote-rs native engine (DER ~11-19%) + calendar attendee mapping | **RESOLVED** |
| 8 | Scope creep (CLI + MCPB + menu bar + intelligence) | High | **Phase 1 split into 1a/1b. 2 weeks, not 1. MVP = capture + transcribe only.** | **RESOLVED** |
| 9 | Name availability | Low | **RESOLVED: `minutes` — crates.io + PyPI available** | **RESOLVED** |
| 10 | Maintenance sustainability | Medium | Keep core tiny: ~1000 lines Rust + ~300 lines Node.js | Active |
| 11 | **Falcon licensing blocks MIT distribution** | **High** | **RESOLVED: Falcon is NOT viable. Use pyannote via subprocess (AGPL-safe) or sherpa-onnx (Apache 2.0).** See Diarization Decision in Phase 1b. | **RESOLVED** |
| 12 | Phase 1 timeline too aggressive (was 1 week) | Medium | **RESOLVED: Split into Phase 1a (pipeline, week 1) + Phase 1b (intelligence, week 2)** | **RESOLVED** |
| 13 | Business-specific content in public repo kills trust | Medium | **RESOLVED: Moved to gitignored `.claude/` directory. Public plan is pure open-source story.** | **RESOLVED** |
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
- [x] ~~**Domain registration**: Register `getminutes.dev` before someone else does~~ **RESOLVED** — registered useminutes.app + useminutes.app on Vercel
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

See `.claude/growth-strategy.md` (gitignored — private strategy doc).

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
- [MacStories Dispatch Review](https://www.macstories.net/stories/hands-on-with-claude-dispatch-for-cowork/) — Real-world testing of Dispatch preview
