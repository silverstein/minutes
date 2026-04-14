# Parakeet Perf Investigation — 2026-04-14

## Question

Why did a real 21-minute meeting transcribe at roughly 9× realtime instead of the expected 30–50× on Apple Silicon, and which default changes are justified right now?

Target meeting:

- Job: `job-20260414144147744-79241-0`
- Meeting audio recorded at `2026-04-14T14:20:35-07:00`
- Production observation from `~/.minutes/logs/minutes.log`: `141,418 ms` transcribe step
- Current job JSON points to `/Users/silverbook/meetings/2026-04-14-untitled-recording-mat.wav`

Note: the handoff memo referenced `~/.minutes/jobs/job-20260414144147744-79241-0.wav`, but that file was not present during this investigation. The job JSON for the same job ID pointed to the meeting WAV under `~/meetings/`, and that artifact was used for all reruns.

## Methodology

Build:

```bash
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build --release -p minutes-cli --features parakeet
```

Isolation strategy:

- Each run used a fresh temporary `HOME` and `XDG_CONFIG_HOME`.
- The temp config pointed `model_path` at the real local Parakeet assets under `/Users/silverbook/.minutes/models`.
- The temp config set `engine = "parakeet"` and used the installed `/Users/silverbook/.local/bin/parakeet`.
- `diarization.engine = "none"` and `summarization.engine = "none"` to keep the run focused on raw transcription while still exercising the real `minutes process` path and structured logging.
- The `transcribe` timing was read from each run's isolated `$HOME/.minutes/logs/minutes.log`.

Why this harness is trustworthy:

- A single validation rerun with `fp16=false` and `noise_reduction=true` produced `143,021 ms`, essentially matching Mat's observed `141,418 ms`.
- That means the temp-home harness reproduced the slow path closely enough to compare settings without polluting the live config.

## Current Code Reality

Two implementation details materially changed the interpretation of the suspects:

1. `parakeet_fp16` is a real lever in the current subprocess path.
2. `noise_reduction` is currently ignored for Parakeet.

Evidence:

- `crates/core/src/transcribe.rs` logs that `noise_reduction` is enabled but not applied for the Parakeet engine.
- The `noise_reduction` config is global and still matters for Whisper, so changing its default based on a Parakeet-only perf sweep would be unsafe.
- The current Parakeet path still shells out per transcription; it does not reuse the upstream warm Unix-socket `example-server`.

## Benchmark Matrix

Audio duration: `1272.6s` (21m 13s)

| Config | Run 1 (ms) | Run 2 (ms) | Run 3 (ms) | Mean (ms) | Stddev (ms) | Realtime |
|---|---:|---:|---:|---:|---:|---:|
| `fp16=false`, `noise_reduction=true` | 192,888 | 147,420 | 134,306 | 158,204.7 | 30,744.0 | 8.04× |
| `fp16=true`, `noise_reduction=true` | 137,244 | 120,049 | 118,880 | 125,391.0 | 10,281.6 | 10.15× |
| `fp16=false`, `noise_reduction=false` | 123,581 | 130,900 | 123,665 | 126,048.7 | 4,201.6 | 10.10× |
| `fp16=true`, `noise_reduction=false` | 113,932 | 113,045 | 130,548 | 119,175.0 | 9,859.3 | 10.68× |

What matters:

- Flipping `parakeet_fp16` improved mean transcribe time by about 20.7% versus the current default lane (`158.2s` -> `125.4s`) with `noise_reduction=true`.
- The fastest lane in this sweep was `fp16=true`, `noise_reduction=false`, about 24.7% faster than the current default lane (`158.2s` -> `119.2s`).
- Despite that improvement, the best current subprocess result is still only about `10.7×` realtime, far short of the expected `30–50×`.

## Transcript Diff Observations

Raw transcript stability by lane:

- `fp32 + noise=true` and `fp32 + noise=false` were byte-identical across all runs.
- `fp16 + noise=true` and `fp16 + noise=false` were also byte-identical across all runs.
- So the `noise_reduction` toggle produced no transcript change for Parakeet, consistent with the code path that ignores it.

Observed fp16 drift relative to the fp32 transcript:

- Exactly one lexical/tokenization difference was found in the entire meeting transcript:

