---
name: minutes-record
description: Start or stop recording a meeting, call, or voice memo. Use this whenever the user says "record", "start recording", "capture this meeting", "stop recording", "I'm in a meeting", "take notes on this call", or wants to transcribe live audio. Also use when they ask about recording status or want to know if something is being recorded.
user_invocable: true
---

# /minutes record

Record audio from the microphone, transcribe it locally with whisper.cpp, and save as searchable markdown.

## How it works

Recording is a two-step process — start and stop. Between those two commands, audio is captured continuously from the default input device.

**Start recording:**
```bash
minutes record
# Or with a title:
minutes record --title "Weekly standup with Alex"
```

The process runs in the foreground. It captures audio from whatever input device is active — the built-in MacBook mic for in-person conversations, or a BlackHole virtual audio device for system audio (Zoom, Meet, Teams calls).

**Stop recording:**
```bash
minutes stop
```
This sends a signal to the recording process, which then:
1. Stops audio capture
2. Transcribes the audio locally via whisper.cpp (no cloud, no data leaves the machine)
3. Saves the transcript as a markdown file in `~/meetings/`
4. Prints the output path and word count as JSON

**Check status:**
```bash
minutes status
```
Returns JSON: `{"recording": true, "pid": 12345}` or `{"recording": false}`

## What you get

A markdown file at `~/meetings/YYYY-MM-DD-title.md` with:
- YAML frontmatter (title, date, duration, type)
- Timestamped transcript
- Summary, decisions, and action items (if LLM summarization is configured)

File permissions are set to 0600 (owner-only) because transcripts contain sensitive content.

## First-time setup

If the user hasn't set up minutes before, they need to download a whisper model first:
```bash
minutes setup --model small
```
This downloads a ~466MB model. For faster but lower quality: `--model tiny` (75MB). For best quality: `--model large-v3` (3.1GB).

## When things go wrong

- **"model not found"** → Run `minutes setup --model small`
- **"already recording"** → Run `minutes stop` first, or `minutes status` to check
- **No audio captured** → Check that the right input device is selected in System Settings > Sound
- **For Zoom/Meet audio** → Install BlackHole (`brew install blackhole-2ch`) and set up a Multi-Output Device in Audio MIDI Setup
