# minutes

[![GitHub stars](https://img.shields.io/github/stars/silverstein/minutes?style=social)](https://github.com/silverstein/minutes)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

**Open-source conversation memory.** &nbsp; [useminutes.app](https://useminutes.app)

Agents have run logs. Humans have conversations. **minutes** captures the human side — the decisions, the intent, the context that agents need but can't observe — and makes it queryable.

Record a meeting. Capture a voice memo on a walk. Ask Claude *"what did I promise Sarah?"* — and get an answer. Your AI remembers every conversation you've had.

> **Own every conversation you've ever had.** Cloud meeting tools rent your own conversations back to you. Minutes writes every meeting to `~/meetings/` as plain markdown, which every AI you use (Claude Code, Codex, Gemini CLI, Cursor, OpenCode, Pi) reads directly. No SDK. No API key. No vendor to outlive. Ten years from now, `grep` still works on your corpus. &nbsp;[**For agents →**](https://useminutes.app/for-agents) &nbsp;·&nbsp; [**Frontmatter schema →**](docs/frontmatter-schema.md)

<p align="center">
  <img src="docs/assets/demo.gif" alt="minutes demo — record, dictate, phone sync, AI recall" width="750">
</p>

### Works with

<p align="center">
  <a href="#claude-code-plugin">Claude Code</a> &bull;
  <a href="#any-mcp-client-claude-code-codex-gemini-cli-claude-desktop-or-your-own-agent">Codex</a> &bull;
  <a href="#opencode-cli">OpenCode</a> &bull;
  <a href="#pi-coding-agent">Pi</a> &bull;
  <a href="#any-mcp-client-claude-code-codex-gemini-cli-claude-desktop-or-your-own-agent">Gemini CLI</a> &bull;
  <a href="#any-mcp-client-claude-code-codex-gemini-cli-claude-desktop-or-your-own-agent">Claude Desktop</a> &bull;
  <a href="#mistral-vibe">Mistral Vibe</a> &bull;
  <a href="#vault-sync-obsidian--logseq">Obsidian</a> &bull;
  <a href="#vault-sync-obsidian--logseq">Logseq</a> &bull;
  <a href="#phone--desktop-voice-memo-pipeline">Phone Voice Memos</a> &bull;
  Any MCP client
</p>

## Quick start

```bash
# macOS — Desktop app (menu bar, recording UI, AI assistant)
brew install --cask silverstein/tap/minutes

# macOS — CLI only
brew tap silverstein/tap && brew install minutes

# Any platform — from source (requires Rust + cmake; Windows also needs LLVM)
cargo install minutes-cli                          # macOS/Linux
cargo install minutes-cli --no-default-features    # Windows (see install notes below)

# MCP server only — no Rust needed (Claude Code, Codex, OpenCode, Gemini CLI, Claude Desktop, etc.)
npx minutes-mcp
```

```bash
minutes setup --model small   # Download whisper model (466MB, recommended)
minutes record                # Start recording
minutes stop                  # Stop and transcribe
```

## Docs and agent surfaces

The README is now the product overview and install guide, not the only home for agent-facing reference.

- Agent entry point: <https://useminutes.app/for-agents>
- MCP tools reference: <https://useminutes.app/docs/mcp/tools>
- MCP tools markdown mirror: <https://useminutes.app/docs/mcp/tools.md>
- Error reference: <https://useminutes.app/docs/errors>
- Concise agent index: <https://useminutes.app/llms.txt>
- Full agent index: <https://useminutes.app/llms-full.txt>

## Choose your surface

- `Desktop app` — `brew install --cask silverstein/tap/minutes`
  Best for first recording, live capture, Recall, and post-meeting artifact work.
- `MCP server` — `npx minutes-mcp`
  Best for agent-first search, recall, and meeting-memory workflows in Claude Desktop, Codex, OpenCode, Gemini CLI, and other MCP clients.
- `CLI` — `brew tap silverstein/tap && brew install minutes`
  Best for terminal-first local operator workflows, import, search, and vault sync.
- `Claude Code plugin` — `claude plugin marketplace add silverstein/minutes`
  Best for workflow guidance, prep, debrief, and meeting coaching with the lifecycle skills and hooks.
- `OpenCode project integration` — built-in `.opencode/skills/` + `.opencode/commands/`
  Best for OpenCode users who want native `/minutes-*` commands plus the portable Minutes skill pack in the repo.

## How it works

```
Audio → Transcribe → Diarize → Summarize → Structured Markdown → Relationship Graph
         (local)     (local)     (LLM)       (decisions,            (people, commitments,
        whisper.cpp  pyannote-rs Claude/       action items,          topics, scores)
        /parakeet    (native)    Ollama/       people, entities)      SQLite index
                                Mistral/OpenAI
```

Everything runs locally. Your audio never leaves your machine (unless you opt into cloud LLM summarization). Speakers are identified via native diarization. The relationship graph indexes people, commitments, and topics across all meetings for instant queries.

## Features

### Record meetings
```bash
minutes record                                    # Record from mic
minutes record --title "Standup" --context "Sprint 4 blockers"  # With context
minutes record --language ur                      # Force Urdu (ISO 639-1 code)
minutes record --device "AirPods Pro"             # Use specific audio device
minutes record --template standup                 # Apply a summary template
minutes stop                                      # Stop from another terminal
```

### Take notes during meetings
```bash
minutes note "Alex wants monthly billing not annual billing"          # Timestamped, feeds into summary
minutes note "Logan agreed"                       # LLM weights your notes heavily
```

### Process voice memos
```bash
minutes process ~/Downloads/voice-memo.m4a        # Any audio format
minutes watch                                     # Auto-process new files in inbox
```

### Search everything
```bash
minutes search "pricing"                          # Full-text search
minutes search "onboarding" -t memo               # Filter by type
minutes actions                                   # Open action items across all meetings
minutes actions --assignee sarah                   # Filter by person
minutes list                                      # Recent recordings
```

### Relationship intelligence

> *"What did I promise Sarah?"* — the query nobody else can answer.

```bash
minutes people                                     # Who you talk to, how often, about what
minutes people --rebuild                           # Rebuild the relationship index
minutes commitments                                # All open + overdue commitments
minutes commitments --person alex                   # What did I promise Alex?
```

Tracks people, commitments, topics, and relationship health across every meeting. Detects when you're losing touch with someone. Suggests duplicate contacts ("Sarah Chen" ↔ "Sarah"). Powered by a SQLite index rebuilt from your markdown in <50ms.

### Cross-meeting intelligence
```bash
minutes research "pricing strategy"               # Search across all meetings
minutes person "Alex"                              # Build a profile from meeting history
minutes consistency                                # Flag contradicting decisions + stale commitments
```

### Live transcript (real-time coaching)
```bash
minutes live                                     # Start real-time transcription
minutes stop                                     # Stop live session
```
Streams local transcription to a JSONL file in real time — any AI agent can read it mid-meeting for live coaching. Depending on your build and config, live mode can run on Whisper, Parakeet, or the experimental Apple Speech standalone-live path. Apple Speech currently applies only to standalone live transcript (`minutes live`), not recording-sidecar, dictation, or batch transcription, and it falls back to a ready Parakeet backend before Whisper if Apple Speech is unavailable or fails mid-session. See [docs/APPLE_SPEECH.md](docs/APPLE_SPEECH.md) for the current Apple Speech scope. The MCP `read_live_transcript` tool provides delta reads (by line cursor or wall-clock duration). Works with Claude Code, Codex, OpenCode, Gemini CLI, or any agent that reads files. The Tauri desktop app has a Live Mode toggle that starts this with one click.

### Dictation mode
```bash
minutes dictate                                  # Speak → text appears as you talk
minutes dictate --stdout                         # Output to stdout instead of clipboard
```
Text streams progressively as you speak (partial results every 2 seconds). By default it accumulates across pauses and writes the combined text to clipboard + daily note when dictation ends. Set `[dictation] accumulate = false` to keep the older per-pause behavior. Local whisper, no cloud.

### Command palette (desktop app)
Press `⌘⇧K` from anywhere on macOS to open a keyboard-first palette of every Minutes command. Start a recording, drop a note into the active session, jump to the latest meeting, search transcripts, or rename the meeting open in your assistant — all without leaving the keyboard. Backed by a single typed command registry in `minutes-core`, so visibility follows real backend state: stop-recording only appears while you're recording, mid-recording dictation rows are hidden, and the list re-fetches automatically when state changes.

Recents float to the top with their original payload intact (re-running a `Search transcripts: pricing` from history skips the retype). The shortcut defaults on for both fresh installs and upgrades, with a one-time macOS notification on first launch announcing the binding. Disable it from the Settings overlay (Command Palette section) or by setting `[palette] shortcut_enabled = false` in your config file (`$XDG_CONFIG_HOME/minutes/config.toml` when `XDG_CONFIG_HOME` is set, otherwise `~/.config/minutes/config.toml`). The Settings dropdown also offers `⌘⇧O` and `⌘⇧U` if `⌘⇧K` collides with your IDE.

### Templates (RFC 0001, Phase 1)
```bash
minutes template list                             # Bundled + project + user templates
minutes template show standup                     # Inspect a template
minutes record --template standup                 # Apply when recording
minutes process voice-memo.m4a --template voice-memo
```
Templates layer prompt-level guidance on top of the baseline structured extraction (`KEY POINTS`, `DECISIONS`, `ACTION ITEMS`, `OPEN QUESTIONS`, `COMMITMENTS`, `PARTICIPANTS`). Phase 1 ships four bundled templates (`meeting`, `standup`, `1-on-1`, `voice-memo`) and resolves overrides from `.minutes/templates/` (project) and `~/.minutes/templates/` (user). Custom `extract:` schemas, compliance rules, and clinical templates land in later phases — see [`docs/rfcs/0001-templates.md`](docs/rfcs/0001-templates.md).

### Try it without a mic
```bash
minutes demo --full                              # Seed 5 sample meetings (Snow Crash theme)
minutes demo --query                             # Cross-meeting intelligence demo
minutes demo --clean                             # Remove sample meetings
```

The interactive demo seeds interconnected meetings, then lets you pick a thread to explore. Two storylines, five meetings, zero setup.

### System diagnostics
```bash
minutes health                                   # Check model, mic, calendar, disk
minutes demo                                     # Run a pipeline test (bundled audio, no mic)
```

## Switching from Granola?

Import your meeting history into Minutes' conversation memory. Once imported, your meetings become searchable context for AI agents, feed the relationship graph for meeting prep, and surface action items and decision patterns across months of conversations.

```bash
minutes import granola --dry-run    # Preview what will be imported
minutes import granola              # Import all meetings to ~/meetings/
```

Reads from `~/.granola-archivist/output/`. Meetings are converted to Minutes' markdown format with YAML frontmatter. Duplicates are skipped automatically. All your data stays local — no cloud, no $18/mo.

### Want transcripts and AI summaries?

[granola-to-minutes](https://github.com/calvindotsg/granola-to-minutes) exports richer data using [granola-cli](https://github.com/magarcia/granola-cli), a community-built CLI tool (not affiliated with Granola Labs) that accesses Granola's internal API:

| | `minutes import granola` | `granola-to-minutes` |
|---|---|---|
| **Data source** | Local export (`~/.granola-archivist/output/`) | Granola internal API via [granola-cli](https://github.com/magarcia/granola-cli) |
| **Notes & transcript** | ✓ | ✓ |
| **AI-enhanced summaries** | — | ✓ |
| **Action items & decisions** | — | ✓ (extracted via Claude) |
| **Speaker attribution** | — | ✓ (`speaker_map` in frontmatter) |
| **Setup** | Export from Granola desktop app | `npm install -g granola-to-minutes` |
| **Works on free tier** | ✓ | ✓ |
| **API stability** | N/A (local files) | Internal API — may change without notice |

```bash
npx granola-to-minutes export    # Export to ~/meetings/
```

## Output format

Meetings save as markdown with structured YAML frontmatter:

```yaml
---
title: Q2 Pricing Discussion with Alex
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
context: "Discuss Q2 pricing, follow up on annual billing decision"
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
  - assignee: sarah
    task: Review competitor grid
    due: March 21
    status: open
decisions:
  - text: Run pricing experiment at monthly billing with 10 advisors
    topic: pricing experiment
---

## Summary
- Alex proposed lowering API launch timeline from annual billing to monthly billing/mo
- Compromise: run experiment with 10 advisors at monthly billing

## Transcript
[SPEAKER_0 0:00] So let's talk about the pricing...
[SPEAKER_1 4:20] I think monthly billing makes more sense...
```

Works with [Obsidian](https://obsidian.md), grep, or any markdown tool. Action items and decisions are queryable via the CLI and MCP tools.

## Phone → desktop voice memo pipeline

No phone app needed. Record a thought on your phone, and it becomes searchable memory on your desktop. Claude even surfaces recent memos proactively — "you had a voice memo about pricing yesterday."

The watcher is folder-agnostic — it processes any audio file that lands in a watched folder. Pick the sync method that matches your setup:

| Phone | Desktop | Sync method |
|-------|---------|-------------|
| **iPhone** | **Mac** | iCloud Drive (built-in, ~5-30s) |
| **iPhone** | **Windows/Linux** | iCloud for Windows, or Dropbox/Google Drive |
| **Android** | **Any** | Dropbox, Google Drive, Syncthing, or any folder sync |
| **Any** | **Any** | AirDrop, USB, email — drop the file in the watched folder |

### Setup (one-time)

**Step 1: Create a sync folder** — pick one that syncs between your phone and desktop:

```bash
# macOS + iPhone (iCloud Drive)
mkdir -p ~/Library/Mobile\ Documents/com~apple~CloudDocs/minutes-inbox

# Any platform (Dropbox)
mkdir -p ~/Dropbox/minutes-inbox

# Any platform (Google Drive)
mkdir -p ~/Google\ Drive/minutes-inbox

# Or just use the default inbox (manually drop files into it)
# ~/.minutes/inbox/  ← already exists
```

**Step 2: Add the sync folder to your watch config** in your config file (`$XDG_CONFIG_HOME/minutes/config.toml` when `XDG_CONFIG_HOME` is set, otherwise `~/.config/minutes/config.toml`):

```toml
[watch]
paths = [
  "~/.minutes/inbox",
  # Add your sync folder here — uncomment one:
  # "~/Library/Mobile Documents/com~apple~CloudDocs/minutes-inbox",  # iCloud
  # "~/Dropbox/minutes-inbox",                                       # Dropbox
  # "~/Google Drive/minutes-inbox",                                  # Google Drive
]
```

**Step 3: Set up your phone**

<details>
<summary><strong>iPhone (Apple Shortcuts)</strong></summary>

1. Open the **Shortcuts** app on your iPhone
2. Tap **+** → Add Action → search **"Save File"**
3. Set destination to `iCloud Drive/minutes-inbox/` (or your Dropbox/Google Drive folder)
4. Turn OFF "Ask Where to Save"
5. Tap the **(i)** info button → enable **Share Sheet** → set to accept **Audio**
6. Name it **"Save to Minutes"**

Now: Voice Memos → Share → **Save to Minutes** → done.
</details>

<details>
<summary><strong>Android</strong></summary>

Use any voice recorder app + your cloud sync of choice:

- **Dropbox**: Record with any app → Share → Save to Dropbox → `minutes-inbox/`
- **Google Drive**: Record → Share → Save to Drive → `minutes-inbox/`
- **Syncthing** (no cloud): Set up a Syncthing share between phone and desktop pointing at your watched folder. Fully local, no cloud.
- **Tasker/Automate** (power users): Auto-move new recordings from your recorder app to the sync folder.
</details>

<details>
<summary><strong>Manual (any phone)</strong></summary>

No sync setup needed — just get the audio file to your desktop's watched folder:
- **AirDrop** (Apple): Share → AirDrop to Mac → move to `~/.minutes/inbox/`
- **Email**: Email the recording to yourself → save attachment to watched folder
- **USB**: Transfer directly
</details>

**Step 4: Start the watcher** (or install as a background service):

```bash
minutes watch                  # Run in foreground
minutes service install        # Install all background services (macOS launchd / Linux systemd)
minutes service status         # Check what's running
minutes service restart        # Restart all services (e.g. after upgrading the binary)
```

`minutes service install` sets up three agents:

| Agent | Schedule | What it does |
|-------|----------|--------------|
| **watcher** | Always on | Processes voice memos from `~/.minutes/inbox/` |
| **weekly-summary** | Sundays 7pm | Generates a weekly digest to `~/.minutes/automations/` |
| **proactive-context** | Daily 8am | Builds a context bundle (recent meetings, stale commitments, losing-touch alerts) |

> **Upgrading?** `minutes service install` is idempotent. Re-running it after a binary
> upgrade rewrites all plists/units and reloads with the new binary path.

### How it works

```
Phone (any)                   Desktop (any)
───────────                   ─────────────
Record voice memo        →    Cloud sync / manual transfer
Share to sync folder               │
                                   ▼
                            minutes watch detects file
                                   │
                            probe duration (<2 min?)
                              ├── yes → memo pipeline (fast, no diarization)
                              └── no  → meeting pipeline (full)
                                   │
                            transcribe → save markdown
                                   │
                            ├── event: VoiceMemoProcessed
                            ├── daily note backlink
                            └── surfaces in next Claude session
```

Short voice memos (<2 minutes) automatically route through the fast memo pipeline — no diarization, no heavy summarization. Long recordings get the full meeting treatment. The threshold is configurable: `dictation_threshold_secs = 120` in `[watch]`.

### Optional: sidecar metadata

If your phone workflow also saves a `.json` file alongside the audio (same name, `.json` extension), Minutes reads it for enriched metadata:

```json
{"device": "iPhone", "source": "voice-memos", "captured_at": "2026-03-24T08:41:00-07:00"}
```

This adds `device` and `captured_at` to the meeting's frontmatter. Works with any automation tool (Apple Shortcuts, Tasker, etc.).

Supports `.m4a`, `.mp3`, `.wav`, `.ogg`, `.webm`. Format conversion is automatic — uses [ffmpeg](https://ffmpeg.org/) when available (recommended for non-English audio), falls back to [symphonia](https://github.com/pdeljanov/Symphonia).

### Vault sync (Obsidian / Logseq)

```bash
minutes vault setup              # Auto-detect vaults, configure sync
minutes vault status             # Check health
minutes vault sync               # Copy existing meetings to vault
```

Three strategies: **symlink** (zero-copy), **copy** (works with iCloud/Obsidian Sync), **direct** (write to vault). `minutes vault setup` detects your vault and recommends the right strategy automatically.

## Claude integration

minutes is a native extension for the Claude ecosystem. **No API keys needed** — Claude summarizes your meetings when you ask, using your existing Claude subscription.

```
You: "Summarize my last meeting"
Claude: [calls get_meeting] → reads transcript → summarizes in conversation

You: "What did Alex say about pricing?"
Claude: [calls search_meetings] → finds matches → synthesizes answer

You: "Any open action items for me?"
Claude: [calls list_meetings] → scans frontmatter → reports open items
```

### Any MCP client (Claude Code, Codex, OpenCode, Gemini CLI, Claude Desktop, or your own agent)

Minutes exposes a standard MCP server. Point any MCP-compatible client at it:

```json
{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}
```

Canonical MCP reference now lives at:

- <https://useminutes.app/docs/mcp/tools>
- <https://useminutes.app/docs/mcp/tools.md>
- <https://useminutes.app/llms.txt>

The MCP surface currently includes recording control, meeting search/retrieval, relationship memory, structured insights, live transcript reading, dictation, QMD integration, and an interactive dashboard resource. Tool names, resource URIs, and prompt templates are generated from the live product surface instead of hand-maintained in this README.

**Interactive dashboard (Claude Desktop):** tools render an inline interactive UI via [MCP Apps](https://modelcontextprotocol.io/specification/2025-03-26/server/utilities/apps) — meeting list with filter/search, detail view with fullscreen + "Send to Claude" context injection, People tab with relationship cards and click-through profiles, and consistency reports. Text-only clients see the same data as plain text.

### OpenCode CLI

Minutes now ships a project-local OpenCode integration layer:

- `.opencode/skills/minutes-*` for OpenCode's one-level skill discovery
- `.opencode/commands/minutes-*.md` so you can run native slash commands like `/minutes-brief`
- the same portable runtime helpers used by the Codex/Gemini skill pack

OpenCode also reads this repo's `AGENTS.md`, so the project rules carry over automatically.

For MCP tools in OpenCode, the official CLI flow is:

```bash
opencode mcp add
```

Choose a local stdio server and point it at:

```bash
npx minutes-mcp
```

If you're wiring OpenCode against this repo before the next npm release is cut,
point it at the repo-local entrypoint instead:

```bash
npm --prefix /absolute/path/to/minutes/crates/mcp exec tsx src/index.ts
```

For the native skill/command workflow, just launch OpenCode in this repo:

```bash
opencode
```

Then use commands like:

```text
/minutes-brief
/minutes-prep Alex
/minutes-debrief
/minutes-weekly
/minutes-video-review /absolute/path/to/demo.mp4
```

### Pi coding agent

Minutes works with Mario Zechner's `pi` coding agent in two places:

- `engine = "agent"` can call `pi` directly for local meeting summarization.
- Pi auto-discovers this repo's existing `.agents/skills/minutes/` skill pack, so there is no separate `.pi/skills` tree to keep in sync.

Install Pi, log in or configure a provider, then set:

```toml
[summarization]
engine = "agent"
agent_command = "pi"
```

Minutes invokes Pi in non-interactive, no-tools mode with a private prompt file. Configure provider/model defaults in Pi itself; Minutes does not currently forward extra `[summarization]` CLI flags. That keeps summarization opt-in and prevents the agent from writing to the repo while it is turning a transcript into notes.

This is separate from Inflection's Pi chatbot/model. Inflection's Pi models are optimized for warmth and emotional intelligence, but the Inflection API terms say not to send regulated personal data. Meeting transcripts often contain personal data, so Minutes does not route transcripts to Inflection by default.

### Mistral Vibe

Add Minutes to your `~/.vibe/config.toml`:

```toml
[[mcp_servers]]
name = "minutes"
transport = "stdio"
command = "npx"
args = ["minutes-mcp"]
```

All 29 tools are available in Vibe as `minutes_*` (e.g. `minutes_start_recording`, `minutes_search_meetings`).

### Claude Code (Plugin)

Install the plugin from the marketplace:
```bash
# First-time install
claude plugin marketplace add silverstein/minutes
claude plugin install minutes
# Restart Claude Code to load skills, hooks, and the meeting-analyst agent
```

**Upgrading?** `claude plugin marketplace add` is a no-op when the marketplace is already on disk — it won't fetch new versions. To pick up new skills and hooks after a release, refresh the marketplace mirror first, then update the plugin:
```bash
claude plugin marketplace update minutes    # git pulls the local marketplace mirror
claude plugin update minutes@minutes        # installs the new version into the cache
# Restart Claude Code to apply
```

19 skills, 1 agent, 2 hooks:
```
├── Capture:      /minutes-record, note, list, recap, cleanup, verify, setup
├── Search:       /minutes-search
├── Lifecycle:    /minutes-brief, prep, debrief, weekly
├── Coaching:     /minutes-tag, mirror
├── Knowledge:    /minutes-ideas, lint, ingest
├── Intelligence: /minutes-graph
├── Artifacts:    /minutes-video-review
├── Agent:        meeting-analyst (cross-meeting intelligence)
└── Hooks:        SessionStart meeting briefings + PostToolUse recording alerts
```

**Meeting lifecycle skills** — inspired by [gstack](https://github.com/garrytan/gstack)'s interactive skill pattern:

```
/minutes-brief                      → fast one-pager (or fired automatically by hook 15 min before calls)
  ↓
/minutes-prep "call with Alex"      → deeper relationship brief + talking points + goal-setting
  ↓
minutes record → minutes stop       → hook alerts if decisions conflict with prior meetings
  ↓
/minutes-tag won|lost|stalled       → 5-second outcome label (unlocks mirror correlation)
  ↓
/minutes-debrief                    → "You wanted to resolve pricing. Did you?"
  ↓
/minutes-mirror                     → talk-time, hedging, what your winning meetings have in common
  ↓
/minutes-weekly                     → themes, decision arcs, stale items, Monday brief
  ↓
/minutes-video-review <video-or-url> → durable artifact bundle from a Loom, ScreenPal, or local walkthrough
  ↓
/minutes-graph "everyone who mentioned Stripe"  → cross-meeting entity queries
```

For the stable public agent-facing docs surface, use:

- <https://useminutes.app/for-agents>
- <https://useminutes.app/docs/mcp/tools>
- <https://useminutes.app/docs/errors>

### Minutes Desktop Assistant

The Tauri menu bar app includes a built-in AI Assistant window backed by the
same local meeting artifacts. It runs as a singleton assistant session:

- `AI Assistant` opens or focuses the persistent assistant window
- `Discuss with AI` reuses that same assistant and switches its active meeting focus
- Auto-updates from GitHub Releases with signed artifacts, never interrupting a recording

### Cowork / Dispatch
The currently verified path for Cowork is plugin-oriented, not “raw MCP automatically appears everywhere.” Minutes ships a Cowork extension scaffold under `integrations/claude-cowork-extension/` and a local bundle build script at `scripts/build_cowork_extension.sh`. On this machine, the bundle build is verified; actual in-Cowork install/use remains a proof-of-life workflow, not a guaranteed default path. Treat Dispatch-triggered recording and other mobile workflows as experimental until the plugin-native path is installed and checked end to end.

### Optional: automated summarization

```toml
# Use your existing Claude Code, Codex, OpenCode, or Pi subscription (recommended)
[summarization]
engine = "agent"
agent_command = "claude"  # or "codex" / "opencode" / "pi"

# Or use Mistral API (requires MISTRAL_API_KEY)
[summarization]
engine = "mistral"
mistral_model = "mistral-large-latest"

# Or use a free local LLM
[summarization]
engine = "ollama"
ollama_model = "llama3.2"

# Or use any OpenAI-compatible gateway/local server.
# Desktop users can paste cloud gateway keys in Settings; Minutes stores them
# in macOS Keychain and hydrates its own runtime secret without rewriting this
# shared config. CLI users can set any env var and name it below. Local servers
# can leave it blank.
[summarization]
engine = "openai-compatible"
openai_compatible_base_url = "https://openrouter.ai/api/v1"
openai_compatible_model = "openai/gpt-4o-mini"
openai_compatible_api_key_env = "OPENROUTER_API_KEY" # leave blank for local servers
```

### File-backed automation primitives

Minutes can emit small automation artifacts that are easy to schedule with
`launchd`, `cron`, or any external runner.

```bash
minutes automate weekly-summary --json
minutes automate proactive-context --json
```

Each run writes:

- a markdown artifact under `~/.minutes/automation-runs/`
- a matching JSON run record beside it

This is intentionally simple: explicit files, explicit output paths, and no
hidden scheduler subsystem.

### Codex epic runner

When you want Codex to keep draining a `bd` epic instead of stopping after one
child bead, use the repo-local epic runner:

```bash
node scripts/codex_epic_runner.mjs <epic-id> -- --full-auto
```

What it does:

- uses `bd` as the source of truth for epic ancestry and ready work
- picks the next ready non-epic descendant bead under the target epic
- claims that bead, runs `codex exec` against it, then checks whether the bead was actually closed
- continues only after a real close; pauses on blocked/needs-human outcomes instead of guessing

Dry-run the order first:

```bash
node scripts/codex_epic_runner.mjs <epic-id> --dry-run
```

If you install a Taskmaster-style Codex wrapper later, use it as the per-bead
engine without changing the epic logic:

```bash
node scripts/codex_epic_runner.mjs <epic-id> --taskmaster -- --sandbox danger-full-access -a never
```

This is intentionally separate from the Claude plugin hooks. The Minutes plugin
hooks are Claude-specific today; the Codex epic runner is a repo-local workflow
layer on top of `bd` and `codex exec`.

### Optional: knowledge base integration

Maintain a living knowledge base from your conversations — person profiles, decision history, and a chronological log that compounds over time. Inspired by [Karpathy's LLM Wiki pattern](https://gist.github.com/karpathy/442a6bf555914893e9891c11519de94f).

```toml
[knowledge]
enabled = true
path = "~/wiki"        # or your Obsidian vault, PARA system, etc.
adapter = "wiki"       # "wiki" (flat markdown), "para" (atomic facts), "obsidian" (wiki + [[links]])
engine = "none"        # "none" = structured YAML only (safest), "agent" = LLM extraction
min_confidence = "strong"
```

After each meeting, structured facts (decisions, action items, commitments) flow into person profiles automatically. Every fact carries provenance back to its source meeting.

```bash
minutes ingest --dry-run --all   # Preview what would be extracted
minutes ingest --all              # Backfill existing meetings
minutes ingest ~/meetings/call.md # Process a single meeting
```

Three output formats:
- **Wiki** — `people/{slug}.md` with facts grouped by category
- **PARA** — `areas/people/{slug}/items.json` with atomic facts (id, status, supersededBy)
- **Obsidian** — Wiki format with `[[wikilinks]]` for cross-references

Safety: default `engine = "none"` extracts only from parsed YAML frontmatter. No LLM call, zero hallucination risk. Confidence thresholds filter speculative facts. Corrupt data is backed up, never silently destroyed.

## Install

### macOS

```bash
# Desktop app (menu bar, recording UI, AI assistant)
brew install --cask silverstein/tap/minutes

# CLI only (terminal recording, search, vault sync)
brew tap silverstein/tap
brew install minutes

# Or from source (requires Rust + cmake)
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo install --path crates/cli
```

### Windows

```powershell
# Download pre-built binary from GitHub releases, or build from source:
# Requires: Rust, cmake, MSVC build tools, LLVM (for libclang)

# Install LLVM (needed by whisper-rs bindgen):
winget install LLVM.LLVM
[Environment]::SetEnvironmentVariable("LIBCLANG_PATH", "C:\Program Files\LLVM\bin", "User")
# Restart your terminal after setting LIBCLANG_PATH

# Full build (includes speaker diarization):
cargo install --path crates/cli

# Without speaker diarization:
cargo install --path crates/cli --no-default-features
```

> **Note:** If diarization fails to compile on Windows, use `--no-default-features`.
> This is a [known upstream issue](https://github.com/silverstein/minutes/issues/27)
> with `pyannote-rs`'s ONNX Runtime dependency. Everything except speaker labels works without it.

### Linux

```bash
# Debian/Ubuntu — full dep list:
sudo apt-get install -y \
  build-essential cmake pkg-config \
  clang libclang-dev \
  libasound2-dev libpipewire-0.3-dev libspa-0.2-dev \
  ffmpeg

cargo install minutes-cli
# or, from a checkout:
cargo install --path crates/cli
```

**Why each dep is needed:**
- `build-essential`, `cmake` — whisper.cpp build
- `clang`, `libclang-dev` — bindgen (used by `whisper-rs` and `pipewire-sys`)
- `libasound2-dev` — cpal's ALSA backend
- `libpipewire-0.3-dev`, `libspa-0.2-dev` — cpal's PipeWire backend (compiled unconditionally on Linux)
- `ffmpeg` — preferred audio decoder for `.m4a`/`.mp3`/`.ogg` (falls back to pure-Rust symphonia if absent)

**Other distros** (best-effort — Debian/Ubuntu is the validated path; please [open an issue](https://github.com/silverstein/minutes/issues) if any package name is wrong on your distro):

- **Fedora/RHEL**: `sudo dnf install -y gcc-c++ cmake pkgconf-pkg-config clang clang-devel alsa-lib-devel pipewire-devel ffmpeg-free`
- **Arch**: `sudo pacman -S --needed base-devel cmake clang alsa-lib pipewire ffmpeg`

### Chromebook (Crostini)

Yes, Minutes runs on a Chromebook via the Linux development environment (Crostini). The CLI is the supported path — there's no native ChromeOS build and the Tauri desktop app isn't exercised there, but the core engine, folder watcher, and MCP server all work.

**One-time ChromeOS setup:**

1. **Turn on Linux.** Settings → About ChromeOS → Developers → Linux development environment → Turn on. Pick a disk size of 10 GB or more (whisper models plus build artifacts).
2. **Grant microphone access to the Linux container.** Settings → Developers → Linux development environment → toggle **Allow Linux to access your microphone**. This is off by default and is the single most common reason `minutes record` produces silence on a Chromebook.
3. **Open the Linux terminal** and follow the [Debian/Ubuntu](#linux) install above (`apt-get install …` + `cargo install minutes-cli`).

**Verify the environment** before your first real recording:

```bash
minutes health          # confirms model, mic, disk, watcher
minutes record          # speak for 5 seconds
minutes stop
```

If `minutes health` flags the mic as missing, the ChromeOS mic toggle is off — not a cpal bug. Flip it on in Settings and re-run.

**What works well on a Chromebook:**

- `minutes watch` is the killer flow. Drop voice memos from your phone into a synced Google Drive / Dropbox folder that also mounts inside Crostini, and Minutes auto-transcribes them. No mic permission dance, no hotkey fight.
- CLI recording and transcription with the `tiny` / `base` / `small` models. Expect CPU-only performance — Crostini doesn't expose GPU acceleration to Linux apps, so skip `--features metal/cuda/vulkan` and pick a smaller model than you would on a Mac.
- The MCP server (`npx minutes-mcp`) for Claude Desktop or other MCP clients running inside the container.

**What to expect less of:**

- **No global hotkeys or tray app.** ChromeOS doesn't surface system-level shortcuts to Crostini. `minutes record` / `minutes stop` from the terminal is the intended flow.
- **No Tauri desktop app support.** It may build, but it isn't tested and the live-coaching / AI Assistant surface assumes a macOS-style window server.
- **Slower transcription.** A Chromebook CPU on the `small` model is usually 2–4x realtime for English. Budget accordingly, or lean on the folder watcher where latency doesn't matter.

If Crostini support breaks for you, please [open an issue](https://github.com/silverstein/minutes/issues) — Chromebook isn't a first-class test target yet, so real bug reports are the fastest way to harden it.

### GPU acceleration

macOS release binaries (DMG + `cargo install minutes-cli` from published CI
artifacts) ship with Metal enabled — `large-v3` runs ~2× faster than the
CPU-only build and offloads nearly all work to the GPU. Other backends remain
opt-in at build time.

| Backend | Platform | Feature flag | Prerequisites | Default in release |
|---------|----------|-------------|---------------|--------------------|
| Metal | macOS | `metal` | Xcode Command Line Tools | **Yes** |
| CoreML | macOS | `coreml` | Xcode Command Line Tools + `.mlmodelc` bundle | No |
| CUDA | Windows/Linux | `cuda` | [CUDA Toolkit](https://developer.nvidia.com/cuda-toolkit) | No |
| ROCm/HIP | Linux | `hipblas` | [ROCm](https://rocm.docs.amd.com/) 6.1+ (`hipcc`, `hipblas`, `rocblas`) | No |
| Vulkan | Windows/Linux | `vulkan` | [Vulkan SDK](https://vulkan.lunarg.com/sdk/home) (+ `vulkan-headers` on Arch) | No |

Metal is the only backend that is exercised daily by the maintainer. CUDA, ROCm/HIP,
and Vulkan should be considered experimental: they wire through to whisper.cpp via
whisper-rs and are expected to work, but have not been validated in CI.

```bash
# Apple Metal (macOS) — already enabled in the release DMG; use this for source builds
cargo install --path crates/cli --features metal

# Apple CoreML (macOS Neural Engine) — encoder-only; see note below
cargo install --path crates/cli --features metal,coreml

# NVIDIA GPU (Windows/Linux)
cargo install --path crates/cli --features cuda

# AMD GPU via ROCm (Linux — experimental)
cargo install --path crates/cli --features hipblas

# Vulkan (Windows/Linux — experimental)
cargo install --path crates/cli --features vulkan
```

> **CoreML note:** `--features coreml` only accelerates the Whisper encoder on
> the Apple Neural Engine. It requires the companion `ggml-<model>-encoder.mlmodelc`
> bundle next to the `.bin` weights (e.g. for `large-v3`, download
> [`ggml-large-v3-encoder.mlmodelc.zip`](https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-encoder.mlmodelc.zip)
> and unzip into `~/.minutes/models/`). Without it, whisper.cpp silently falls
> back to the CPU/Metal encoder. Stack it with `metal` for the best of both
> worlds — a subsequent PR will fetch the bundle automatically from
> `minutes setup --model large-v3 --coreml`.

> **Windows CUDA users:** You may need to set environment variables before building:
> ```powershell
> $env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v12.4"
> $env:CMAKE_CUDA_COMPILER = "$env:CUDA_PATH\bin\nvcc.exe"
> $env:LIBCLANG_PATH = "C:\Program Files\LLVM\bin"
> $env:CMAKE_GENERATOR = "NMake Makefiles"
> ```
> The first CUDA build takes longer than usual (compiling GPU kernels) — this is a one-time cost.

> **ROCm/HIP users:** The build expects ROCm installed at `/opt/rocm`. If your
> installation is elsewhere, set `HIP_PATH` before building:
> ```bash
> export HIP_PATH=/path/to/rocm
> ```
>
> **Vulkan users:** On Windows and macOS, set `VULKAN_SDK` to your SDK install
> root before building. On Linux, `whisper-rs-sys` links against the system
> `libvulkan`.

### Setup (all platforms)

```bash
# Download whisper model (also downloads Silero VAD model for non-English audio)
minutes setup --model small   # Recommended (466MB, good accuracy)
minutes setup --model tiny    # Fastest (75MB, but misses quiet audio)
minutes setup --model base    # Middle ground (141MB)

# Install ffmpeg for best transcription quality (strongly recommended for non-English audio)
brew install ffmpeg           # macOS
# apt install ffmpeg          # Linux
# Without ffmpeg, symphonia handles m4a/mp3 decoding — works for English but may
# produce loops on non-English audio. ffmpeg is optional but recommended.

# Enable speaker diarization (optional, ~34MB ONNX models)
minutes setup --diarization

# Alternative: use Parakeet engine (opt-in, local GPU via parakeet.cpp)
# Requires parakeet.cpp installed: https://github.com/Frikallo/parakeet.cpp
minutes setup --parakeet                          # Multilingual v3 (tdt-600m, ~1.2GB)
minutes setup --parakeet --parakeet-model tdt-ctc-110m  # English-only compact model (~220MB)
# Also installs native Silero VAD weights for the parakeet.cpp --vad path

# Enroll your voice for automatic speaker identification
minutes enroll              # Records 10s of your voice
minutes voices              # View enrolled profiles
```

### Speaker identification

Minutes maps anonymous speaker labels (`SPEAKER_1`, `SPEAKER_2`) to real names using four levels of confidence-aware attribution:

| Level | How | Confidence | Requires |
|-------|-----|-----------|----------|
| **0** | Calendar attendees + `identity.name` → deterministic mapping for 1-on-1 meetings | Medium | Calendar access, `[identity] name` in config |
| **1** | LLM analyzes transcript context clues and maps speakers to attendees | Medium (capped) | Attendees known + summarization engine or agent CLI |
| **2** | Your enrolled voice is matched against speaker segments | High | `minutes enroll` (one-time 10s recording) |
| **3** | You confirm "SPEAKER_1 is Sarah" after a meeting | High | `minutes confirm --meeting <path>` |

Only **High**-confidence attributions rewrite transcript labels. Medium/Low are stored in frontmatter (`speaker_map`) for Claude to surface when asked — "SPEAKER_1 is likely Sarah."

```bash
# Set your name (required for Levels 0-2)
# In your config file (`$XDG_CONFIG_HOME/minutes/config.toml` when set,
# otherwise `~/.config/minutes/config.toml`):
[identity]
name = "Your Name"

# Enroll your voice (Level 2)
minutes enroll                    # Record 10s sample
minutes enroll --file sample.wav  # Or from existing audio

# Confirm attributions after a meeting (Level 3)
minutes confirm --meeting ~/meetings/2026-03-25-standup.md
minutes confirm --meeting path.md --speaker SPEAKER_1 --name "Sarah" --save-voice

# Manage voice profiles
minutes voices              # List profiles
minutes voices --json       # JSON output
minutes voices --delete     # Remove all profiles
```

**Privacy**: Voice enrollment is self-only (no enrolling others). Level 3 confirmed profiles require explicit opt-in per person. Voice embeddings are stored locally in `~/.minutes/voices.db` with 0600 permissions. Nothing leaves your machine.

> **Platform notes:** Calendar integration (auto-detecting meeting attendees) requires macOS. Screen context capture works on macOS and Linux. The voice memo pipeline works on all platforms — any folder sync (iCloud, Dropbox, Google Drive, Syncthing) can feed the watcher. The `minutes service install` auto-start command requires macOS (launchd); on Linux, use systemd or cron. Speaker diarization (`pyannote-rs`) works on all platforms (CLI, Tauri app, and via MCP). All other features — recording, transcription, search, action items, person profiles — work on all platforms.

### Desktop app

```bash
# macOS — Homebrew cask (recommended)
brew install --cask silverstein/tap/minutes

# macOS — build from source
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
export MACOSX_DEPLOYMENT_TARGET=11.0
cargo tauri build --bundles app

# macOS — local desktop development with stable permissions
./scripts/install-dev-app.sh
```

The notarized Homebrew cask/update feed currently tracks the Apple Silicon desktop build. Intel Macs on macOS 15+ can still use the desktop app by building from source with the commands above.

```powershell
# Windows — build desktop installer from source
cargo install tauri-cli --version 2.10.1 --locked
cd tauri/src-tauri
cargo tauri build --ci --bundles nsis --no-sign
```

Tagged GitHub releases can include both a Windows NSIS installer as `minutes-desktop-windows-x64-setup.exe` and a raw desktop binary as `minutes-desktop-windows-x64.exe`. The installer is currently unsigned, so treat it as an advanced-user / preview distribution surface until Windows signing is added.

The desktop app adds a system tray icon, recording controls, audio visualizer, Recall, and a meeting list window. The current Windows desktop build covers recording, transcription, search, settings, and Recall. Calendar suggestions, call detection, tray copy/paste automation, and the native dictation hotkey remain macOS-only for now.

Release workflow details live in:

- [docs/RELEASE-MACOS.md](docs/RELEASE-MACOS.md)
- [docs/RELEASE-WINDOWS.md](docs/RELEASE-WINDOWS.md)

For macOS development, use a dedicated signed dev app identity:

- Production app: `/Applications/Minutes.app` (`com.useminutes.desktop`)
- Development app: `~/Applications/Minutes Dev.app` (`com.useminutes.desktop.dev`)

If you are testing hotkeys, Screen Recording, Input Monitoring, or repeated macOS permission prompts, launch only `Minutes Dev.app` via `./scripts/install-dev-app.sh`. Avoid the repo symlink `./Minutes.app`, raw `target/` binaries, or ad-hoc local bundles for TCC-sensitive testing.

This repository is open source, so local development does not require the
maintainer's Apple signing credentials:

- `./scripts/install-dev-app.sh` works with ad-hoc signing by default
- for more stable macOS permission behavior across rebuilds, set
  `MINUTES_DEV_SIGNING_IDENTITY` to a consistent local codesigning identity
- release signing and notarization remain maintainer/release workflows

For dictation, the recommended path is the standard shortcut in the desktop app
(`Cmd/Ctrl + Shift + D` by default). The raw-key path for keys like `Caps Lock`
is available as an advanced option but remains more fragile and permission-heavy.

**Privacy:** All Minutes windows are hidden from screen sharing by default — other participants on Zoom/Meet/Teams won't see the app. Toggle via the tray menu: "Hide from Screen Share ✓".

### Troubleshooting

**No speech detected / blank audio:**
The most common cause is microphone permissions. Check System Settings → Privacy & Security → Microphone and ensure your terminal app (or Minutes.app) has access.

**tmux users:** tmux server runs as a separate process that doesn't inherit your terminal's mic permission. Either run `minutes record` from a direct terminal window (not inside tmux), or use the Minutes.app desktop bundle which gets its own mic permission.

**Build fails with C++ errors on macOS 26+:**
whisper.cpp needs the SDK include path. Set `CXXFLAGS` as shown above before building.

**Dictation hotkey still fails after you enabled it in System Settings:**
The native hotkey uses macOS Input Monitoring, which is separate from Screen Recording. The fastest way to test the exact installed desktop identity is:

```bash
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app"
```

Use `./scripts/install-dev-app.sh` first so you are testing the stable development app identity rather than a raw `target/` build. The helper intentionally launches the app through LaunchServices; direct shell execution of `Contents/MacOS/minutes-app --diagnose-hotkey` can misreport TCC status.

### Updating

```bash
# macOS desktop app (Homebrew cask)
brew upgrade --cask silverstein/tap/minutes

# macOS CLI (Homebrew)
brew upgrade silverstein/tap/minutes

# From source (CLI)
git pull && cargo install --path crates/cli --features parakeet,metal

# From source (desktop app)
git pull
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo tauri build --bundles app --features parakeet,metal
# Then replace /Applications/Minutes.app with the new build from
# target/release/bundle/macos/Minutes.app

# GitHub release (desktop app)
# Download the latest .dmg from https://github.com/silverstein/minutes/releases
# and drag Minutes.app to /Applications, replacing the old version
```

For local source builds, keep the CLI and desktop app on the same transcription feature set. The repo build scripts now default to `MINUTES_BUILD_FEATURES=parakeet,metal`; override that env var only if you intentionally want a narrower build flavor.

Check your current version with `minutes --version` (CLI) or the Settings gear in the desktop app.

## Configuration

Optional — minutes works out of the box.

```toml
# By default: ~/.config/minutes/config.toml
# Or: $XDG_CONFIG_HOME/minutes/config.toml when XDG_CONFIG_HOME is set

[transcription]
engine = "whisper"        # "whisper" (default), "parakeet" (opt-in, lower WER), or "apple-speech" (experimental)
model = "small"           # whisper: tiny (75MB), base, small (466MB), medium, large-v3 (3.1GB)
# language = "ur"          # Force transcription language (ISO 639-1 code, e.g. "en", "ur", "es", "zh")
                          # Default: auto-detect. Set this for similar-sounding languages (Urdu/Hindi, etc.)
# engine = "apple-speech"  # Experimental: standalone `minutes live` only. Configure via config file or CLI, not desktop settings.
#                         # If Apple Speech cannot run, standalone live falls back to a ready Parakeet backend, then Whisper.
#                         # See docs/APPLE_SPEECH.md for current scope and limitations.
# parakeet_model = "tdt-600m"                    # parakeet: tdt-ctc-110m (English), tdt-600m (multilingual v3)
# parakeet_binary = "parakeet"                   # Path to parakeet.cpp binary (or name in PATH)
# parakeet_boost_limit = 25                      # Experimental: boost top graph-derived phrases (0 disables)
# parakeet_boost_score = 2.0                     # Experimental tuning for parakeet.cpp --boost-score
# parakeet_fp16 = true                           # Default on macOS Apple Silicon: ~35% faster transcription with lower GPU memory (see docs/designs/parakeet-perf-2026-04-14.md)
# parakeet_vocab = "tdt-600m.tokenizer.vocab"      # Safer when multiple Parakeet models are installed
# vad_model = "silero-v6.2.0"     # Silero VAD model (auto-downloaded by setup). Empty = disable.
                                   # Prevents whisper hallucination loops on non-English/noisy audio.

[summarization]
engine = "none"           # Default: Claude summarizes conversationally via MCP
                          # "auto" = auto-detect an installed agent CLI for pipeline summaries
                          # "agent" = uses your Claude Code, Codex, OpenCode, or Pi subscription (no API key)
                          # "ollama" = local, free
                          # "openai-compatible" = OpenRouter, Vercel/Cloudflare gateways, llama.cpp, LM Studio, etc.
                          # "claude" / "openai" = direct API key (legacy)
agent_command = "claude"  # Which CLI to use when engine = "agent" (claude, codex, opencode, pi, etc.)
ollama_url = "http://localhost:11434"
ollama_model = "llama3.2"
openai_compatible_base_url = "http://localhost:11434/v1"
openai_compatible_model = "llama3.2"
openai_compatible_api_key_env = "" # Blank means no Authorization header for local endpoints. Desktop cloud endpoints can still use a saved Keychain key without rewriting config.

[diarization]
engine = "auto"           # "auto" (default — uses pyannote-rs if models downloaded, otherwise skips),
                          # "pyannote-rs" (always on — native Rust, no Python),
                          # "pyannote" (legacy — requires pip install pyannote.audio),
                          # "none" (explicitly disabled)
# embedding_model = "cam++"  # "cam++" (default) or "cam++-lm" (~12% lower EER on benchmarks).
                          # Note: cam++-lm produces lower cosine similarities, so if you switch
                          # to it you should also lower voice.match_threshold to ~0.1–0.2.
# threshold = 0.5         # Speaker similarity threshold (0.0–1.0). Lower = fewer speakers.

[voice]
# enabled = true          # Voice profile matching during diarization (default: true if enrolled)
# match_threshold = 0.65  # Cosine similarity threshold for voice matching (higher = stricter).
                          # If using embedding_model = "cam++-lm", lower this to ~0.1–0.2.

[search]
engine = "builtin"        # builtin (regex) or qmd (semantic)

[watch]
paths = ["~/.minutes/inbox"]
settle_delay_ms = 2000              # Cloud sync safety delay (wait for file to finish syncing)
dictation_threshold_secs = 120      # Files shorter than this → memo (skip diarize). 0 = disable.
# Add cloud sync folders to watch for phone voice memos:
# paths = ["~/.minutes/inbox", "~/Dropbox/minutes-inbox"]

[screen_context]
enabled = false           # Opt-in: capture screenshots during recording for LLM context
interval_secs = 30        # How often to capture (seconds)
keep_after_summary = false # Delete screenshots after summarization (default: clean up)

[call_detection]
enabled = true            # macOS-only today
poll_interval_secs = 1
cooldown_minutes = 5
# Default apps stay conservative:
# apps = ["zoom.us", "Microsoft Teams", "Webex"]
#
# Browser-based integrations such as Google Meet are opt-in on purpose.
# If you want to dogfood browser detection, add the sentinel explicitly:
# apps = ["zoom.us", "Microsoft Teams", "Webex", "google-meet"]

[assistant]
agent = "claude"          # CLI launched by the Tauri AI Assistant
agent_args = []           # Optional extra args, e.g. ["--dangerously-skip-permissions"]
```

## Architecture

```
minutes/
├── crates/core/          45 Rust modules — the engine (shared by all interfaces)
├── crates/cli/           CLI binary — 49 commands (recording, search, health, templates, workflows)
├── crates/whisper-guard/ Anti-hallucination toolkit (VAD gating, dedup, noise trimming)
├── crates/reader/        Lightweight read-only meeting parser (no audio deps)
├── crates/assets/        Bundled assets (demo.wav)
├── crates/sdk/           TypeScript SDK — `npm install minutes-sdk` (query meetings programmatically)
├── crates/mcp/           MCP server — 29 tools + 6 resources + interactive dashboard
│   └── ui/               MCP App dashboard (vanilla TS → single-file HTML)
├── tauri/                Menu bar app — system tray, recording UI, singleton AI Assistant
└── .claude/plugins/minutes/   Claude Code plugin — 19 skills + 1 agent + 2 hooks
```

Single `minutes-core` library shared by CLI, MCP server, and Tauri app. Zero code duplication.

### Building your own agent on Minutes

Minutes is designed as infrastructure for AI agents. The MCP server is the primary integration surface:

- **Read meetings**: `list_meetings`, `search_meetings`, `get_meeting` return structured JSON
- **Track people**: `get_person_profile` builds cross-meeting profiles with topics, open commitments
- **Monitor consistency**: `consistency_report` flags conflicting decisions and stale commitments
- **Record + process**: `start_recording`, `stop_recording`, `process_audio` for pipeline control
- **Live coaching**: `start_live_transcript`, `read_live_transcript` for real-time mid-meeting access
- **Voice profiles**: `list_voices`, `confirm_speaker` for speaker identification workflows
- **Resources**: Stable URIs (`minutes://meetings/recent`, `minutes://actions/open`) for agent context injection

Any agent framework that speaks MCP can use Minutes as its conversation memory layer — the agent handles the intelligence, Minutes handles the recall.

**TypeScript SDK** — for direct programmatic access without MCP:

```bash
npm install minutes-sdk
```

```typescript
import { listMeetings, searchMeetings, parseFrontmatter } from "minutes-sdk";

const meetings = await listMeetings("~/meetings", 20);
const results = await searchMeetings("~/meetings", "pricing");
```

**Built with:** Rust, [whisper.cpp](https://github.com/ggerganov/whisper.cpp) (transcription), [pyannote-rs](https://github.com/pyannote/pyannote-rs) (speaker diarization), [Silero VAD](https://github.com/snakers4/silero-vad) (voice activity detection), [symphonia](https://github.com/pdeljanov/Symphonia) (audio decoding), [cpal](https://github.com/RustAudio/cpal) (audio capture), [Tauri v2](https://v2.tauri.app/) (desktop app), [ureq](https://github.com/algesten/ureq) (HTTP). Optional: [ffmpeg](https://ffmpeg.org/) (recommended for non-English audio decoding).

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=silverstein/minutes&type=Date)](https://star-history.com/#silverstein/minutes&Date)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT — Built by [Mat Silverstein](https://github.com/silverstein), founder of [X1 Wealth](https://x1wealth.com)
