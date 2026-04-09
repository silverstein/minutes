---
name: minutes-ideas
description: Surface recent voice memos and ideas captured from any device. Use when the user asks "what ideas did I have?", "what were my recent memos?", "what did I record while walking?", or wants to recall a captured thought.
triggers:
  - what ideas did I have?
  - what were my recent memos?
  - what did I record while walking?
allowed_tools:
  - Bash
  - Read
  - AskUserQuestion
metadata:
  display_name: Minutes Ideas
  short_description: Surface recent voice memos and ideas captured from any device.
  default_prompt: Use Minutes Ideas for this task.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-ideas/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-ideas/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-ideas — Recent Voice Memos & Ideas

Surface voice memos and ideas captured from any device in the last 14 days.
This is the recall layer for the cross-device ghost context pipeline.

## How to run

1. Search for recent voice memos using the `minutes` CLI:

```bash
minutes list --type memo --limit 20 --json 2>/dev/null
```

2. If no results or CLI unavailable, scan `~/meetings/memos/` directly:

```bash
ls -t ~/meetings/memos/*.md 2>/dev/null | head -20
```

3. For each memo found, read the frontmatter to get title, date, duration, and device:

```bash
head -20 "<path>"
```

4. Present the memos as a clean list:
   - Date, title, duration, device (if from iPhone)
   - Ask: "Want to dig into any of these?"

5. If the user picks one, read the full file and present the transcript/summary.

## Ghost Context

These memos were captured on the user's phone (or Mac) and automatically
transcribed by the Minutes watcher. They may contain ideas, thoughts,
observations, or reminders that the user recorded while away from their desk.

When the user asks "what was that idea I had while walking?" — search these
memos first, then broaden to full meeting search if needed.
