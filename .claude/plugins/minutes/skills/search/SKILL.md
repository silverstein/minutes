---
name: minutes-search
description: Search past meeting transcripts and voice memos for specific topics, people, decisions, or ideas. Use this whenever the user asks "what did we discuss about X", "find that meeting where we talked about Y", "what did Alex say", "did we decide on", "what was that idea about", or any question that could be answered by searching their meeting history. Also use for "do I have any notes about" or "check my meetings for".
user_invocable: true
---

# /minutes search

Find information across all meeting transcripts and voice memos.

## Usage

```bash
# Basic search
minutes search "pricing strategy"

# Filter to just voice memos
minutes search "onboarding idea" -t memo

# Filter to just meetings
minutes search "sprint planning" -t meeting

# Date filter + limit
minutes search "API redesign" --since 2026-03-01 --limit 5
```

## Flags

| Flag | Description |
|------|-------------|
| `-t, --content-type <meeting\|memo>` | Filter by type |
| `--since <date>` | Only results after this date (ISO format, e.g., `2026-03-01`) |
| `-l, --limit <n>` | Maximum results (default: 10) |

## Output

Returns JSON to stdout with an array of matches. Each result includes:
- `title` — Meeting or memo title
- `date` — When it was recorded
- `content_type` — "meeting" or "memo"
- `snippet` — The line containing the match
- `path` — Full path to the markdown file

Human-readable output goes to stderr. To read the full transcript of a match, use `cat <path>` on any result's path.

## How search works

Search is case-insensitive and matches against both the transcript body and the YAML frontmatter title. It walks all `.md` files in `~/meetings/` (including the `memos/` subfolder).

For richer semantic search, users can configure QMD as the search engine in `~/.config/minutes/config.toml`:
```toml
[search]
engine = "qmd"
qmd_collection = "meetings"
```

## Tips for good searches

- Search for **what people said**, not document titles: `"we should postpone the launch"` not `"launch delay meeting"`
- Search for **names** to find everything someone discussed: `"Alex"` or `"Logan"`
- Search for **decisions**: `"decided"`, `"agreed"`, `"committed to"`
- Combine with `Read` to load the full context after finding a match
