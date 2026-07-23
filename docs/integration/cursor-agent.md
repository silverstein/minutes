# Cursor Agent CLI support

Minutes supports the Cursor Agent CLI (`agent`) as an opt-in local agent for
non-interactive summarization.

## What is wired

- Desktop settings recognize `agent` as a well-known `agent_command` when the
  binary is on PATH (or in well-known install dirs such as `~/.local/bin`).
- `engine = "agent"` can run `agent` for post-meeting summarization, title
  refinement, and L1 speaker mapping.
- Cursor continues to work as a files + MCP host for interactive chat; this doc
  covers only the headless `agent_command` path.
- No separate Cursor skill tree — reuse raw `~/meetings/` files and/or the
  Minutes MCP server / portable `.agents/skills` mirror.

## Summarization config

```toml
[summarization]
engine = "agent"
agent_command = "agent"
```

Use an absolute path when the desktop app PATH is minimal:

```toml
agent_command = "/Users/you/.local/bin/agent"
```

Minutes runs Cursor Agent with:

```bash
agent -p --mode ask --output-format text --trust "<prompt>"
```

That invocation is intentionally narrow:

- `-p` / print mode for non-interactive stdout capture
- `--mode ask` for read-only Q&A (no write or shell tools as a side effect)
- `--trust` so headless runs do not block on an interactive workspace-trust prompt
  (same class of bypass as Codex `--skip-git-repo-check` / Gemini `--skip-trust`)
- prompt as a trailing positional argument (CLI contract)
- no `--force` / `--yolo`
- no hardcoded `--model` — model selection stays with your Cursor CLI / account
  (Pro/Ultra subscription or `CURSOR_API_KEY` auth as configured for the CLI)

Authenticate the CLI first (`agent login`, or set `CURSOR_API_KEY` for print
mode). Interactive IDE login alone may not be enough for headless `-p` runs.

## Screens / live coaching

Headless Cursor Agent summarization is text-only in this release (same posture
as gemini/opencode/pi). Screen-context delivery for `agent` is not wired yet.
