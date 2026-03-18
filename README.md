# minutes

**Your AI remembers every conversation you've had.**

Agents have run logs. Humans have conversations. **minutes** captures the human side — the decisions, the intent, the context that agents need but can't observe — and makes it queryable.

Record a meeting. Capture a voice memo on a walk. Ask Claude what was decided three weeks ago. It just works.

```
$ minutes record --context "Q2 pricing discussion with Sarah"
Recording... (Ctrl-C or `minutes stop` to finish)
  Tip: add notes with `minutes note "your note"` in another terminal

^C
Stopping recording...
Transcribing.....
Saved: ~/meetings/2026-03-17-q2-pricing-discussion-with-sarah.md
```

```
$ minutes search "pricing"
2026-03-17 — Q2 Pricing Discussion with Sarah [meeting]
  [4:20] I think $399 makes more sense for independent advisors...
```

```
$ minutes actions --assignee mat
Open action items (2):
  @mat: Send pricing doc (by Friday)
    from: 2026-03-17 — Q2 Pricing Discussion with Sarah
  @mat: Set up $399 tier experiment (by next week)
    from: 2026-03-17 — Q2 Pricing Discussion with Sarah
```

## How it works

```
Audio → Transcribe → Diarize → Summarize → Structured Markdown
         (local)     (speakers)   (LLM)       (decisions,
        whisper.cpp   pyannote   Claude/       action items,
                                 Ollama/       searchable)
                                 OpenAI
```

Everything runs locally. Your audio never leaves your machine (unless you opt into cloud LLM summarization).

## Install

```bash
# Homebrew (macOS)
brew tap silverstein/tap
brew install minutes

# From source (requires Rust + cmake)
cargo install --path crates/cli

# Download whisper model (~466MB)
minutes setup --model small
```

## Features

### Record meetings
```bash
minutes record                                    # Record from mic
minutes record --title "Standup" --context "Sprint 4 blockers"  # With context
minutes stop                                      # Stop from another terminal
```

### Take notes during meetings
```bash
minutes note "Sarah wants $399 not $499"          # Timestamped, feeds into summary
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

### Post-meeting annotations
```bash
minutes note --meeting ~/meetings/2026-03-17-pricing.md "Sarah confirmed via email"
```

## Output format

Meetings save as markdown with structured YAML frontmatter:

```yaml
---
title: Q2 Pricing Discussion with Sarah
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
context: "Discuss Q2 pricing, follow up on $499 decision"
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
  - text: Run pricing experiment at $399 with 10 advisors
    topic: pricing experiment
---

## Summary
- Sarah proposed lowering advisor pricing from $499 to $399/mo
- Compromise: run experiment with 10 advisors at $399

## Notes
- [4:23] Sarah wants $399 not $499
- [12:10] Logan agreed

## Decisions
- [x] Run pricing experiment at $399 with 10 advisors

## Action Items
- [ ] @mat: Send pricing doc by Friday
- [ ] @sarah: Review competitor grid by March 21

## Transcript
[SPEAKER_0 0:00] So let's talk about the pricing...
[SPEAKER_1 4:20] I think $399 makes more sense...
```

Works with [Obsidian](https://obsidian.md), grep, or any markdown tool. Action items and decisions are queryable via the CLI and MCP tools.

## Claude integration

minutes is a native extension for the Claude ecosystem.

### Claude Desktop (MCP)
```json
{
  "mcpServers": {
    "minutes": {
      "command": "node",
      "args": ["path/to/minutes/crates/mcp/dist/index.js"]
    }
  }
}
```

8 MCP tools: `start_recording`, `stop_recording`, `get_status`, `list_meetings`, `search_meetings`, `get_meeting`, `process_audio`, `add_note`

Ask Claude: *"What did Sarah say about pricing last Tuesday?"* — Claude searches your meetings and answers.

### Claude Code (Plugin)
```
.claude/plugins/minutes/
├── 5 skills: /minutes record, search, list, recap, note
├── 1 agent: meeting-analyst (cross-meeting intelligence)
└── 1 hook: auto-tags meetings with current git repo
```

### Cowork / Dispatch
MCP tools are automatically available in Cowork. From your phone via Dispatch: *"Start recording"* → Mac captures → Claude processes → summary on your phone.

## Voice memos (iPhone → Mac)

No iOS app needed. Use Apple's built-in Voice Memos + a Shortcut:

1. Run `minutes watch` on your Mac (or install the launchd service for auto-start)
2. Record in Voice Memos on iPhone
3. Share → "Save to Minutes" (iCloud Drive sync)
4. Markdown appears in `~/meetings/memos/`

Supports `.m4a`, `.mp3`, `.wav`, `.ogg`, `.webm`. Format conversion is automatic via [symphonia](https://github.com/pdeljanov/Symphonia).

## Configuration

Optional — minutes works out of the box.

```toml
# ~/.config/minutes/config.toml

[transcription]
model = "small"           # tiny (75MB), base, small (466MB), medium, large-v3 (3.1GB)

[summarization]
engine = "claude"         # claude, openai, ollama, or none
ollama_url = "http://localhost:11434"
ollama_model = "llama3.2"

[diarization]
engine = "pyannote"       # pyannote (best quality) or none

[search]
engine = "builtin"        # builtin (regex) or qmd (semantic)

[watch]
paths = ["~/.minutes/inbox"]
settle_delay_ms = 2000    # iCloud sync safety delay
```

## Architecture

```
minutes/
├── crates/core/    12 Rust modules — the engine (shared by all interfaces)
├── crates/cli/     CLI binary — 12 commands
├── crates/mcp/     MCP server — 8 tools for Claude Desktop
├── tauri/          Menu bar app — system tray, dark-mode UI
└── .claude/plugins/minutes/   Claude Code plugin — 5 skills + 1 agent
```

Single `minutes-core` library shared by CLI, MCP server, and Tauri app. Zero code duplication.

**Built with:** Rust, [whisper.cpp](https://github.com/ggerganov/whisper.cpp), [symphonia](https://github.com/pdeljanov/Symphonia), [cpal](https://github.com/RustAudio/cpal), [Tauri v2](https://v2.tauri.app/), [ureq](https://github.com/algesten/ureq)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

MIT
