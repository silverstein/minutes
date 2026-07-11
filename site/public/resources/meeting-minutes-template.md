# Meeting minutes templates (markdown, copy-paste ready)

Last reviewed: 2026-07-11

Four deliberately short templates — the graveyard of meeting documentation is full of beautiful templates nobody filled in twice.

## Standard team meeting

```markdown
# Team Meeting — {date}
**Attendees:** {names}
**Facilitator:** {name}

## Agenda
1. {topic}

## Discussion
- {topic}: {key points, disagreements, context}

## Decisions
- {decision} — decided by {who}, because {why}

## Action items
- [ ] {task} — @{owner}, due {date}

## Parking lot
- {deferred topic}
```

## Board / formal meeting

```markdown
# {Organization} Board Meeting Minutes
**Date/Time:** {date, start–end}
**Location:** {place / video}
**Present:** {names, roles}  **Absent:** {names}  **Quorum:** {yes/no}

## Call to order
Called to order at {time} by {chair}.

## Approval of prior minutes
Minutes of {date} were {approved / amended}.

## Reports
- {Officer/Committee}: {summary}

## Motions
- MOTION: {text}. Moved {name}, seconded {name}.
  Vote: {for}–{against}–{abstain}. {Carried/Failed}.

## Adjournment
Adjourned at {time}. Next meeting: {date}.
Respectfully submitted, {secretary}
```

## Action-item-focused (standup / working session)

```markdown
# {Project} Working Session — {date}

## What changed since last time
- {update}

## Blockers
- {blocker} — needs {who/what}

## Action items
- [ ] {task} — @{owner}, due {date}

## Next checkpoint
{date} — success looks like: {criteria}
```

## 1:1 meeting

```markdown
# 1:1 — {name} & {name}, {date}

## Their agenda / My agenda
- {topics}

## Notes
- {what was actually said}

## Commitments
- [ ] {mine} — due {date}
- [ ] {theirs} — due {date}

## Follow up next time
- {thread to pull}
```

## What belongs in minutes

Minutes are not a transcript. They answer three future questions: what did we decide, who owes what by when, and why did we choose this over the alternative. Write decisions with reasons, action items with a single owner and a date, and nothing without one of those. The board template is the exception — it's a legal record: motions, votes, quorum, minimal discussion.

## Or stop filling in templates

Full disclosure: we build the tool that makes this page partly obsolete. Minutes (open source, free) records the meeting, transcribes on your device, and — once you connect an assistant (Claude via MCP, or a local LLM) — fills in this structure automatically: attendees, decisions, action items as structured YAML in markdown on your own disk. Templates still win for meetings you don't record and formal board minutes where a human secretary is the point.

https://github.com/silverstein/minutes
