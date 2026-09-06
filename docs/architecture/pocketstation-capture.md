# PocketStation capture on Windows and Linux

Minutes can use PocketStation to record one desktop application without a
virtual audio device. The microphone remains a separate source, so Minutes can
write the mixed recording and both original stems.

This integration is available on Windows and Linux and is off by default.
macOS recording continues to use Minutes' existing capture code.

## Build Minutes

Enable the `pocketstation-capture` feature:

```bash
cargo build --release -p minutes-cli --features pocketstation-capture
```

The feature uses PocketStation 1.1.10 from crates.io. It does not require a
PocketStation source checkout.

## Choose an application

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
This experimental backend does not yet measure a common start time between the
microphone and application capture, so a delayed application start is not
backfilled with silence from the microphone's start time.

## Failure behavior

PocketStation delivers 10 ms audio frames to a dedicated Minutes worker. The
worker converts 48 kHz application audio to the 16 kHz format used by Minutes
and groups it into 100 ms chunks.

Minutes keeps 64 call-audio chunks in memory, which is 6.4 seconds at this
format. If the recording loop cannot keep up, new application chunks are
dropped and the total is written to the log. Capture errors cancel the current
PocketStation Session; Minutes can then use its existing restart handling.
Stopping a recording also cancels this polled-audio Session before Minutes
waits for the capture worker, so finalization does not drain pending delivery.

Application capture does not bypass microphone permissions, operating-system
audio permissions, or Minutes' recording consent settings.
