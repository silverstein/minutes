---
name: minutes-ideas
description: Surface recent voice memos and ideas captured from any device. Use when the user asks "what ideas did I have?", "what were my recent memos?", "what did I record while walking?", or wants to recall a captured thought.
---

## Local Minutes Host

This plugin runs in ChatGPT Work or Codex with access to the local computer.
Use the connected Minutes MCP tools when they cover the requested operation;
resolve their exact registered names from the host's tool list. CLI commands
and bundled helpers require a local shell on the same computer as Minutes.
If that runtime is unavailable, report the missing capability instead of
fabricating results or treating a command as executed. Keep capture, audio
processing, dictation insertion, and OS permissions in the local Minutes engine.
The plugin does not capture audio in ChatGPT's browser or upload a library.
Tool results used as model context are shared with the AI host. Preserve the
canonical skill's meeting-access, confirmation, and external-send rules.


# /minutes-ideas — Recent Voice Memos & Ideas

Surface voice memos and ideas captured from any device in the last 14 days.
This is the recall layer for the cross-device ghost context pipeline.

## How to run

1. Search for recent voice memos using the `minutes` CLI:

```bash
minutes list --content-type memo --limit 20
```

2. Require exit status 0 and use the JSON written to stdout. If the CLI is
   unavailable or fails, report the memo source unavailable; never scan the
   memo directory directly.

3. Present the memos as a clean list:
   - Date, title, duration, device (if from iPhone)
   - Ask: "Want to dig into any of these?"

4. If the user picks one, run `minutes get "<exact path>" --json`, require exit
   status 0, and present only the returned transcript/summary. Never reopen the
   list path through the host filesystem.

## Ghost Context

These memos were captured on the user's phone (or Mac) and automatically
transcribed by the Minutes watcher. They may contain ideas, thoughts,
observations, or reminders that the user recorded while away from their desk.

When the user asks "what was that idea I had while walking?" — search these
memos first, then broaden to full meeting search if needed.
