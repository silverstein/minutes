# Parakeet Warm Server Sidecar Design — 2026-04-14

## Question

How should Minutes wire the upstream `parakeet.cpp` warm `examples/server`
daemon into the existing Parakeet transcription path so we remove per-request
model reload cost, preserve the current cleanup pipeline, and degrade safely
back to subprocess transcription when the sidecar is unavailable?

This document is phase 1 only: design and scaffolding decisions for review
before runtime code lands.

## Constraints

The implementation must satisfy all of these at once:

- Keep work scoped to the Parakeet offline transcription path in
  `crates/core/src/transcribe.rs`.
- Do not modify upstream `parakeet.cpp`.
- Keep the feature opt-in with `transcription.parakeet_sidecar_enabled = false`
  by default.
- Preserve the existing subprocess path as the safety net.
- Cleanly shut down the sidecar on normal app exit.
- Avoid global socket collisions across multiple Minutes processes.
- Handle the known fp16 warm-server crash on Apple Silicon gracefully.
- Reuse Minutes' existing transcript cleanup pipeline instead of introducing a
  second formatting path.
- Avoid changing unrelated paths such as Whisper, live transcription, or
  Parakeet batch mode.

## Current State

Today, `transcribe_with_parakeet` does the following:

1. Loads and normalizes audio.
2. Writes a temporary 16 kHz mono WAV.
3. Resolves model, vocab, and optional native VAD assets.
4. Shells out to either:
   - the `minutes parakeet-helper` wrapper, or
   - the raw `parakeet` CLI directly.
5. Parses CLI timestamp output into `ParakeetCliTranscript`.
6. Runs the shared cleanup pipeline and returns the final `[M:SS] text` format.

That means every request pays process startup plus model load/cast cost.

The upstream server example added in `Frikallo/parakeet.cpp` PR #19 is a better
fit for repeated offline transcriptions:

- one loaded model per process
- Unix domain socket
- one JSON request per line
- one JSON response per line
- single-threaded request handling
- stderr carries operational logs

Important protocol note: the server response shape is not identical to
Minutes' current subprocess parser. The server returns `text` plus optional
`word_timestamps`, while Minutes currently normalizes CLI output into
`ParakeetCliTranscript { raw_output, segments, transcript }`.

That adapter layer is the main non-trivial integration seam.

## Goals

- Remove per-request model reload when multiple Parakeet transcriptions happen
  inside one Minutes process.
- Keep the transport swap local to the Parakeet entrypoint.
- Make sidecar failures boring: log, fallback, continue.
- Preserve transcript formatting and cleanup behavior.

## Non-Goals

- No change to upstream `parakeet.cpp` server protocol.
- No cross-process shared daemon. This is a per-Minutes-process child.
- No change to `transcribe_parakeet_batch` in the first slice.
- No attempt to fix the upstream fp16 MPSGraph crash.
- No enable-by-default rollout in this change.

## Proposed Architecture

Add a new core module:

- `crates/core/src/parakeet_sidecar.rs`

Primary type:

- `ParakeetSidecarManager`

Responsibilities:

- Resolve the warm server binary.
- Spawn one per-process `example-server` child lazily.
- Own the socket path, child handle, and recent stderr ring buffer.
- Perform startup health checks.
- Send one request over a Unix socket and read one newline-delimited response.
- Restart or fallback when the child dies or the protocol breaks.
- Shut down the child on normal process exit.

### Component Diagram

```text
Minutes caller
  |
  v
transcribe_with_parakeet(...)
  |
  +--> build temp WAV + resolve model/vocab/VAD + boost phrases
  |
  +--> if sidecar disabled/unavailable
  |      |
  |      v
  |    existing subprocess path
  |
  +--> ParakeetSidecarManager::transcribe(request)
         |
         +--> ensure_started()
         |      |
         |      +--> spawn example-server child
         |      +--> drain stderr into tracing + ring buffer
         |      +--> health check via tiny silent WAV request
         |
         +--> connect UnixStream(socket)
         +--> write one JSON line
         +--> read until newline
         +--> parse JSON response
         +--> adapt word_timestamps -> ParakeetCliTranscript
         |
         +--> on failure: restart once or fallback to subprocess
```

