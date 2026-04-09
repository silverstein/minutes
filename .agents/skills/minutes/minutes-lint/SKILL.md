---
name: minutes-lint
description: Health-check your meeting knowledge for contradictions, stale commitments, and decision conflicts. Use when the user asks "any conflicts in my meetings", "check for stale action items", "lint my meetings", "consistency check", "are there contradictions", or wants to audit their decision history.
---

# /minutes-lint

Run a consistency check across all meetings to find decision conflicts and stale commitments.

## How to run the lint

1. **Run the consistency check**:
   ```bash
   minutes consistency --stale-after-days 14
   ```

   Optional filters:
   - `--owner <name>` — limit to commitments assigned to a specific person
   - `--stale-after-days <N>` — change the staleness threshold (default: 7)

2. **Parse the JSON output** and present it as readable markdown.

## Formatting the report

### Decision Conflicts

For each conflict, show:

```
**Topic: {topic}**
- Latest: "{latest decision text}" — *{meeting title}* ({date})
- Prior: "{prior decision text}" — *{meeting title}* ({date})
- **Status**: These decisions may contradict each other.
```

### Stale Commitments

For each stale item, show:

```
- [ ] **@{who}**: {task} (due {due_date}, {age_days} days overdue)
  - Last discussed: *{meeting title}* ({date})
```

### Clean bill of health

If no conflicts and no stale commitments, say: "No decision conflicts or stale commitments found across your meetings. Everything looks consistent."

## When to suggest next steps

- If there are decision conflicts: suggest running `/minutes-debrief` on the most recent conflicting meeting, or `/minutes-search "{topic}"` to review the full decision history
- If there are stale commitments: suggest the user update the action item status in the meeting file, or bring it up in the next meeting with that person
- If the user wants to dig deeper into a specific person's commitments: suggest `minutes commitments --person "{name}"`

## Gotchas

- **The consistency check uses graph.db** — if it seems stale, suggest `minutes people --rebuild` to refresh the index
- **Stale != forgotten** — some action items are intentionally deferred. Don't alarm the user; present the data and let them decide
- **Decision conflicts are topic-based** — two meetings discussing "pricing" with different conclusions will flag, even if the later decision intentionally superseded the earlier one. Context matters.

