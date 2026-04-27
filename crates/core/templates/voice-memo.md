---
name: Voice Memo
slug: voice-memo
version: 1.0.0
description: Single-speaker brain dump. Captures ideas, intent, and follow-ups without imposing a meeting structure.
keywords: [voice-memo, memo, brain-dump, single-speaker, capture]
extends_base: true
additional_instructions: |
  Treat this as a single-speaker brain dump, not a meeting. The PARTICIPANTS
  section will usually be empty or contain only the recorder; do not invent
  participants. Lean toward KEY POINTS and ACTION ITEMS that capture
  intent. DECISIONS and OPEN QUESTIONS may be empty for short memos and
  that is fine.
language: en
---

# Voice Memo Template

Use this for solo brain dumps captured via `minutes record` or processed
via the folder watcher. The Phase 1 template stays tonal: it tells the
summarizer not to fabricate participants or force a meeting shape onto a
single-speaker recording.

## Coming in later phases

- `triggers.transcript_keywords` for post-hoc auto-selection when no
  template was picked at record time (Phase 4).
- `extract:` schema for `intent`, `next_actions`, `open_loops` (Phase 2).
