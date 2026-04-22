---
name: minutes-verify
description: Verify that Minutes is properly set up and working — model downloaded, mic accessible, directories exist, no stale state. Use when the user says "is minutes working", "check my setup", "verify minutes", "test recording setup", "why isn't minutes working", "minutes health check", or after running setup for the first time.
triggers:
  - is minutes working
  - check my setup
  - verify minutes
  - test recording setup
  - why isn't minutes working
  - minutes health check
user_invocable: true
metadata:
  display_name: Minutes Verify
  short_description: Verify that Minutes is properly set up and working — model downloaded, mic accessible, directories exist, no stale state.
  default_prompt: Use Minutes Verify for this task.
  site_category: Capture
  site_example: /minutes-verify
  site_best_for: Health-check the install, models, mic, and stale state before trusting it in production.
assets:
  scripts:
    - scripts/verify-setup.sh
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-verify/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-verify/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-verify

Run a health check on the Minutes installation to confirm everything is working.

## How to verify

Run the verification script included with this skill:

```bash
bash "${CLAUDE_PLUGIN_ROOT}/skills/minutes-verify/scripts/verify-setup.sh"
```

The script checks each component and outputs a pass/fail status for each. Read the output and report results to the user.

## What gets checked

| Check | What it verifies |
|-------|-----------------|
| Binary | `minutes` command exists on PATH |
| Model | At least one whisper model downloaded in `~/.minutes/models/` or `~/.cache/whisper/` |
| Meetings dir | `~/meetings/` directory exists |
| Memos dir | `~/meetings/memos/` directory exists |
| PID state | No stale PID file in `~/.minutes/recording.pid` |
| Audio input | At least one audio input device available (macOS only) |
| Config | `~/.config/minutes/config.toml` exists (optional — defaults work fine) |

## After verification

If any checks fail, tell the user exactly what to do:

- **Binary missing** → `cargo build --release` in the minutes repo, then add to PATH
- **No model** → `minutes setup --model small` (recommended) or `--model tiny` (faster, lower quality)
- **No meetings dir** → `mkdir -p ~/meetings/memos` — will also be created on first recording
- **Stale PID** → `rm ~/.minutes/recording.pid` — previous recording crashed without cleanup
- **No audio input** → Check System Settings > Sound > Input, ensure a microphone is selected

## Gotchas

- **The script is macOS-specific** for the audio input check (uses `system_profiler`). On Linux, that check will be skipped.
- **"Model not found" is the #1 setup issue** — most people forget to run `minutes setup` after building.
- **Config file is optional** — if `~/.config/minutes/config.toml` doesn't exist, that's fine. Minutes uses compiled defaults. Only flag it as "not configured" (informational), not as an error.
