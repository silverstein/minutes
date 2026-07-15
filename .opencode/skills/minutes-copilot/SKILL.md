---
name: minutes-copilot
description: Start and control Minutes Coach, the separate real-time copilot HUD, with an explicit meeting goal. Use only for explicit Coach or HUD lifecycle requests such as "start Minutes Coach", "open the Coach HUD", "pause Minutes Coach", "resume Minutes Coach", "Minutes Coach status", or "stop Minutes Coach". Do not use for requests that explicitly ask the current terminal agent to watch or strategize; those belong to minutes-live-sidekick. An ambiguous request such as "coach me live" requires one short surface clarification and must not automatically start Coach.
compatibility: opencode
---

# /minutes-copilot

Use the real Minutes copilot runtime to start or control Coach. Do not build a transcript reader, event poller, prompt loop, or shell tailer in this skill.

If the user explicitly asks **you or the current terminal agent** to watch, advise, or strategize, use `/minutes-live-sidekick` instead. If the user says only "coach me live" or otherwise leaves the surface ambiguous, ask exactly: "Do you want me in this terminal to be your sidekick, or should I open the Minutes Coach HUD?" Do not start Coach while clarifying.

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
