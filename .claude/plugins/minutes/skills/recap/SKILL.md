---
name: minutes-recap
description: Generate a daily digest of today's meetings and voice memos — key decisions, action items, and themes across all recordings. Use when the user asks "recap my day", "what happened in my meetings today", "daily summary", "what did I discuss today", "any action items from today", or wants a consolidated view of the day's conversations.
user_invocable: true
---

# /minutes recap

Synthesize all of today's meetings and voice memos into a single daily brief.

## How to generate the recap

1. **Get today's recordings:**
   ```bash
   minutes search "$(date +%Y-%m-%d)" --limit 50
   ```

2. **Read each meeting file** using `Read` on the paths returned

3. **Synthesize into a daily brief** with this structure:

```markdown
## Daily Recap — [date]

**[N] meetings, [M] voice memos**

### Key Decisions
- [Decision from meeting title]: [what was decided]

### Action Items
- [ ] @person: [task] (from: [meeting title])

### Topics Discussed
- [Topic 1] — discussed in [meeting 1], [meeting 2]
- [Topic 2] — raised in [voice memo title]

### Ideas Captured
- [Any voice memo insights worth surfacing]
```

4. Present the recap directly in the conversation — don't save it to a file unless asked.

## What makes a good recap

- **Cross-reference** across meetings: if pricing came up in two different calls, note that
- **Surface conflicts**: if Meeting A decided X but Meeting B discussed doing Y, flag it
- **Prioritize action items**: these are the things the user needs to act on
- **Include voice memos**: ideas captured on the go are easy to forget — surface them
- If there are no meetings or memos today, say so clearly rather than making something up
