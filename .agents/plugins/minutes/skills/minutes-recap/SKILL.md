---
name: minutes-recap
description: Generate a daily digest of today's policy-authorized meetings and voice memos — key decisions, action items, and themes across available recordings. Use when the user asks "recap my day", "what happened in my meetings today", "daily summary", "what did I discuss today", "any action items from today", or wants a consolidated view of the day's conversations.
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


## Skill Path

Resolve this skill's installed SKILL.md path from the host's skill metadata.
Set MINUTES_SKILL_ROOT to that file's absolute parent directory and
MINUTES_SKILLS_ROOT to the absolute parent of MINUTES_SKILL_ROOT. Do this before
running a helper. Do not infer either path from the current working directory
or a Git checkout. The bundled runtime is under MINUTES_SKILLS_ROOT/_runtime.

# /minutes-recap

Synthesize today's policy-authorized normal meetings and voice memos into a
single daily brief. Restricted history is excluded from this agent workflow by
default and must never be treated as absent evidence.

## How to generate the recap

1. **Find today's recordings** from the bounded live list:
   ```bash
   minutes list --limit 50
   ```
   Require exit status 0, parse the JSON written to stdout, and select entries
   whose `date` is today. A date string is not a search query.

2. **Read each selected recording** with
   `minutes get "<exact path>" --json`. Require exit status 0 and use only the
   returned content. Never reopen list paths through the host filesystem.

3. **Synthesize into a daily brief** — use the template in `templates/daily-recap.md` as a starting point, adapting sections based on what actually exists in the day's recordings.

4. Present the recap directly in the conversation — don't save it to a file unless asked.

## What makes a good recap

- **Cross-reference** across meetings: if pricing came up in two different calls, note that
- **Surface conflicts**: if Meeting A decided X but Meeting B discussed doing Y, flag it
- **Prioritize action items**: these are the things the user needs to act on
- **Include voice memos**: ideas captured on the go are easy to forget — surface them
- If there are no meetings or memos today, say so clearly rather than making something up

## Interactive conflict detection

When you find conflicts between meetings (e.g., different decisions on the same topic, contradictory action items, or shifted priorities), don't just note them — ask the user about them.

Use AskUserQuestion: "I found a conflict between your meetings today: [Meeting A] decided [X], but [Meeting B] discussed doing [Y]. Which one is current?"

Options should include:
- The decision from Meeting A
- The decision from Meeting B
- "Neither — it's still unresolved"
- "Both are valid in different contexts"

This turns the recap from a passive report into an active reconciliation tool. Surface at most 2-3 conflicts per recap to avoid fatigue.

## Gotchas

- **No recordings today ≠ an error** — If there are no meetings or memos for today, say "No recordings found for today" and offer to search a different date range. Don't hallucinate a recap.
- **Voice memos are easy to miss** — They live in `~/meetings/memos/`, not the main `~/meetings/` directory. The search command includes both, but double-check if the user says "I recorded a voice memo today" and you don't see it.
- **Meetings without LLM summarization have less structure** — If a meeting file only has a raw transcript (no Summary, Decisions, or Action Items sections), you'll need to extract insights yourself from the transcript text. Check the YAML frontmatter for `action_items:` and `decisions:` fields.
- **Cross-day meetings** — A meeting that started at 11 PM and ended at 1 AM will be dated by its start time. If the user asks "what happened today" and is missing a late-night meeting, check yesterday's date too.
