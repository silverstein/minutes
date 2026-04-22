# Dependencies

`minutes-video-review` is designed to degrade gracefully, but these tools improve the experience:

## Required for full workflow

- `ffmpeg`

Use it for:

- audio extraction
- frame sampling
- media normalization

Install:

```bash
brew install ffmpeg
```

## Required for hosted video URLs

- `yt-dlp`

Use it for:

- Loom share URLs
- ScreenPal watch URLs
- other hosted video URLs that `yt-dlp` can resolve

Install:

```bash
brew install yt-dlp
```

Without `yt-dlp`, the skill still works for local video files.

## Optional transcript fallbacks

- `minutes` CLI on `PATH`
- `whisper` CLI on `PATH`
- `OPENAI_API_KEY` plus OpenAI CLI access

Preferred transcript order:

1. hosted captions / VTT
2. Minutes CLI
3. local Whisper CLI
4. OpenAI audio transcription

## Optional environment file

You can pass:

```bash
--env-file /absolute/path/to/.env
```

This is useful when the review depends on project-local credentials such as hosted model API keys.

## Output location

By default the skill writes to:

```bash
~/.minutes/video-reviews/
```

These artifacts are intentionally separate from `~/meetings/`.

## Evaluation loop

To re-run the current quality loop against a real example:

```bash
python3 "$MINUTES_SKILL_ROOT/scripts/eval_video_review.py" \
  --source "https://go.screenpal.com/watch/cOfrr5nOUj8" \
  --scenario bug \
  --focus "video review eval"
```

Scenarios currently supported:

- `bug`
- `demo`
- `culture`

This harness is intentionally lightweight. It checks whether the bundle shape and analysis are good enough for the current scenario rather than pretending to be a full benchmark suite.
