# Apple Speech Scope

This document describes the **current shipped scope** of Minutes' experimental
Apple Speech integration. It is intentionally practical and user-facing.

If you want the benchmark evidence that informed this experiment, see
[`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md).

## Current product scope

As of the current `main` branch, Apple Speech is not selectable for live,
dictation, recording-sidecar, or batch transcription. Its helper accepts an
audio pathname, which would require Minutes to create a named plaintext WAV
outside the sealed private-audio capability. Minutes fails closed instead:

- existing `engine = "apple-speech"`, `[live_transcript] backend =
  "apple-speech"`, and `[dictation] backend = "apple-speech"` preferences are
  retained so configuration history is not destroyed
- every such preference resolves to sealed local Whisper
- capability probes and benchmark tooling remain available for development,
  but a positive operating-system capability probe does not make the backend
  selectable
- the desktop UI reports the retained preference and actual Whisper backend
  honestly

Apple Speech can become selectable only after its helper accepts an exact byte
stream or another transport proves equivalent isolation without a named
plaintext staging file. This follow-up is tracked locally as `minutes-hueo`.

## Fallback behavior

If standalone live transcript or dictation retains an Apple Speech preference,
Minutes resolves it before capture starts in this order:

1. Whisper

Existing Parakeet preferences behave the same way: retained, but resolved to
Whisper until secure byte transport lands. No current fallback branch creates a
named plaintext WAV for either helper.

## What Apple Speech does not do today

Apple Speech does **not** currently:

- replace the recording-sidecar live path used during `minutes record`
- provide dictation partials before finalization
- replace post-recording batch transcription or watcher processing
- become selectable from the desktop settings transcription-engine picker
- receive any private recording or dictation audio

## Related docs

- Benchmark evidence:
  [`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md)
- Parakeet setup and scope:
  [`docs/architecture/parakeet.md`](parakeet.md)
