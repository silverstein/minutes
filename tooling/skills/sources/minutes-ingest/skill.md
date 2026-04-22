---
name: minutes-ingest
description: Extract facts from meetings and update your knowledge base — person profiles, chronological log, and index. Use when the user asks "ingest my meetings", "update my knowledge base", "extract facts from meetings", "sync meetings to wiki", "backfill knowledge", or wants their PARA/Obsidian/wiki profiles updated from conversation data.
triggers:
  - ingest my meetings
  - update my knowledge base
  - extract facts from meetings
  - sync meetings to wiki
  - backfill knowledge
user_invocable: true
metadata:
  display_name: Minutes Ingest
  short_description: Extract facts from meetings and update your knowledge base — person profiles, chronological log, and index.
  default_prompt: Use Minutes Ingest for this task.
  site_category: Knowledge
  site_example: /minutes-ingest
  site_best_for: Extract structured facts from meetings into an external knowledge base.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-ingest/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-ingest/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-ingest

Process meetings through the knowledge extraction pipeline to update person profiles, append to the knowledge log, and maintain the index.

## Prerequisites

The `[knowledge]` section must be configured in `~/.config/minutes/config.toml`:

```toml
[knowledge]
enabled = true
path = "/path/to/knowledge/base"
adapter = "wiki"  # or "para", "obsidian"
engine = "none"   # or "agent" for LLM extraction
min_confidence = "strong"
```

If not configured, explain what's needed and offer to help set it up.

## How to run

### Single meeting
```bash
minutes ingest ~/meetings/2026-04-03-strategy-call.md
```

### All meetings (backfill)
```bash
minutes ingest --all
```

### Preview without writing (recommended first time)
```bash
minutes ingest --all --dry-run
```

## What it does

1. **Reads** each meeting's YAML frontmatter (decisions, action_items, entities, intents)
2. **Extracts** structured facts with confidence levels and source provenance
3. **Updates** person profiles in the knowledge base (adapter-dependent format)
4. **Appends** to `log.md` with a timestamped entry for each ingested meeting
5. **Skips** facts that already exist (deduplication) or are below the confidence threshold

## Safety guarantees

- **`engine = "none"` (default)**: Only extracts from parsed YAML frontmatter. No LLM involved, zero hallucination risk.
- **Confidence thresholds**: Facts below `min_confidence` are counted as "skipped" but never written.
- **Provenance**: Every fact records which meeting it came from and when.
- **Deduplication**: Facts whose text already appears in a person's profile are skipped.
- **Dry-run**: Always suggest `--dry-run` first if the user hasn't used ingest before.

## Interpreting the output

```
Ingesting 73 meeting(s) into knowledge base at /path/to/kb
  2026-04-03-strategy.md — 4 written, 1 skipped — Mat, Dan
  2026-04-05-standup.md — 2 written, 0 skipped — Alice
  SKIP 2026-03-18-test.md: no frontmatter

Done. 6 fact(s) written, 1 skipped, 1 error(s), 3 people updated.
```

- **written**: facts that passed confidence threshold and didn't already exist
- **skipped**: facts below confidence threshold (logged, not written)
- **SKIP**: files that couldn't be parsed (no frontmatter, invalid YAML, etc.)

## Gotchas

- **Meetings without summarization have no structured data** — If a meeting was recorded before summarization was enabled, its frontmatter won't have `action_items` or `decisions`. The ingest will correctly extract 0 facts. This is expected, not an error.
- **`engine = "agent"` requires an AI CLI** — If the user wants richer LLM-based extraction from transcript body text, they need `claude`, `codex`, or `gemini` on PATH.
- **PARA adapter writes `items.json`** — If the user's knowledge base uses the PARA format, facts go into `areas/people/{slug}/items.json` with atomic fact schema (id, status, supersededBy).
- **First run should be dry-run** — Always suggest `minutes ingest --all --dry-run` before the first real run so the user can see what would be extracted.
