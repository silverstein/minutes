---
name: minutes-graph
description: Policy-safe relationship rankings, commitments, aliases, person profiles, and topic research. Always use Minutes' bounded native CLI surfaces; never build or read a durable graph cache.
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


# /minutes-graph

Minutes builds relationship rankings, exact person profiles, and commitments from one supervised, process-private SQLite projection of stable policy-authorized Markdown plus confirmed identity corrections. One ordered snapshot authority spans corpus and corrections, and the worker is hard-limited for memory, output, and wall time. Topic research uses the separately bounded live-source search boundary. Both paths re-attest policy before returning facts. Do not fall back to a retired durable index or read meeting files directly.

## Privacy boundary

- Never walk meeting files, parse frontmatter yourself, or read raw transcripts for this skill.
- Never run `graph_build.py` or read `~/.minutes/graph/index.json`; those are retired legacy surfaces.
- Never create a replacement graph cache, spreadsheet, JSON file, or database.
- Never pass `--include-restricted`. Restricted meetings are intentionally absent from this agent-facing skill.
- Treat any authorization, resource-budget, correction-race, or projection error as a hard stop. Do not fall back to filesystem reads.

## Available commands

- `minutes people --json` — bounded relationship rankings and losing-touch signals.
- `minutes commitments --json` — bounded graph commitments.
- `minutes people merge <canonical> <alias...>` — confirm an identity correction in the local vocabulary; uncertain names are never merged automatically.
- `minutes person "<name>"` — bounded person profile.
- `minutes research "<topic>"` — bounded topic research.

## Workflow

1. Classify the request and use the narrowest command above.
2. Require exit status 0. Use only the bounded native result and never substitute filesystem reads.
3. For a proposed alias, show the suggestion and ask for confirmation before running `minutes people merge`; a wrong merge is worse than no merge.
4. Do not imply that restricted history or a relationship fact is absent when any command fails.

## Output

Return only the bounded native result. Never invent rankings, commitments, or relationship signals from raw files.

## Gotchas

- A failed person profile cannot be interpreted as “never met.” Report the source unavailable.
- Do not imply that a later sensitivity change proves a historical fact absent; report only the current authorized projection.
- Alias suggestions are evidence, not permission to rewrite identity. Require explicit confirmation before merging.
- Use `minutes research "<topic>"` for bounded company, product, or topic research. If it fails, report the source unavailable; never fall back to raw corpus reads.
