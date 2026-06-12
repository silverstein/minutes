# MLX Audio Engine Setup

Minutes supports MLX Audio as an opt-in local transcription engine on Apple
Silicon. It is designed for larger local ASR models that are expensive to load:
Minutes starts a small Python helper, loads the configured model once, and sends
JSONL requests for saved audio.

## Scope

`transcription.engine = "mlx-audio"` applies to saved-audio processing:
`minutes process`, desktop post-recording processing, and meeting/memo
processing.

Live transcript and dictation are intentionally out of scope for this first
backend. They can continue to use Whisper, Apple Speech, or Parakeet while
saved-audio processing uses MLX.

## Setup

The recommended setup command creates/uses `~/.minutes/mlx-audio`, installs
`mlx-audio`, checks that the selected model can load, and saves config:

```bash
minutes setup --mlx-audio
minutes setup --mlx-audio --mlx-audio-model mlx-community/Qwen3-ASR-1.7B-8bit
```

Manual setup is also possible:

```bash
python3 -m venv ~/.minutes/mlx-audio
~/.minutes/mlx-audio/bin/python -m pip install -U pip
~/.minutes/mlx-audio/bin/python -m pip install -U mlx-audio
```

If the model you want requires unreleased MLX Audio support, install from a
local clone or GitHub instead:

```bash
git clone https://github.com/Blaizzy/mlx-audio ~/src/mlx-audio
~/.minutes/mlx-audio/bin/python -m pip install -e ~/src/mlx-audio
```

Then configure Minutes:

```toml
[transcription]
engine = "mlx-audio"
mlx_audio_python = "/Users/you/.minutes/mlx-audio/bin/python"
mlx_audio_model = "mlx-community/Qwen3-ASR-1.7B-8bit"
mlx_audio_warm = true
mlx_audio_timeout_secs = 1800
mlx_audio_chunk_secs = 30.0
```

For desktop launches on macOS, prefer an absolute `mlx_audio_python` path. Apps
launched from Finder or Spotlight do not always inherit your shell `PATH`.
Desktop post-recording/meeting processing uses the same core config and does
not require a Settings UI change. Use **Settings → Advanced → Open config** to
review or edit MLX fields. The Settings engine dropdown may not list MLX yet.

## Timestamp Contract

Saved-audio meeting/memo transcripts need timed transcript segments so
downstream diarization can align speaker labels with transcript windows. The
MLX batch path therefore accepts only model outputs with real segment or
sentence timestamps.

If a model returns text without timestamps, Minutes fails the transcription
instead of inventing timing. Pick a timestamp-capable model or lower the chunk
duration if the model exposes chunk-level timestamps.

Live transcript and dictation do not use MLX in this phase.

## Warm Helper Behavior

`mlx_audio_warm = true` keeps one helper process resident in the current
Minutes process. That avoids repeated model loads for large local ASR models
during batch runs.

Set `mlx_audio_warm = false` to spawn a fresh helper per request. For tests or
debugging, `MINUTES_MLX_AUDIO_FORCE_ONESHOT=1` forces the one-shot path without
editing config.

## Real-Model Smoke Test

The unit tests use fake helpers and do not download model weights. To exercise
a real model:

```bash
MINUTES_MLX_AUDIO_E2E_AUDIO=/path/to/audio.wav \
MINUTES_MLX_AUDIO_E2E_PYTHON=/Users/you/.minutes/mlx-audio/bin/python \
MINUTES_MLX_AUDIO_E2E_MODEL=mlx-community/Qwen3-ASR-1.7B-8bit \
MINUTES_MLX_AUDIO_E2E_CHUNK_SECS=10 \
MINUTES_MLX_AUDIO_E2E_TIMEOUT_SECS=3600 \
cargo test -p minutes-core mlx_audio_real_model_e2e_when_env_is_set -- --nocapture
```

For a private reference run, keep the audio and VTT outside the repo and point
the env vars at local files, for example:

```bash
MINUTES_MLX_AUDIO_E2E_AUDIO=/path/to/reference-meeting.m4a \
MINUTES_MLX_AUDIO_E2E_PYTHON=/Users/you/.minutes/mlx-audio/bin/python \
MINUTES_MLX_AUDIO_E2E_MODEL=mlx-community/Qwen3-ASR-1.7B-8bit \
cargo test -p minutes-core mlx_audio_real_model_e2e_when_env_is_set -- --nocapture
```

Use a matching `.vtt` transcript as local reference material for a private
benchmark. The benchmark crops the audio in-process, compares the MLX
transcript against the matching VTT window, and prints WER, CER, and realtime
factor:

```bash
MINUTES_MLX_AUDIO_BENCH_AUDIO=/path/to/reference-meeting.m4a \
MINUTES_MLX_AUDIO_BENCH_REFERENCE_VTT=/path/to/reference-meeting.vtt \
MINUTES_MLX_AUDIO_BENCH_PYTHON=/Users/you/.minutes/mlx-audio/bin/python \
MINUTES_MLX_AUDIO_BENCH_MODEL=mlx-community/Qwen3-ASR-1.7B-8bit \
MINUTES_MLX_AUDIO_BENCH_START_SECS=2 \
MINUTES_MLX_AUDIO_BENCH_DURATION_SECS=20 \
MINUTES_MLX_AUDIO_BENCH_CHUNK_SECS=10 \
MINUTES_MLX_AUDIO_BENCH_TIMEOUT_SECS=3600 \
cargo test -p minutes-core mlx_audio_reference_benchmark_when_env_is_set -- --nocapture
```

Optional gates make the benchmark fail when a model regresses beyond an agreed
threshold:

```bash
MINUTES_MLX_AUDIO_BENCH_MAX_WER=0.25 \
MINUTES_MLX_AUDIO_BENCH_MAX_CER=0.15 \
MINUTES_MLX_AUDIO_BENCH_MAX_RTF=1.0 \
cargo test -p minutes-core mlx_audio_reference_benchmark_when_env_is_set -- --nocapture
```

Do not commit private meeting audio or transcripts to the upstream PR.
