---
name: minutes-verify
description: Verify that Minutes is properly set up and working — model downloaded, mic accessible, directories exist, no stale state. Use when the user says "is minutes working", "check my setup", "verify minutes", "test recording setup", "why isn't minutes working", "minutes health check", or after running setup for the first time.
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

# /minutes-verify

Run a health check on the Minutes installation to confirm everything is working.

## How to verify

Run the verification script included with this skill:

```bash
bash "$MINUTES_SKILL_ROOT/scripts/verify-setup.sh"
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
| Audio input | CLI-context audio input visibility (macOS only; desktop readiness is authoritative for the signed app) |
| Config | `~/.config/minutes/config.toml` exists (optional — defaults work fine) |
| Spotlight privacy | macOS `.metadata_never_index` markers exist for `~/.minutes` and `~/meetings` when those directories exist |

## After verification

If any checks fail, tell the user exactly what to do:

- **Binary missing** → `cargo build --release` in the minutes repo, then add to PATH
- **No model** → `minutes setup --model small` (recommended) or `--model tiny` (faster, lower quality)
- **No meetings dir** → `mkdir -p ~/meetings/memos` — will also be created on first recording
- **Stale PID** → `rm ~/.minutes/recording.pid` — previous recording crashed without cleanup
- **Audio input warning** → Treat as CLI-context-only. If the signed desktop app's Readiness Center shows Audio input/Microphone as ready, trust the desktop app identity.
- **Spotlight privacy warning** → Open the desktop app or run `minutes setup` so Minutes can recreate the `.metadata_never_index` markers.

## Gotchas

- **The script is macOS-specific** for the audio input check (uses `system_profiler`). On Linux, that check will be skipped. On macOS, shell-context checks can disagree with the signed desktop app; use the desktop Readiness Center for TCC-sensitive truth.
- **"Model not found" is the #1 setup issue** — most people forget to run `minutes setup` after building.
- **Config file is optional** — if `~/.config/minutes/config.toml` doesn't exist, that's fine. Minutes uses compiled defaults. Only flag it as "not configured" (informational), not as an error.
- **Spotlight privacy uses folder markers** — Minutes verifies `.metadata_never_index`; it does not disable Spotlight indexing for the whole user volume.
