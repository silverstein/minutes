# PocketStation capture

Minutes has two opt-in PocketStation integrations behind the
`pocketstation-capture` feature:

- On Windows and Linux, PocketStation records one desktop application without
  a virtual audio device. Minutes retains its existing microphone path.
- On macOS, Minutes retains its Core Audio process-tap backend for system audio
  and uses PocketStation to supervise the microphone in a dual-source recording.

In both cases the microphone and system/application audio remain independent
stems. A microphone failure does not terminate a healthy system-audio stem.

## Build Minutes

Enable the `pocketstation-capture` feature:

```bash
cargo build --release -p minutes-cli --features pocketstation-capture
```

The feature uses PocketStation 1.1.11 from crates.io and does not require a
PocketStation source checkout.

## Windows and Linux: choose an application

Add the following settings to `~/.config/minutes/config.toml`:

```toml
[recording]
capture_backend = "pocketstation"

[recording.sources]
voice = "default"
call = "Zoom"
```

`voice` selects the microphone. `call` selects the application audio and must
contain one of these values:

- an exact application name, such as `"Zoom"`;
- an application ID exposed by the operating system;
- a process ID written as `"pid:1234"`.

PocketStation rejects a name or ID that matches more than one application.
Process IDs are useful for automated tests but change whenever an application
starts. `call = "auto"` is not supported because recording the wrong
application would be difficult to notice.

Start a recording with the normal Minutes command:

```bash
minutes record
```

Minutes writes:

- `meeting.wav`, the mixed recording;
- `meeting.voice.wav`, the microphone;
- `meeting.system.wav`, the selected application.

Minutes writes all three files at 16 kHz and groups both live sources into
100 ms chunks. Each source begins with chunk zero when that source starts.

## macOS: supervise the microphone

Use Minutes' existing Core Audio process tap for system audio and select
`call = "auto"`:

```toml
[recording]
capture_backend = "core-audio-tap"

[recording.sources]
voice = "default"
call = "auto"
```

With `pocketstation-capture` enabled, this exact dual-source configuration uses
PocketStation for the microphone only. The process tap remains a separate
Minutes-owned system-audio backend; PocketStation does not replace it.

The microphone worker observes the opened native format, first and latest
frame activity, and caller-configured signal measurements away from the audio
callback. Minutes distinguishes these conditions:

- the source is waiting for its first frame;
- no frames arrived before the deadline;
- frames stopped arriving;
- frames are arriving but remain exact digital zero;
- the measured signal is below the configured peak/RMS thresholds;
- nonzero input remains below the separate near-silence observation envelope
  for the bounded warning window;
- useful signal was observed;
- samples were non-finite; or
- the source failed.

For a recoverable microphone failure, Minutes requests one bounded reopen of
the exact selected device. When the recording follows the system default and
that reopen fails, or the reopened device remains unhealthy, Minutes resolves
one explicit fallback device ID and asks PocketStation to replace the source.
It follows the current OS default only when that default resolves to a
different physical device. If the default still resolves to the failed
microphone, Minutes continues system-only instead of inventing an arbitrary
built-in fallback. The user must stop the recording, change or select the
microphone, and start a new recording. If the user explicitly selected a
microphone, Minutes never substitutes another physical device automatically:
after the exact retry it continues system-only with the same stop/change/start
guidance. This prevents a hardware-muted or intentionally selected headset
from being silently replaced by an active built-in microphone.

One fallback is attempted for default-following capture. If it also remains
unhealthy, Minutes continues in system-only degraded mode instead of looping
forever. If attaching a changed default fails, Minutes reports that attachment
failure directly. Terminal degraded mode does not live-rearm microphone
capture: recovery requires stopping this recording and starting a new one
after correcting the input. A replacement carries a new source generation and
discontinuity instead of pretending that two physical devices are one
uninterrupted source. While the microphone is unavailable, the system stem
continues and missing voice slots are represented as silence.

The near-silence observation envelope is deliberately much lower than the
ordinary signal threshold. It requires both peak at or below -70 dBFS and RMS
at or below -80 dBFS for 1.5 seconds. This covers #1057's measured -78.3 dBFS
peak and -91 dBFS mean level. Crossing that envelope produces a one-shot
persistent diagnostic and user warning; amplitude alone never authorizes
Minutes to reopen or replace a microphone.

After reopen or replacement, Minutes resets its activity/signal baseline and
accepts a signal window only when its source generation and discontinuity
match the current attachment. A useful window left over from the previous
microphone therefore cannot confirm recovery before the replacement delivers
a new frame.

This is host policy, not automatic Core policy: PocketStation supplies the
measurements, explicit reopen/replacement operations, lineage, and bounded
delivery; Minutes chooses when to retry or replace the source.

## Timing and evidence limits

The Windows/Linux application-capture path still does not measure a common
start time between the microphone and application capture, so a delayed source
is not backfilled from the other source's start time.

The macOS adapter persists PocketStation microphone lineage beside the voice
stem as `<recording>.voice.lineage.jsonl`, including source, stem and clock IDs,
source sequence bounds, missing-sequence counts, inserted-silence samples,
source timestamps, source generation, and discontinuity and permission epochs.
When sequence and timestamp evidence agree that canonical 10 ms frames are
missing, Minutes inserts the corresponding 16 kHz silence instead of
compressing the microphone timeline. Inconsistent or unbounded gaps fail the
microphone worker and enter the same bounded host recovery path.

The adapter does not yet correlate the PocketStation microphone clock with the
independent Core Audio process-tap clock. The mixer still aligns the two inputs
through Minutes' chunk positions, so this branch must not claim a measured
common microphone/system timeline or bounded cross-source drift.

Unit and synthetic integration tests cover classification, bounded queues,
cancel-before-join shutdown, source replacement lineage, and survival of the
system stem while the microphone is unavailable. No physical Logi HFP headset
or Teams call has been tested through this branch yet. Until that hardware
matrix passes, this is not proof that Minutes issue #1057 is fixed.

## Failure behavior

PocketStation delivers 10 ms audio frames to a dedicated Minutes worker. The
worker converts canonical 48 kHz audio to the 16 kHz format used by Minutes and
groups it into 100 ms chunks. PocketStation opens supported native microphone
formats and performs conversion/resampling before delivering those canonical
frames; Minutes exposes the actual opened format in its observations.
On each successful physical attachment, Minutes also writes one content-free
`pocketstation_microphone_opened_format` record to
`~/.minutes/logs/minutes.log` and prints the rate, channel count, and sample
representation to stderr. The record contains device/format and continuity
facts, never PCM samples.

Each PocketStation path uses a bounded 64-chunk output queue, which is 6.4
seconds at this format. If the recording loop cannot keep up, new chunks are
dropped and the total is written to the log. A stalled consumer cannot block
the capture callback. Stopping a recording cancels the polled-audio Session
before Minutes waits for the worker, so finalization does not drain pending
delivery. Worker shutdown is bounded to two seconds; a worker that does not
confirm cancellation is detached and reported instead of blocking recording
finalization indefinitely.

Application capture does not bypass microphone permissions, operating-system
audio permissions, or Minutes' recording consent settings.
