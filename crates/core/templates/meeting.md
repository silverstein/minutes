---
name: Meeting
slug: meeting
version: 1.0.0
description: Generic meeting summary. Used as the default fallback when no other template matches.
keywords: [meeting, default, generic]
extends_base: true
---

# Meeting Template

The default Minutes template. Produces the baseline structured extraction
(KEY POINTS, DECISIONS, ACTION ITEMS, OPEN QUESTIONS, COMMITMENTS,
PARTICIPANTS) without any domain-specific guidance.

If you do not pass `--template` when recording or processing, this template
is selected.

## Phase awareness

- Phase 1 fields only: `name`, `slug`, `version`, `description`, `keywords`,
  `extends_base`, `additional_instructions`, `language`.
- Future phases will add `triggers`, `extract`, `compliance`,
  `agent_context`, `post_record_skill`, and `extends`. Those fields will be
  rejected by Phase 1 binaries.
