# Apple Speech Scope

This document describes the **current shipped scope** of Minutes' experimental
Apple Speech path. It is intentionally practical and user-facing.

If you want the benchmark evidence that informed this experiment, see
[`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md).

## Current product scope

As of the current `main` branch:

- `engine = "apple-speech"` is an **experimental standalone live-transcript
  path**.
- It applies to `minutes live`.
- Dictation can opt into Apple Speech finalization with
  `[dictation] backend = "apple-speech"` on macOS when DictationTranscriber is
  available. Whisper still powers progressive partial text and remains the
  fallback.
- It does not apply to `minutes record` or post-recording / batch
  transcription.
- The desktop settings UI can surface Apple Speech availability, but it does
  **not** currently let you switch the main transcription engine to Apple
  Speech from the settings picker.
- To use Apple Speech, configure standalone live transcript or dictation
  through the config file / CLI-driven flows instead of the desktop
  transcription-engine dropdown.

## Fallback behavior

If standalone live transcript is configured to use Apple Speech and Apple
Speech cannot run or fails mid-session, Minutes falls back in this order:

1. a ready Parakeet backend, if one is available
2. Whisper, if Parakeet is unavailable or also fails

That means Apple Speech is not a replacement for the rest of the transcription
stack. It is an experimental first-choice backend for standalone live mode
and opt-in dictation finalization, with the existing local engines still
providing the safety net.

## What Apple Speech does not do today

Apple Speech does **not** currently:

- replace the recording-sidecar live path used during `minutes record`
- provide dictation partials before finalization
- replace post-recording batch transcription or watcher processing
- become selectable from the desktop settings transcription-engine picker

## Related docs

- Benchmark evidence:
  [`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md)
- Parakeet setup and scope:
  [`docs/architecture/parakeet.md`](parakeet.md)
