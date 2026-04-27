---
name: Engineering Standup
slug: standup
version: 1.0.0
description: Engineering standup summary focused on yesterday, today, and blockers.
keywords: [standup, daily, engineering, scrum]
extends_base: true
additional_instructions: |
  Be concise. Blockers are the priority section. When summarizing ACTION ITEMS,
  prefer the engineering team's owners and call out cross-team dependencies
  explicitly so handoffs do not get buried in DECISIONS.
language: en
---

# Engineering Standup Template

Use this for daily engineering standups. Phase 1 ships the prompt-only
version: the structured extraction stays the same as the baseline `meeting`
template, but the summary leans on blockers and ownership.

## Coming in later phases

- `triggers.calendar_keywords: [standup, daily, scrum]` — auto-selection
  from calendar event titles (Phase 4).
- `extract:` schema with `yesterday`, `today`, `blockers.{technical,
  cross_team}` sub-fields surfaced into YAML frontmatter (Phase 2).
- `post_record_skill: minutes-standup-digest` — auto-invoked digest skill
  (Phase 3).

These fields are intentionally omitted from the Phase 1 frontmatter
because the loader rejects unknown fields. Phase 1 binaries advertising
support for them would be lying.
