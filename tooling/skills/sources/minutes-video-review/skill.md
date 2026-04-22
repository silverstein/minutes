---
name: minutes-video-review
description: Analyze a product walkthrough, bug report video, Loom, or ScreenPal using Minutes transcription plus visual review. Use when the user wants a recorded demo or bug clip turned into a durable brief with transcript, key frames, issues, and next steps.
triggers:
  - analyze this video
  - review this video
  - review this walkthrough
  - review this bug report video
  - summarize this Loom
  - summarize this ScreenPal
  - video intel
user_invocable: true
metadata:
  display_name: Minutes Video Review
  short_description: Review a demo, walkthrough, or bug video into a durable brief.
  default_prompt: Use Minutes Video Review to analyze this recorded video and return a transcript plus actionable brief.
  site_category: Artifacts
  site_example: /minutes-video-review https://go.screenpal.com/watch/...
  site_best_for: Turn a Loom, ScreenPal, or local walkthrough video into a durable artifact bundle for agent review.
assets:
  scripts:
    - scripts/video_review.py
  templates: []
  references:
    - references/dependencies.md
    - references/output-schema.md
output:
  claude:
    path: .claude/plugins/minutes/skills/minutes-video-review/SKILL.md
  codex:
    path: .agents/skills/minutes/minutes-video-review/SKILL.md
tests:
  golden: true
  lint_commands: true
---

# /minutes-video-review

Analyze a product walkthrough, bug report video, Loom, ScreenPal, or local recording into a durable artifact bundle that agents can keep working from.

This skill is for **meeting-adjacent product artifacts**, not for generic "understand any video" requests. Use it when the user wants a recorded demo, bug repro, or walkthrough turned into something actionable for engineering, product, support, or follow-up agent work.

## What this skill does

The bundled script handles the deterministic pipeline:

- resolve a local file or hosted video URL
- download hosted video when needed
- extract audio with `ffmpeg`
- transcribe with Minutes first, using the user's existing Minutes transcription setup
- sample key frames with adaptive caps so long videos do not blow up context
- write a durable artifact bundle under `~/.minutes/video-reviews/`

Then **you** review the resulting artifacts and return the actual user-facing brief.

## Primary command

Local file:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-video-review/scripts/video_review.py" \
  "/absolute/path/to/video.mp4"
```

Hosted video:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-video-review/scripts/video_review.py" \
  "https://go.screenpal.com/watch/..."
```

Useful options:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/skills/minutes-video-review/scripts/video_review.py" \
  "https://www.loom.com/share/..." \
  --focus "customer signup bug repro" \
  --cookies-from-browser chrome \
  --env-file /absolute/path/to/.env \
  --frame-step 15 \
  --max-frames 36 \
  --keep-temp
```

## How to use it

### Phase 1: Run the pipeline

Run the script on the provided local file or hosted video URL.

The script prints JSON with the output artifact paths. Important outputs include:

- `analysis_md`
- `analysis_json`
- `transcript_md`
- `metadata_json`
- `frames_dir`
- `contact_sheet_artifact`

### Phase 2: Inspect the artifacts

Read the generated `analysis.md` and `analysis.json` first.

Then inspect:

- `transcript.md` for the actual spoken content
- selected images from `frames/` when visual state matters
- `contact-sheet.jpg` for a quick visual sweep across sampled frames
- `metadata.json` for transcript method, duration, source kind, and frame sampling details

### Phase 3: Produce the real brief

Return a concise, useful brief to the user that includes:

- what the video is trying to show
- likely bug / proposal / walkthrough intent
- key moments or timestamps
- likely impacted area or flow
- the clearest next actions

Do not just echo the generated markdown blindly. Use the artifacts as evidence and produce a thoughtful agent answer.

## Minutes-first transcription rules

This skill should prefer transcript backends in this order:

1. hosted captions / VTT when the source exposes them
2. `minutes process` with an isolated temporary config
3. local `whisper` CLI if available
4. OpenAI audio transcription only as a last resort when configured

Important:

- the Minutes path should use the user's current Minutes transcription setup
- if Minutes is configured for Whisper, use Whisper
- if Minutes is configured for Parakeet, use Parakeet
- do not silently fork a separate transcription stack unless the Minutes path is unavailable

When reporting the artifacts back to the user, preserve the transcript method exactly. Prefer labels like:

- `vtt_captions`
- `minutes-whisper`
- `minutes-parakeet`
- `minutes-whisper-fallback`
- `local_whisper_cli`
- `openai_audio_transcription`

## Context discipline

This skill must stay disciplined about context size.

- Do not send the full video itself to the reasoning layer.
- Do not dump a long transcript and dozens of frames into the final answer.
- Treat the transcript as the backbone and frames as supporting evidence.
- Prefer inspecting a curated subset of frames instead of every sampled image.

The bundled script already caps frames adaptively, but you should still exercise judgment when deciding what to read or mention.

## Output contract

The script writes a durable bundle under:

```bash
~/.minutes/video-reviews/<timestamp>-<slug>/
```

Expected files:

- `analysis.md`
- `analysis.json`
- `transcript.md`
- `metadata.json`
- `frames/`

These artifacts are **not** part of the normal `~/meetings/` corpus by default.

## Dependencies

See:

- `${CLAUDE_PLUGIN_ROOT}/skills/minutes-video-review/references/dependencies.md`
- `${CLAUDE_PLUGIN_ROOT}/skills/minutes-video-review/references/output-schema.md`

## Gotchas

- **Hosted URLs need `yt-dlp`.** Local file review still works without it.
- **Frame caps are intentional.** The script samples enough evidence to review the video without turning this into a generic video-intelligence pipeline.
- **Minutes artifacts stay isolated.** The script uses a temp config/output path for the Minutes transcription run so it does not pollute the user's normal archive.
- **Model-powered auto-analysis is optional.** The generated `analysis.md/json` may be heuristic when no multimodal provider key is available. You still need to read the artifacts and produce the final answer.
- **Long videos need synthesis, not brute force.** If the transcript is long, work from the generated artifacts and only open the most relevant frames and transcript sections.
