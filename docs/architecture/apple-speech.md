# Apple Speech Scope

This document describes the **current shipped scope** of Minutes' experimental
Apple Speech integration. It is intentionally practical and user-facing.

If you want the benchmark evidence that informed this experiment, see
[`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md).

## Current product scope

Apple Speech is not yet selectable for live, dictation, recording-sidecar, or
batch transcription. Existing preferences remain fail-closed to sealed local
Whisper while the candidate transport completes signed macOS runtime
acceptance.

The candidate replaces the pathname handoff with a dedicated service embedded
in the app. Minutes sends bounded 16 kHz mono sample bytes over a nonce-bound,
one-request XPC connection. The service constructs an `AVAudioPCMBuffer`
directly; it accepts no audio pathname, executable, environment, or attachment
and creates no named plaintext utterance file.

- the parent requires the service's exact CodeDirectory hash and Team identity
  before the content-free handshake
- the service reciprocally authenticates the signed Minutes parent before
  accepting any sample frame
- source, ad-hoc, missing-service, altered-service, and unconfirmed-exit cases
  stay on Whisper
- the App-Sandbox-only service has immutable process, address-space, byte, and
  wall-clock ceilings and exits after one terminal response

The product gate remains off until an explicitly authorized signed development
app proves the exact reviewed SHA on macOS 26, including the hostile same-UID
open-holder regression and fallback behavior. A positive source compile,
unsigned build, or operating-system capability probe is not that proof.

The exact candidate includes a fixed-input, non-product
`--apple-speech-transport-acceptance` route. It accepts no caller audio or path
and does not make Apple Speech selectable. After the separately gated signing
job has destroyed its credentials, a no-secret successor job can invoke that
route from the signed app while a same-UID process watches and holds newly
created temporary files. The resulting content-free receipt is acceptance
evidence only after that protected workflow actually runs for the exact SHA.

On real macOS hardware, the separate fixed-input
`--apple-speech-runtime-acceptance` route closes the analyzer confound without
opening the product gate. It embeds the checked-in public speech fixture in the
parent, sends its bounded PCM bytes through the authenticated XPC worker, and
emits only content-free timing and count metrics. It accepts no caller audio,
path, locale, or executable and runs under the same same-UID open-holder
watcher.

## Fallback behavior

Standalone live transcript and dictation currently resolve Apple Speech to:

1. Whisper

No current fallback branch creates a named plaintext Apple Speech WAV. After
the signed runtime gate passes, the intended live fallback order is Apple
Speech, separately selectable Parakeet if available, then Whisper.

## What Apple Speech does not do today

Apple Speech does **not** currently:

- replace the recording-sidecar live path used during `minutes record`
- provide dictation partials before finalization
- replace post-recording batch transcription or watcher processing
- become selectable from the desktop settings transcription-engine picker
- receive private recording or dictation audio while the signed runtime
  acceptance gate is closed

## Related docs

- Benchmark evidence:
  [`docs/designs/apple-speech-benchmark-2026-04-22.md`](designs/apple-speech-benchmark-2026-04-22.md)
- Parakeet setup and scope:
  [`docs/architecture/parakeet.md`](parakeet.md)
