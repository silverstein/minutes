---
name: minutes-note
description: Add a note to the current recording or annotate a past meeting. Use whenever the user says "note that", "remember this", "mark this as important", "add a note about", "annotate the meeting", or wants to capture a thought during or after a recording. Plain text input — no markdown needed.
user_invocable: true
---

# /minutes note

Add a timestamped note during a recording, or annotate a past meeting.

## During a recording

```bash
# Add a note to the active recording (auto-timestamped)
minutes note "Alex wants monthly billing not annual billing"
minutes note "Case agreed — compromise at monthly billing for experiment"
```

Each note gets a timestamp matching the recording position (e.g., `[4:23]`). Notes feed into the LLM summarizer as high-priority context — the AI knows what you thought was important and weights those parts of the transcript more heavily in the summary.

## After a meeting

```bash
# Annotate an existing meeting file
minutes note "Follow-up: Alex confirmed via email on Mar 18" --meeting ~/meetings/2026-03-17-pricing-call.md
```

Appends to the `## Notes` section of the meeting file with a date stamp.

## Tips

- Notes are plain text — just type what you're thinking, no formatting needed
- Short notes work best: "pricing pushback" > "Alex expressed concerns about the current pricing structure and suggested..."
- Notes are searchable via `minutes search`