## Binary Resolution Decision

User-facing config added in this slice:

- `transcription.parakeet_sidecar_enabled: bool` (default `false`)

No second user-facing config knob in phase 1.

Server binary resolution order:

1. `MINUTES_PARAKEET_SERVER_BINARY` environment override, if set
2. `example-server` on `PATH`
3. If `parakeet_binary` is an absolute path, probe a sibling
   `example-server` binary in the same directory

If none resolve, the manager does not start and Minutes falls back to the
existing subprocess path with a warning.

Why this choice:

- It honors the requested narrow config surface.
- It keeps beta setup flexible on Mat's machine.
- It avoids hard-coding another path into config before the workflow settles.

## Socket Path Decision

Use a per-process socket path under the Minutes runtime area:

- `~/.minutes/tmp/parakeet-sidecar-<pid>-<model>.sock`

Why:

- avoids cross-process collisions between CLI, queue worker, and Tauri
- gives each process clear ownership of its child
- keeps the path short enough for `sockaddr_un`
- makes stale socket cleanup easy on next startup

The manager removes any leftover socket file before `bind`-time child launch
and again after confirmed shutdown.

## Protocol Decision

Stick with the upstream example-server's line-delimited JSON protocol exactly.
Minutes should not invent a parallel protocol in the first slice.

### Request Shape

Minutes sends one JSON object per line with these fields:

```json
{
  "request_id": "uuid-or-counter",
  "audio_path": "/abs/path/to/temp.wav",
  "decoder": "tdt",
  "timestamps": true,
  "use_vad": true,
  "beam_width": 8,
  "lm_path": "",
  "lm_weight": 0.5,
  "boost_phrases": ["Minutes", "Parakeet"],
  "boost_score": 2.0
}
```

Field mapping decision:

- `decoder`: always `"tdt"` for `tdt-600m`; preserve existing model-specific
  behavior rather than exposing new modes here
- `timestamps`: always `true` so the response contains `word_timestamps`
- `use_vad`: `true` only when Minutes resolved native Silero VAD at startup
- `beam_width`, `lm_path`, `lm_weight`: keep the upstream shape even if
  Minutes leaves them at defaults for now
- `boost_phrases` and `boost_score`: reuse the current Minutes graph boost
  phrase logic

### Response Shape

Minutes accepts:

```json
{
  "ok": true,
  "request_id": "same-id",
  "text": "...",
  "elapsed_ms": 812,
  "word_timestamps": [
    {"word":"hello","start":0.0,"end":0.4,"confidence":0.98}
  ]
}
```

And on failure:

```json
{
  "ok": false,
  "request_id": "same-id",
  "error": "message"
}
```

Minutes will ignore unknown response keys so upstream can add fields later
without breaking the client.

## Transcript Adapter Decision

The sidecar path should not bypass Minutes' cleanup pipeline.

Instead, the client will adapt the server response into the existing internal
shape:

1. Parse `word_timestamps`.
2. Convert each word timestamp into a temporary `ParakeetCliSegment`.
3. Run `crate::parakeet::group_word_segments(...)` to rebuild sentence-like
   segments exactly the way the subprocess parser already does.
4. Build `ParakeetCliTranscript { raw_output, segments, transcript }`.
5. Hand that into the existing `transcribe_result_from_parakeet_parsed(...)`
   path.

This keeps:

- `[M:SS] text` formatting
- dedup / script filtering / noise cleanup
- transcript diagnostics

all shared between subprocess and sidecar modes.

## Lifecycle Model

```text
Disabled
  |
  v
Cold
  |
  +-- first request --> Starting(fp16? from config)
                           |
                           +-- health check passes --> Healthy
                           |
                           +-- fp16 crash signature in <=60s --> RestartingFp32
                           |                                     |
                           |                                     +--> HealthyFp32
                           |
                           +-- spawn/health failure --> SubprocessOnly
Healthy / HealthyFp32
  |
  +-- request succeeds --> Healthy / HealthyFp32
  |
  +-- socket/protocol/child failure --> Restarting
                                         |
                                         +-- restart succeeds --> Healthy
                                         +-- restart fails --> SubprocessOnly
SubprocessOnly
  |
  +-- all requests use existing subprocess path for remainder of process
Stopping
  |
  +-- SIGTERM child, wait, SIGKILL if needed --> Cold
```

