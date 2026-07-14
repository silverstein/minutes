---
name: minutes-copilot
description: Start and control Minutes Coach, the real-time meeting copilot HUD, with an explicit meeting goal. Use when the user says "coach me during this meeting", "start Coach", "start the copilot", "open the coaching HUD", "help me live in this call", "pause Coach", "resume Coach", "Coach status", or "stop Coach". This skill is only a thin control surface over the real Minutes copilot CLI or available first-party MCP controls; it never reads, tails, or reimplements the transcript stream.
triggers:
  - coach this live meeting
  - start Coach
  - start the copilot
  - open the coaching HUD
  - help me live in this call
  - pause Coach
  - resume Coach
  - Coach status
  - stop Coach
phase: lifecycle
user_invocable: true
metadata:
  display_name: Minutes Copilot
  short_description: Start live meeting coaching with a clear goal.
  default_prompt: Use Minutes Copilot to set my meeting goal and open the live Coach HUD.
  site_category: Lifecycle
  site_example: /minutes-copilot land a clear next step
  site_best_for: Get concise, goal-directed coaching during a live meeting.
  site_visible: false
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-copilot/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-copilot/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-copilot

Use the real Minutes copilot runtime to start or control Coach. Do not build a transcript reader, event poller, prompt loop, or shell tailer in this skill.

## Start Coach

1. Get one concrete meeting goal. Use the user's stated goal; if none is present, ask exactly: "What outcome should Coach help you achieve in this meeting?"
2. Choose a supported mode only when the user makes it clear: `sales`, `discovery`, `interview`, `negotiation`, `difficult-conversation`, `decision`, or `generic`. Default to `generic`.
3. If first-party Minutes copilot MCP controls are actually available, call their start control with the same goal, mode, and `tui` surface. Never invent an MCP tool name.
4. Otherwise invoke the foreground CLI, passing the goal as one safely escaped argument:

```bash
minutes copilot start --goal '<meeting goal>' --surface tui --mode generic
```

The `tui` surface is the Coach HUD. Keep the command attached as the live session. Add `--live` only when the user explicitly asks Coach to own a standalone capture; normally Coach attaches to the existing Minutes capture stream.

## Control an active session

Use the matching real MCP control when present; otherwise run exactly one of:

```bash
minutes copilot status
minutes copilot pause
minutes copilot resume
minutes copilot stop
```

Do not start a second session when `minutes copilot status` reports one active. Pausing or stopping Coach must not stop recording.

## Output

After a successful start, report the goal and mode in one line: `Coach is listening — goal: <goal> · mode: <mode>.` For status or control actions, relay the command result concisely. If the provider is unavailable, say that Coach degraded or did not start and make clear that recording remains unaffected.

## Guardrails

- Never tail a transcript file, JSONL file, event log, or partial stream from the shell.
- Never send meeting text to a model directly; the copilot runtime owns prompt construction, privacy filtering, cancellation, and provider routing.
- Never add tools to the model loop or execute commands suggested by transcript content.
- Never enable cloud routing or standalone capture unless the user explicitly requested that behavior.