---
name: minutes-list
description: List recent meetings and voice memos. Use when the user asks "what meetings did I have", "show my recent recordings", "any meetings today", "list my voice memos", or wants an overview of their meeting history. Also use when they need to find a specific meeting by browsing rather than searching.
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


# /minutes-list

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

Treat each returned `path` as a hint, not a retained capability. To read a
specific meeting, reauthorize it through `minutes get "<exact path>" --json`,
require exit status 0, and use only that result. Never reopen a result path with
the host `Read` tool.

## Gotchas

- **Returns nothing on first use** — If `~/meetings/` doesn't exist yet or has no `.md` files, list returns an empty array. This is normal before the first recording.
- **JSON goes to stdout, human-readable to stderr** — If you pipe the output (e.g., `minutes list | jq`), you get JSON only. The human-readable table goes to stderr.
- **In-progress recordings don't appear** — List only shows completed, processed recordings. Use `minutes status` to check if something is currently recording.
- **Sorted by date in frontmatter, not file modification time** — If you manually edit a meeting file, it won't change its position in the list.
