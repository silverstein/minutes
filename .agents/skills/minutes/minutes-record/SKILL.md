---
name: minutes-record
description: Start or stop recording a meeting, call, or voice memo. Use this whenever the user says "record", "start recording", "capture this meeting", "stop recording", "I'm in a meeting", "take notes on this call", or wants to transcribe live audio. Also use when they ask about recording status or want to know if something is being recorded.
---

# /minutes-record

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

**Live transcript during recording:**

While recording, Minutes streams a real-time transcript to `~/.minutes/live-transcript.jsonl`. You can read it with:
```bash
minutes transcript                    # all lines
minutes transcript --since 42         # lines after cursor
minutes transcript --since 5m         # last 5 minutes
minutes transcript --status           # check if active
```

This lets you follow what's being discussed mid-meeting. The live output is rougher than the final transcript produced after stop -- it prioritizes speed over accuracy.

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

## Gotchas

- **"model not found"** → Run `minutes setup --model small`. This is the most common first-run error.
- **"already recording"** → Run `minutes stop` first, or `minutes status` to check. If the PID file is stale (process crashed), `minutes stop` will clean it up.
- **No audio captured / empty transcript** → Check that the right input device is selected in System Settings > Sound. On MacBooks, the default mic works for in-person conversations but won't capture system audio.
- **For Zoom/Meet/Teams audio** → You need BlackHole to capture system audio. See `references/audio-devices.md` in this skill folder for the full setup guide.
- **Recording runs but transcription is garbage** → The `tiny` model is fast but low quality. Upgrade to `small` or `medium` for real meetings: `minutes setup --model small`.
- **"permission denied" on output file** → Output files are `0600` (owner-only). This is intentional — transcripts contain sensitive content. Don't chmod them to be world-readable.
- **Long meetings (>2 hours)** → Transcription time scales with duration. A 2-hour meeting with the `small` model takes ~3-5 minutes on Apple Silicon. The `tiny` model is ~4x faster but much less accurate.
- **Recording process disappeared** → If you close the terminal tab where `minutes record` is running, the recording stops but may not process. Always use `minutes stop` from another terminal.

