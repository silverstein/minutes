# parakeet-coreml

`parakeet-coreml` is a small Swift CLI helper that wraps the `FluidAudio` framework to run Parakeet CoreML speech-to-text on macOS.

## Requirements

- macOS 14 or newer
- A local Parakeet CoreML model directory
- Xcode / Swift toolchain with Swift Package Manager support

Default model lookup starts from `~/.minutes/models/parakeet-coreml/` and resolves the matching FluidAudio Parakeet model bundle from there.

## Build

```bash
cd helpers/parakeet-coreml
swift build -c release
```

From the repo root, build and install the helper into `~/.local/bin/parakeet-coreml`:

```bash
./scripts/build-parakeet-helper.sh
```

## Batch Mode

Transcribe a WAV file and print timestamped transcript lines:

```bash
./.build/release/parakeet-coreml --batch meeting.wav
./.build/release/parakeet-coreml --batch meeting.wav --model-dir ~/.minutes/models/parakeet-coreml --language en
```

Output format:

```text
[0.00 - 2.45] Hello, how are you doing today?
```

## Stream Mode

Run the helper as a JSON-lines process that keeps the model loaded:

```bash
./.build/release/parakeet-coreml --stream
```

Example commands:

```json
{"cmd":"audio","samples":[0.1,-0.2,0.05]}
{"cmd":"transcribe"}
{"cmd":"finalize"}
{"cmd":"reset"}
{"cmd":"quit"}
```

Protocol notes:

- `audio` appends 16 kHz mono PCM samples and does not emit a response on success.
- `transcribe` emits a partial response: `{"type":"partial","text":"...","duration_secs":1.23}`
- `finalize` emits a final response: `{"type":"final","text":"...","duration_secs":1.23}`
- `reset` clears buffered audio for the next utterance while keeping the loaded model in memory.
- `quit` exits the process.
- Malformed input emits an error response: `{"type":"error","message":"..."}`

Responses are written as JSON lines, and stdout is flushed after every emitted JSON line.
