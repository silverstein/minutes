# Interactive Skill Template

> A template for building multi-phase interactive Claude Code skills.
> Based on the patterns used in Minutes' `/minutes-prep`, `/minutes-debrief`, and `/minutes-weekly`.
> Inspired by [gstack](https://github.com/garrytan/gstack) `/office-hours` by Garry Tan.

## What makes an interactive skill different

Most Claude Code skills are reference docs — they tell Claude what commands to run. Interactive skills are **coaching experiences**: multi-phase flows that use `AskUserQuestion` to guide the user through a structured process, push back on vague answers, and produce a durable artifact at the end.

The difference:
- **Passive skill**: "Run `my-tool search <query>` to find results."
- **Interactive skill**: "Who are you looking for? ... Be specific — name one person. ... What do you need from them? ... Here's what I found, and here's what you should do next."

## Template

Copy this template and replace the placeholders.

```markdown
---
name: my-skill-name
description: |
  One-line description of what this skill does and when to trigger it.
  Include trigger phrases: "Use when the user says 'X', 'Y', or 'Z'."
user_invocable: true
---

# /my-skill-name

One paragraph explaining the skill's purpose and what it produces.

## How it works

This is a multi-phase interactive flow, not a single command.
Walk the user through each phase using AskUserQuestion, pushing back on vague answers.

### Phase 0: Auto-detect context (optional)

Before asking the user anything, check if context is available automatically:
- Check environment (calendar, git, files, MCP tools)
- If context found → pre-populate and skip Phase 1
- If not found → silently fall through to Phase 1

Rules:
- Never error if auto-detection fails — silently fall back to asking
- Best-effort, not required

### Phase 1: Gather the key input

Ask ONE question via AskUserQuestion. This is the most important input.

**If the answer is specific:**
→ Proceed to Phase 2 with the input.

**If the answer is vague:**
→ Push back with evidence or a sharper question. Don't accept the first vague answer.
   "Be specific. [Concrete example of what a good answer looks like]."

**If the user skips:**
→ Accept it gracefully. Adapt the flow, don't block.

### Phase 2: Research / analysis

Use the input from Phase 1 to search, analyze, or compute.
This phase is where the skill does the actual work.

Present findings to the user. Don't ask for approval — show the results
and move forward.

**If no data found:**
Say so honestly. Don't hallucinate. Adjust the remaining phases.

### Phase 3: Refine with the user

Ask a follow-up question that builds on the research.
Connect it to what you found — use evidence to push the user toward specificity.

This is where the coaching happens:
- "Based on [finding], which of these matters most to you?"
- "You mentioned [X] — is that still the priority?"
- "[Finding] suggests [insight]. Does that match your experience?"

### Phase 4: Produce the artifact

Save a durable file that downstream skills or future sessions can use.

```bash
mkdir -p ~/.my-tool/artifacts
```

Write a markdown file with YAML frontmatter:
```markdown
---
key_field: {from Phase 1}
date: {today ISO}
source: {what skill produced this}
---

## Section 1
{from Phase 2 research}

## Section 2
{from Phase 3 refinement}
```

Set permissions to 0600 if the content is sensitive.

### Phase 5: Closing ritual

End with three beats:

1. **Signal reflection** — Quote a specific thing the user said during the session.
   "You said '[exact quote]' — [why that matters]."

2. **Assignment** — One concrete real-world action. Not "go build it" —
   something specific the user can do in the next 30 minutes.

3. **Next skill nudge** — Point to the logical next step.
   "After [event], run `/my-next-skill` to [benefit]."

## Gotchas

- **Push back on vague answers** — This is THE pattern. Vague input = useless output.
- **Zero data is not an error** — Adjust the flow, don't apologize.
- **Don't hallucinate** — If search returned nothing, say so.
- **Artifacts are sensitive** — 0600 permissions on files with personal data.
- **Skill chaining** — If this skill produces a file, document how downstream skills find and use it.
```

## Design principles

### 1. Push back on vagueness

The most important pattern. Interactive skills are valuable BECAUSE they force specificity.

| User says | Bad response | Good response |
|---|---|---|
| "everyone" | "OK, preparing for everyone" | "Name one person who'll be there" |
| "just catch up" | "Here's a general agenda" | "Based on your last 3 meetings, [topic] keeps coming up. Is that what you want to focus on?" |
| "I'm not sure" | "OK, let me know when you're ready" | "Here's what I know from your history: [data]. Which of these matters most right now?" |

### 2. Use evidence from search

Interactive skills should search existing data and present it as evidence.
This is what makes them coaching experiences, not just questionnaires.

- "You've discussed pricing 5 times in 2 weeks. Something's on your mind."
- "Alex committed to this 3 meetings ago. No follow-up since."
- "This decision changed twice last month. It's still in flux."

### 3. Produce a durable artifact

Every interactive skill should save a file that:
- Has a predictable naming convention (so other skills can find it)
- Uses YAML frontmatter (so it's machine-parseable)
- Is useful standalone (readable markdown, not just structured data)
- Can be picked up by downstream skills (skill chaining)

### 4. Three-beat closing

End every session with:
1. **Mirror** — Show the user they were heard (quote their words)
2. **Move** — Give them one action to take right now
3. **Bridge** — Connect to the next skill in the lifecycle

### 5. Graceful degradation

Every phase should handle missing data:
- No calendar → ask manually
- No past meetings → first meeting, adjust flow
- No prep file → standalone debrief
- User skips a question → adapt, don't block

## Skill chaining pattern

Interactive skills can form a lifecycle by producing and consuming artifacts:

```
/prep  ──→ .prep.md file ──→ /debrief reads it
                                    │
                              compares outcomes to intentions
                                    │
                              .debrief.md file ──→ /weekly reads all
                                                        │
                                                  synthesizes the week
```

The convention:
- **Naming**: `~/.my-tool/artifacts/YYYY-MM-DD-{slug}.{type}.md`
- **Matching**: downstream skills match by date + entity (person name, project, topic)
- **Staleness**: define a TTL (e.g., 48 hours) after which upstream artifacts are ignored
- **Missing upstream**: always work standalone — the chain is a bonus, not a requirement

## Real examples

See these skills in the Minutes plugin for working implementations:
- [`/minutes-prep`](/.claude/plugins/minutes/skills/minutes-prep/SKILL.md) — 5 phases, calendar auto-detect + relationship brief
- [`/minutes-debrief`](/.claude/plugins/minutes/skills/minutes-debrief/SKILL.md) — prep comparison + decision evolution
- [`/minutes-weekly`](/.claude/plugins/minutes/skills/minutes-weekly/SKILL.md) — cross-meeting synthesis + forward planning
- [`/minutes-brief`](/.claude/plugins/minutes/skills/minutes-brief/SKILL.md) — fast hook-fireable briefing (v0.8.0)
- [`/minutes-mirror`](/.claude/plugins/minutes/skills/minutes-mirror/SKILL.md) — self-coaching from transcripts with bundled metric-counting script (v0.8.0)
