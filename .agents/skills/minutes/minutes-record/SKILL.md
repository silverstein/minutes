---
name: minutes-record
description: Start or stop recording a meeting, call, or voice memo. Use this whenever the user says "record", "start recording", "capture this meeting", "stop recording", "I'm in a meeting", "take notes on this call", or wants to transcribe live audio. Also use when they ask about recording status or want to know if something is being recorded.
---

## Skill Path

Before running helper scripts or opening bundled references, set:

```bash
export MINUTES_SKILLS_ROOT="$(git rev-parse --show-toplevel)/.agents/skills/minutes"
export MINUTES_SKILL_ROOT="$MINUTES_SKILLS_ROOT/minutes-record"
```

# /minutes-record

Capture a meeting, call or voice memo with the local Minutes engine and save a searchable record.

## Choose the connected capture path

Check `get_status` first when Minutes MCP tools are connected. Resolve the exact registered tool names from the host. If another capture already owns the microphone, report its state; stop it only when the user asks.

Use `start_recording` for a requested recording, with the title, capture mode and intent supported by its schema. For a call, use call intent so Minutes checks the system-audio route and delegates to the running desktop app where needed. Keep the call preflight intact. If it reports unavailable system audio, explain the missing route; do not silently permit microphone-only call capture.

Use `stop_recording` when the user asks to finish. The stop response may describe processing still in progress. Check `list_processing_jobs` or `get_status` before claiming the final transcript is ready. Report the resulting path or the actual failure.

The MCP server does not listen for spoken stop commands. The user must request stop in chat or use Minutes' native stop control. Do not promise that saying "stop recording" into a meeting will stop capture.

## Local CLI alternative

If no connected MCP capture tool is available and a local shell is on the same computer as Minutes:

```bash
minutes status
minutes record --title "Weekly standup with Alex"
```

`minutes record` runs in the foreground. Keep its owned process/session alive during capture. Use a separate local command to finish:

```bash
minutes stop
```

A successful command dispatch is not proof of active recording. Check status after startup, preserve the WAV on interruption, and report capture and transcription separately. Do not terminate an unrelated recording process or close its terminal.

## Call audio and permissions

The supported Minutes desktop app can capture Mac call audio through its native system-audio path. BlackHole is an optional configured virtual-device alternative, not a prerequisite for every Mac call. Follow `references/audio-devices.md` for native preflight and optional routing.

Microphone and system-audio permissions belong to the responsible local app/process. Installing an AI plugin does not grant them. If capture fails, identify the failed permission or route from the runtime result. Keep Input Monitoring and Accessibility separate from audio permissions; do not reset privacy settings or replace the installed app to troubleshoot.

## Live transcript

Use `read_live_transcript` when the current capture exposes a live stream, or inspect the local CLI:

```bash
minutes transcript --status
minutes transcript --since 42
```

Live text is provisional. If a live consumer fails, recording and WAV preservation must continue. Do not claim that starting recording necessarily starts every optional live consumer.

## Output and first capture setup

Minutes saves to the configured library, commonly `~/meetings/`, with title/date metadata and a transcript. Summary, decisions and action items depend on configured processing and must be described only when present. Local transcription keeps audio processing on the computer; excerpts returned through MCP become context for the AI host under its settings.

A missing speech model blocks capture/transcription, not every library-retrieval operation. Use the actual missing-model result and the setup skill. A supported Whisper starting point is:

```bash
minutes setup --model small
```

Do not automatically download a model or change engines while qualifying the connection. Respect restricted-meeting policies and private file permissions. For setup help, use https://github.com/silverstein/minutes/discussions.