### Internal State Enum

Planned internal state:

```rust
enum SidecarState {
    Disabled,
    Cold,
    Starting { fp16: bool, started_at: Instant },
    Healthy(RunningSidecar),
    SubprocessOnly { reason: String, since: Instant },
    Stopping,
}
```

`RunningSidecar` will carry:

- `std::process::Child`
- socket path
- launch mode (`fp16` vs downgraded `fp32`)
- recent stderr lines for crash diagnosis
- startup timestamp

## Health Check Decision

The upstream server has no protocol-level `ping`.

So the health check will send a real but tiny request using a generated silent
WAV under `~/.minutes/tmp`, for example:

- 250 ms of 16 kHz mono silence
- `request_id = "__minutes_healthcheck__"`
- `timestamps = false`
- `use_vad = false`

Success criteria:

- connection to the Unix socket succeeds
- one newline-delimited JSON response is received within timeout
- response `ok == true`

Why use a real transcription request:

- it validates that the server is not merely listening, but that the model is
  loaded and able to answer requests
- it does not require changing the upstream protocol

## Timeout and Retry Policy

Initial constants for implementation:

- startup spawn + health timeout: `30s`
- socket connect timeout: `3s`
- per-request read timeout: `max(120s, 2 * audio_duration_secs)` capped at `30m`
- graceful shutdown wait after `SIGTERM`: `5s`
- one automatic restart attempt after a live-sidecar request failure

The request client will handle:

- partial reads by buffering until newline
- short writes by looping until the entire line is written
- reconnect-on-failure by closing the failed socket, ensuring the child is
  still alive, and retrying once after restart if needed

Request connections are short-lived:

- one `UnixStream` per transcription request
- connect, send, read one response line, close

This keeps the client simple and compatible with the upstream example's
single-threaded, request-at-a-time model.

## fp16 Crash Handling Decision

Choose the graceful auto-downgrade path, not a hard refusal.

Rationale:

- `parakeet_fp16` is already `true` by default in Minutes
- requiring users to remember to flip fp16 off just to try the sidecar would
  make beta testing confusing
- the known failure mode is specific and detectable

Decision:

1. If the sidecar is launched with `--fp16` and exits within `60s` of startup,
   inspect the recent stderr buffer.
2. If stderr contains the known MPSGraph dtype mismatch signature, for example:
   - `MPSGraph`
   - `requires the same element type`
   - `original module failed verification`
3. Log a warning that Minutes is downgrading the warm sidecar to fp32.
4. Relaunch once without `--fp16`.
5. Keep subprocess fallback available if the fp32 relaunch also fails.

This downgrade is sidecar-only. It does not mutate the user's config file and
does not change subprocess behavior.

## Fallback Conditions

Minutes falls back to the current subprocess path when any of these happen:

- `parakeet_sidecar_enabled` is `false`
- build is not on a Unix platform
- warm server binary cannot be resolved
- child spawn fails
- socket never becomes healthy
- child exits before becoming healthy for any non-fp16-downgrade reason
- request connection fails and a one-time restart does not recover it
- response is malformed JSON
- response `request_id` does not match
- response is missing required success fields

Fallback behavior:

- log one structured warning with the reason
- mark the manager `SubprocessOnly` for the remainder of the current process
- continue transcription via existing helper/direct subprocess code

Using a process-local circuit breaker avoids repeated crash loops and noisy logs.

## Integration Plan

### New Module

- `crates/core/src/parakeet_sidecar.rs`

Planned public surface:

- `ParakeetSidecarManager`
- `ParakeetSidecarRequest`
- `ParakeetSidecarResponse`
- `SidecarTranscriptionResult`
- `global_parakeet_sidecar()` or equivalent singleton accessor
- `shutdown_global_parakeet_sidecar()`

