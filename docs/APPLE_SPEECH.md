# Apple Speech Scope

This document describes the **current shipped scope** of Minutes' experimental
Apple Speech path. It is intentionally practical and user-facing.

If you want the benchmark evidence that informed this experiment, see
[`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md).

## Current product scope

As of the current `main` branch:

- `engine = "apple-speech"` is an **experimental standalone live-transcript
  path**.
- It applies to `minutes live`, not to `minutes record`, dictation, or
  post-recording / batch transcription.
- The desktop settings UI can surface Apple Speech availability, but it does
  **not** currently let you switch the transcription engine to Apple Speech
  from the settings picker.
- To use Apple Speech, configure it through the config file or CLI-driven
  flows instead of the desktop transcription-engine dropdown.

## Fallback behavior

If standalone live transcript is configured to use Apple Speech and Apple
Speech cannot run or fails mid-session, Minutes falls back in this order:

1. a ready Parakeet backend, if one is available
2. Whisper, if Parakeet is unavailable or also fails

That means Apple Speech is not a replacement for the rest of the transcription
stack. It is an experimental first-choice backend for standalone live mode
only, with the existing local engines still providing the safety net.

## What Apple Speech does not do today

Apple Speech does **not** currently:

- replace the recording-sidecar live path used during `minutes record`
- replace dictation (`minutes dictate` and the dictation hotkey)
- replace post-recording batch transcription or watcher processing
- become selectable from the desktop settings transcription-engine picker

## Related docs

- Benchmark evidence:
  [`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md)
- Parakeet setup and scope:
  [`docs/PARAKEET.md`](PARAKEET.md)
