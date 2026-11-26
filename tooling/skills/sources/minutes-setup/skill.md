---
name: minutes-setup
description: Guided first-time setup for Minutes — download whisper model, create directories, configure audio input. Use when the user says "set up minutes", "install minutes", "first time setup", "configure minutes", "get started with minutes", "how do I start using minutes", or when verify shows missing components.
triggers:
  - set up minutes
  - install minutes
  - first time setup
  - configure minutes
  - get started with minutes
  - how do I start using minutes
user_invocable: true
metadata:
  display_name: Minutes Setup
  short_description: Guided first-time setup for Minutes — download whisper model, create directories, configure audio input.
  default_prompt: Use Minutes Setup for this task.
  site_category: Capture
  site_example: /minutes-setup
  site_best_for: Walk a first-time user through getting Minutes ready to record.
assets:
  scripts: []
  templates: []
  references: []
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-setup/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-setup/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-setup

Help the user reach the requested Minutes workflow using the components already installed.

## Check the connection first

If Minutes MCP is connected, call `get_status` and report the engine and current capture modes from its result. Otherwise use the verify skill's helper on the local Minutes computer:

```bash
bash "${CLAUDE_PLUGIN_ROOT}/skills/minutes-verify/scripts/verify-setup.sh"
```

Skip working components. Distinguish library retrieval, audio capture, optional model processing and destination insertion. A plugin installation or discovered tool does not establish that each of these is ready.

## Install a released engine if needed

On a supported Mac, the desktop package includes a bundled CLI:

```bash
brew install --cask silverstein/tap/minutes
```

For a standalone CLI instead:

```bash
brew install silverstein/tap/minutes
```

For other platforms, use https://useminutes.app or the repository's installation instructions. Do not assume a checkout at `~/Sites/minutes`, build from source by default, or replace an existing desktop app with an ad-hoc bundle. Development privacy testing uses the project's signed Minutes Dev identity.

## Configure the requested library

Use the existing configured library. Create or change a library only when requested. Search and sourced retrieval can use existing text records without installing a speech model. An OpenAI plugin uses the AI host's existing session; the separate ChatGPT-plan OAuth prototype is not required for plugin setup.

Explain the sharing boundary: records stay in the local library, while requested tool results become context for the AI host. Preserve restricted-meeting rules.

## Prepare audio capture when requested

If a speech model is missing, explain its download size and let the user choose. Whisper small is a reasonable starting point; use the runtime's exact missing-model guidance when another model is configured.

```bash
minutes setup --model small
```

For in-person capture, check the selected microphone. For Mac calls, use the running Minutes desktop app's native call/system-audio route and call preflight. Request the specific OS permission in the responsible app when needed. BlackHole is an optional virtual-device fallback for a deliberately configured route; see `minutes-record/references/audio-devices.md`.

Dictation insertion additionally requires its own destination permissions. Keep Microphone, system-audio capture, Input Monitoring and Accessibility distinct. A visible Settings toggle alone is not runtime permission evidence.

## Verify the requested operation

Start with a connection/status check. For retrieval, ask a question against an explicit sample or user-selected record and check its source. For capture, start a short test only when requested, confirm active state, stop through the connected tool or native control, then check the saved output and processing state.

Do not stop an existing meeting to run a setup test. Do not start microphone capture merely because the user asked to check the connection. Report remaining dependencies with a concrete next step. Setup questions belong at https://github.com/silverstein/minutes/discussions.
