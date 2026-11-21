---
name: minutes-lint
description: Health-check your meeting knowledge for contradictions, stale commitments, and decision conflicts. Use when the user asks "any conflicts in my meetings", "check for stale action items", "lint my meetings", "consistency check", "are there contradictions", or wants to audit their decision history.
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


# /minutes-lint

Run a consistency check across policy-authorized normal meetings to find
decision conflicts and stale commitments. Restricted meetings are excluded from
this agent workflow by default and must never be described as absent evidence.

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

**Frontmatter v2: resolved supersessions.** When the `resolution` field is
present on a conflict, the newer decision has a `supersedes:` entry in its
frontmatter. Treat this as informational, not a red flag. Format as:

```
**Topic: {topic}** ✓ Resolved
- Current: "{latest decision text}" — *{meeting title}* ({date})
- Superseded: "{prior decision text}" — *{meeting title}* ({date})
- **Status**: {resolution text}
```

If the latest decision also carries an `authority` field (`high`/`medium`/`low`),
surface it next to the title. Authority is optional — when absent, omit the tag.

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
- Relationship-graph commitments come from `minutes commitments --json`; treat a nonzero exit as unavailable, never empty

## Gotchas

- **The consistency check reads live Markdown** — it uses stable source snapshots, not a durable graph cache. If a result looks wrong, inspect and correct the referenced meeting source; do not suggest deleting or rebuilding `graph.db` (it does not exist)
- **Stale != forgotten** — some action items are intentionally deferred. Don't alarm the user; present the data and let them decide
- **Decision conflicts are topic-based** — two meetings discussing "pricing" with different conclusions will flag, even if the later decision intentionally superseded the earlier one. Context matters.
