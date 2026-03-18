---
name: minutes-list
description: List recent meetings and voice memos. Use when the user asks "what meetings did I have", "show my recent recordings", "any meetings today", "list my voice memos", or wants an overview of their meeting history. Also use when they need to find a specific meeting by browsing rather than searching.
user_invocable: true
---

# /minutes list

Show recent meetings and voice memos, sorted newest-first.

## Usage

```bash
# List last 10 recordings (default)
minutes list

# Show more
minutes list --limit 20

# Only voice memos
minutes list -t memo

# Only meetings
minutes list -t meeting
```

## Output

Human-readable list to stderr, JSON array to stdout. Each entry has:
- `title`, `date`, `content_type`, `path`

To read a specific meeting's full transcript, use `Read` on its `path`.
