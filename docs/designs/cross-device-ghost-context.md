# Cross-Device Ghost Context Layer

> CEO plan promoted from CEO review on 2026-03-24

## Problem

Minutes v0.6.0 shipped dictation mode (global hotkey → whisper → clipboard/daily note), but dictation only works at the desk. The most valuable thoughts happen while walking, driving, commuting — away from the Mac. Voice memos recorded on iPhone sit disconnected from the AI workflow.

## Vision

Minutes becomes ambient memory — every thought you have, anywhere, on any device, joins your AI assistant's context before you ask for it. "Claude, what was that idea I had while walking?" works because your walking thought quietly became searchable memory 60 seconds after you recorded it on your phone. No new app, no cloud, no subscription. The pipeline is invisible. Claude feels like it was there.

**Key insight (eureka):** Every competitor builds intelligence in their app UI. Minutes builds dumb markdown and lets the AI assistant (Claude via MCP) be the intelligence layer. The pipeline should optimize for speed and invisibility, not smarts.

## Scope

### Core Components (7)

1. **Duration-based routing in watch.rs** — Probe audio duration via symphonia, route <120s as `ContentType::Memo` (skips diarization), >=120s as `ContentType::Meeting`
2. **Apple Shortcut (.shortcut file)** — "Save to Minutes" on iPhone share sheet, saves to iCloud Drive inbox
3. **Sidecar JSON metadata ingestion** — Optional JSON alongside audio with `captured_at`, `device`, `source` fields
4. **Sync folder inbox** — iCloud Drive (macOS default), Dropbox, Google Drive, Syncthing, or any folder sync. Configurable in `minutes setup`
5. **SessionStart hook upgrade** — Surfaces recent voice memos (last 3 days, max 5) alongside today's meetings
6. **`recent_ideas` MCP resource** — Returns voice memos from last 14 days, frontmatter-only scan
7. **`/minutes-ideas` Claude Code skill** — Interactive recall of recent voice memos

### Cherry-Picks (3)

8. **macOS notification on processing** — "Your thought is now searchable: Pricing idea for consultants"
9. **Daily note backlinks for voice memos** — Extend `daily_notes.rs` to append voice memo backlinks
10. **MCP event emission (VoiceMemoProcessed)** — New event variant in `events.rs` JSONL log

### Architecture

```
iPhone Voice Memo
       │
       ▼
iCloud Drive sync (5-30s)
       │
       ▼
~/.../Minutes Inbox/
  ├── audio.m4a
  └── audio.json (sidecar, optional)
       │
       ▼
watch.rs → audio_duration() → symphonia probe
       │
  ┌────┴────┐
  │<120s    │>=120s
  ▼         ▼
Memo        Meeting
pipeline    pipeline
       │
       ▼
markdown.rs (with device/source frontmatter)
       │
       ├──→ events.rs (VoiceMemoProcessed)
       ├──→ daily_notes.rs (backlink)
       ├──→ macOS notification
       └──→ SessionStart hook (next Claude session)
```

### Deferred (ROADMAP.md)

- Full Ambient Memory: LLM classification, intent extraction on memos (P2)
- Tray menu voice memo count, QMD auto-registration (minor, add during implementation)
- Shortcuts Automation (iOS testing required)
- Cross-meeting intelligence on the go (depends on Cowork/Dispatch)

## Design Doc

Full implementation details: `~/.gstack/projects/silverstein-minutes/<hostname>-main-design-20260324-092635.md`

## Review Status

- CEO Review: CLEAR (SELECTIVE EXPANSION, 5 proposed, 3 accepted, 1 deferred)
- Eng Review: PENDING (required gate before implementation)
- Codex second opinion: ran during /office-hours (endorsed "ghost context" approach)