### `transcribe.rs`

Keep `transcribe_with_parakeet(...)` as the integration seam.

Planned flow:

1. Keep audio loading, temp WAV creation, model/vocab/VAD resolution where they
   are today.
2. Build a `ParakeetSidecarRequest` from the already-resolved assets.
3. If sidecar is enabled, call the manager.
4. On success, adapt the response to `ParakeetCliTranscript` and reuse
   `transcribe_result_from_parakeet_parsed(...)`.
5. On sidecar unavailability or failure, fall back to the existing subprocess
   call graph unchanged.

This keeps the transport swap local and leaves the cleanup pipeline untouched.

### `config.rs`

Add:

```toml
[transcription]
parakeet_sidecar_enabled = false
```

No other user-facing config field is required in phase 1.

### `lib.rs`

Export the new module:

- `pub mod parakeet_sidecar;`

### Shutdown Hooks

Normal-process cleanup should be explicit, not left to orphaned-child luck.

Planned hooks:

- CLI paths that perform offline transcription will call
  `shutdown_global_parakeet_sidecar()` before returning to `main`
- Tauri will call the same function on app exit / after `.run(...)` returns

Even with explicit shutdown, startup should still remove stale socket files to
cover crashes or forced termination.

## Batch Mode Decision

Do not change `transcribe_parakeet_batch(...)` in the first sidecar slice.

Reason:

- the upstream server example is single-request and sequential
- Minutes' existing batch path is already specialized for batched encoder work
- forcing batch jobs through one-request-at-a-time socket traffic would be a
  scope increase with unclear benefit

Phase 1 sidecar integration only targets the single-file
`transcribe_with_parakeet(...)` path.

## Logging and Diagnostics

The manager should emit structured logs with these fields:

- `backend = "parakeet-sidecar"` or `"parakeet-subprocess"`
- `socket_path`
- `request_id`
- `launch_fp16`
- `effective_fp16`
- `startup_ms`
- `elapsed_ms`
- `fallback_reason`
- `server_exit_code`

stderr from the child should be:

- mirrored into `tracing`
- retained in a small recent-lines ring buffer for crash diagnosis

This is necessary for the fp16 downgrade path and for future perf debugging.

## Risks

### 1. Response-shape mismatch

The server returns `word_timestamps`, not the subprocess parser's segment
format. If the adapter is wrong, transcript cleanup could diverge subtly.

Mitigation:

- reuse `group_word_segments(...)`
- keep the final cleanup path shared
- add parity tests using fixture responses

### 2. Orphaned child processes

If shutdown hooks are missed, `example-server` could survive after Minutes
exits.

Mitigation:

- explicit shutdown API
- stale socket cleanup at startup
- per-process socket path

### 3. Crash loops

Repeated launch failures could slow down every transcription request.

Mitigation:

- process-local `SubprocessOnly` circuit breaker after unrecoverable failure

### 4. fp16 confusion

Users may think sidecar is broken when the real problem is the known upstream
MPSGraph fp16 crash.

Mitigation:

- automatic one-time fp32 downgrade with a clear warning

## Phase 2+ Test Plan Preview

Not for implementation in phase 1, but this is the validation target:

- unit tests for request JSON serialization
- unit tests for response parsing and `word_timestamps` -> grouped segments
- unit tests for malformed/partial JSON lines
- integration tests for sidecar startup + fallback using a fake Unix socket server
- manual benchmark matrix on the 21-minute sample:
  - fp32 subprocess baseline
  - fp32 sidecar
  - fp16 subprocess
  - fp16 sidecar if it stabilizes, otherwise fp32-downgraded sidecar

## Open Review Points

These are the decisions I want to lock before implementation:

1. Per-process socket path instead of one shared daemon socket.
2. Auto-downgrade fp16 sidecar launches to fp32 on the known MPSGraph crash,
   instead of refusing sidecar entirely.
3. Keep batch transcription on the existing subprocess batch path for now.
4. Add only `parakeet_sidecar_enabled` as user-facing config in this first
   slice, with server binary discovery handled internally.