```diff
- [17:06] what is not true, I guess, is what I'm see here is that CCRX still owns the data.
+ [17:06] what is not true, I guess, is what I'm see here is that C CRX still owns the data.
```

Summary:

- Word count changed from `3402` to `3403`.
- No broader content loss, truncation, or timestamp instability was observed.
- This is a very small regression, but it is not literally zero-diff.

## Warm Server Side Benchmark

The separate `parakeet-cpp-server` worktree already had a built Unix-socket `example-server` binary at:

- `/Users/silverbook/Sites/parakeet-cpp-server/build/examples/server/example-server`

The installed CLI binary and the server worktree binary were both Metal-capable:

- both are Mach-O arm64
- both reference axiom Metal availability symbols
- the server worktree build has Metal and MPSGraph enabled in `CMakeCache.txt`

Warm server fp32 benchmark (`--gpu`, `--vad`, `tdt-600m`, three requests against one warm process):

| Warm Server Config | Run 1 (ms) | Run 2 (ms) | Run 3 (ms) | Mean (ms) | Stddev (ms) | Realtime |
|---|---:|---:|---:|---:|---:|---:|
| `example-server`, `fp16=false` | 92,164 | 103,037 | 107,361 | 100,854.0 | 7,830.2 | 12.62× |

Warm server fp16 result:

- `example-server --fp16 --gpu` crashed at startup/runtime with an MPSGraph dtype mismatch:
  - `'mps.add' op requires the same element type for all operands and results`
  - `failed assertion 'original module failed verification'`

Interpretation:

- A warm server is materially faster than the current shipped subprocess path.
- But the server's fp16 path is not currently viable on this machine, so wiring Minutes to it now would still require a fallback or upstream/server-side fix.
- Even warm fp32 only reached about `12.6×` realtime on this real meeting, which is better, but still nowhere close to `30–50×`.

## Recommendation

### Default changes now

1. Flip `parakeet_fp16` default to `true`.
   - Evidence: about 20% faster on mean in the current shipped path.
   - Risk: one small transcript drift (`CCRX` -> `C CRX`) on this meeting.
   - Decision: acceptable for the default, but worth documenting as an experimental tradeoff rather than claiming perfect parity.

2. Do **not** flip `noise_reduction` default based on this bead.
   - In the Parakeet path, it is currently ignored.
   - In the Whisper path, it is still meaningful.
   - Changing the global default here would be acting on the wrong signal.

### Warm server work

Pursue Unix-socket server wiring as a separate bead, not in this one.

Why separate:

- The current upstream `example-server` fp16 path crashes on this machine.
- The server request/response path still needs transcript parity verification versus Minutes' current cleaned transcript format.
- Wiring the socket client cleanly needs config decisions and fallback behavior, not just a mechanical transport swap.

### Bottom line

- The immediate low-risk win is `parakeet_fp16 = true`.
- The bigger structural opportunity remains warm-process reuse, but it is not ready to land today without more targeted work.

## Sketch: where to wire the Unix socket server

Do not implement in this bead; this is the likely insertion point.

Current flow in `crates/core/src/transcribe.rs`:

1. `transcribe_with_parakeet(...)`
2. load audio + build temp WAV
3. resolve model/vocab/VAD
4. helper branch:
   - `minutes parakeet-helper`
   - which then calls `run_parakeet_cli_structured(...)`
   - which then shells out to `parakeet`

Suggested refactor:

1. Add an optional config field such as `parakeet_server_socket: Option<PathBuf>`.
2. In `transcribe_with_parakeet(...)`, after model/vocab/VAD resolution and before the helper/direct subprocess branch:
   - if `parakeet_server_socket` is configured and the socket is reachable:
     - send one JSON request over the Unix socket
     - receive newline-delimited JSON response
     - map the response into `ParakeetCliTranscript`
   - else fall back to today's helper/direct subprocess path
3. Reuse the existing `transcribe_result_from_parakeet_parsed(...)` cleanup so transcript normalization stays shared.
4. Log which backend served the request (`subprocess-direct`, `subprocess-helper`, `unix-socket-server`) so future perf investigations can attribute timings correctly.

This keeps the transport swap local to the Parakeet entrypoint instead of forcing pipeline-wide changes.
