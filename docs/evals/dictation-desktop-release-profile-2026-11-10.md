# Desktop dictation release work — 2026-11-10

## Finding and candidate

Desktop dictation recognizes a partial every two seconds on the audio consumer
thread, although its compact HUD deliberately hides partial transcript text.
The same thread receives capture frames and observes stop/cancel. A preview can
therefore delay release handling and let audio queue up for work with no visible
consumer. This differs from the CLI, which displays partial transcripts.

The candidate adds a caller-level `emit_partials` option. Its default is true,
preserving CLI previews. Desktop explicitly disables it, so `StreamingWhisper`
accumulates the same incoming speech samples and performs final recognition at
the existing utterance boundaries. The model, full final audio buffer, decoding
parameters, recognition hints, cleanup, recovery, cancellation output gate and
backend selection are retained. This does not reuse or stitch a partial result.
There is no new model, download, refiner or persistent setting.

The receive timeout also changes from 100 to 20 ms so a control flag can be
observed between capture frames. It is a control poll interval, not a change to
the 100 ms audio frame size, VAD cadence or silence timeout. This creates more
idle control checks while dictating; its battery effect has not been measured.
The CLI retains its preview generation and also benefits from the shorter idle
control wait.

This follows the [negative exact-audio reuse experiment](https://github.com/silverstein/minutes/blob/70314ec223a142bd517626cf01c5c2cce34fa311/docs/evals/dictation-reuse-usefulness-2026-11-10.md).
Unlike that candidate, it removes work at every scheduled desktop preview,
without requiring the end of speech to coincide with a completed partial.

## Source and measurement boundaries

The baseline is `origin/main` at
`3a69ff467a5004c7ec97d746c5b49284c294b3b7`. The implementation and example were
also built in the owned Mac checkout; production source and example hashes match
the Linux candidate. The component profiles did not target the installed app.
A separate signed installed-app acceptance check follows below; its isolated
configuration did not replace the normal config.

`dictation_release_profile` runs three kinds of explicit public-audio control:

- **Identical audio:** partials-plus-final versus accumulation-plus-final on
  two-second, four-second and full-fixture buffers. These are text regression
  checks; their unpaced timings are not used as native latency claims.
- **Paced release:** one continuously active utterance, 100 ms frames, a bounded
  64-frame channel and stop-at-loop-top routing. A separate thread requests
  release at 2,050, 2,350, 3,950 or 4,050 ms. The original path generates previews
  and polls control every 100 ms; the candidate accumulates and polls every
  20 ms. Pair order alternates across release cases.
- **Fresh process:** three alternating-order process pairs per host, with a new
  context and no warmup. CPU time includes model setup, replay and teardown.
  OS/model-page/backend caches are not cleared. CPU time is not GPU time or a
  battery measurement.

The paced replay uses the actual `StreamingWhisper` methods but a source-matched
consumer loop, not `dictation::run`. It intentionally excludes VAD segmentation
to isolate release while a preview may be running. Device initialization,
resampling, native hotkeys, recovery I/O, cleanup, target insertion and OS input
dispatch are outside these timings. Thus `release_to_final` below means the
replay's control request to decoded text, **not key-up to visible inserted text**.
The existing VAD/end-of-speech route remains covered by speech regressions; no
silence-stop or real-speaker quality improvement is claimed from this profile.

Removing blocking preview work can change which frames have reached recognition
when stop is observed. The baseline and candidate therefore sometimes consume
different audio. Each paced result is checked against an independent final-only
decode of its own exact consumed buffer. This does not establish baseline versus
candidate text equality across different buffers. Capture frame quantization
and the existing policy of not draining queued audio at stop remain visible in
the raw counts. No new trimming or queue-drain policy is introduced.

## Metal results

Host: Apple M4 Max, 128 GiB RAM, macOS 26.6, existing Tiny and Small models with
`whisper,streaming,metal`. Each row is one selected case, not a p95 estimate or
an estimate of typical user behavior.

| Model / release request | Original stop wait | Candidate stop wait | Original release-to-final | Candidate release-to-final |
| --- | ---: | ---: | ---: | ---: |
| Tiny / 2,050 ms | 29.4 ms | 14.0 ms | 90.5 ms | 79.6 ms |
| Tiny / 2,350 ms | 51.1 ms | 19.4 ms | 130.7 ms | 105.3 ms |
| Tiny / 3,950 ms | 50.3 ms | 17.6 ms | 151.8 ms | 134.3 ms |
| Tiny / 4,050 ms | 87.0 ms | 16.6 ms | 174.2 ms | 119.0 ms |
| Small / 2,050 ms | 95.8 ms | 16.8 ms | 239.9 ms | 184.5 ms |
| Small / 2,350 ms | 50.4 ms | 20.0 ms | 195.9 ms | 194.6 ms |
| Small / 3,950 ms | 234.3 ms | 18.8 ms | 434.7 ms | 248.0 ms |
| Small / 4,050 ms | 136.6 ms | 20.8 ms | 272.2 ms | 230.9 ms |

All eight selected Metal cases improved in this run. Differences of around one
millisecond are within ordinary run noise; this is evidence for the scheduling
mechanism, not a general speedup percentage. Stop checks are not real-time
deadlines: scheduling can exceed the 20 ms poll interval. Candidate queue length
at stop was zero in these controls; the original had zero or one queued frame.

All six identical-input Metal pairs matched, and all 16 paced Metal results
matched independent final decodes of their own consumed audio. Raw evidence:
[Tiny](fixtures/dictation-release-tiny-metal-2026-11-10.json) and
[Small](fixtures/dictation-release-small-metal-2026-11-10.json).

## CPU work and the cold-decode tradeoff

Fresh-process controls use the complete 10.63-second public demo. The CPU figures
below are medians of three runs per mode. Text was identical across all six
fresh-process runs on each host.

| Host / Tiny | Preview mode CPU time | Final-only CPU time | Preview mode final decode | Final-only final decode |
| --- | ---: | ---: | ---: | ---: |
| Linux CPU | 31.372 CPU-s | 4.650 CPU-s | 607.7 ms | 626.0 ms |
| M4 Max Metal | 0.275 CPU-s | 0.138 CPU-s | 61.9 ms | 99.7 ms |

This selected probe used about 85% less CPU time on Linux and 50% less on the
Mac, including model setup and teardown. These are per-process fixture costs,
not whole-app CPU reductions, GPU utilization or battery savings. Removing the
earlier preview also moves cold decoding work to the final pass: approximately
18 ms more on Linux and 38 ms more on Metal in these medians. That tradeoff is
preserved rather than hidden behind the warm replay.

An initial candidate removed previews but retained the old 100 ms control
timeout. It reduced computation but regressed some selected Metal release
cases. That result motivated the shorter control wait; both initial and final
results are retained. The final combined candidate improved all eight selected
Metal cases above, including its real final-decode cost. This does not guarantee
an improvement for every utterance, machine, model or first session.

Raw fresh-process controls:
[Linux](fixtures/dictation-release-fresh-linux-2026-11-10.json),
[Metal](fixtures/dictation-release-fresh-metal-2026-11-10.json).
Initial buffer-only / 100 ms control result:
[Metal](fixtures/dictation-release-initial-metal-2026-11-10.json).

## Linux paced cross-check

Using Tiny on the shared Linux CPU host, the two public fixtures each improved
in all four selected cases. Original stop waits ranged from 152 to 1,051 ms;
candidate waits were approximately 10.5–10.7 ms. Original paths left four to ten
frames queued at stop; candidates left zero in these cases. These CPU figures
must not be used as Mac latency estimates.

All six identical-input Linux pairs matched. All 16 paced results matched their
independent same-consumed-audio references. Across both hosts and models, that
is 12 identical-input pairs plus 32 consumed-buffer reference checks, all passing.
These checks establish neither human-reference transcription accuracy nor
representative names/numbers/negation quality.

Raw Linux results: [demo](fixtures/dictation-release-linux-demo-2026-11-10.json),
[dictation](fixtures/dictation-release-linux-dictation-2026-11-10.json).
The [summary](fixtures/dictation-release-summary-2026-11-10.json) retains source
and fixture hashes, exact values and whether each paced pair consumed equal
sample counts.

## Reproduction

Only use explicit public audio for shared evidence because outputs contain
transcripts. Supply an existing model; these tools do not download one.

```sh
cargo build -p minutes-core --no-default-features --features whisper,streaming \
  --example dictation_release_profile

target/debug/examples/dictation_release_profile \
  /absolute/path/to/ggml-tiny.bin crates/assets/demo.wav

python3 scripts/run_dictation_fresh_controls.py \
  target/debug/examples/dictation_release_profile \
  /absolute/path/to/ggml-tiny.bin crates/assets/demo.wav /absolute/evidence-dir

MINUTES_TEST_WHISPER_MODEL=/absolute/path/to/ggml-tiny.bin \
  cargo test -p minutes-core --no-default-features --features whisper,streaming \
  --lib streaming_whisper::tests::deferred_previews_preserve_final_text_and_remove_recognition_passes \
  -- --exact --ignored --test-threads=1
```

Add `metal` to the feature list for the Apple Silicon build. The installed-model
regression was explicitly executed on Linux and Metal. It verifies that buffer
accumulation performs zero recognition passes, finalization performs one, and
the final text and audio duration match the preview-enabled path. A model-free
regression preserves all samples beyond the partial-window cap and checks reset.
The shipped HUD controller regression confirms partial updates stay hidden while
final copied text remains accessible. The frontend controller is unchanged.

## Validation

| Check | Result |
| --- | --- |
| Core library, no default features, serial | 2,196 passed; 5 ignored |
| Speech feature execution profile | 144 passed; 6 ignored |
| Explicit installed-model recognition-count/text regression | Passed on Linux and M4 Max Metal |
| HUD / readiness / history controller regressions | 16 / 23 / 13 passed |
| Feature coverage auditor tests and mapping audit | 16 passed; mapping audit passed |
| Strict all-target core Clippy, `pocketstation-capture,whisper` | Passed |
| Desktop `cargo check -p minutes-app` | Passed |
| CLI builds with and without Whisper | Passed |
| Core `streaming,engine-sherpa` compile check | Passed |
| Rust formatting, source identities, JSON evidence and report links | Passed |

The implementation and these limits are tracked in local Bead `minutes-mf86`.
A signed installed-app check exercised Finish and Cancel as described below.
Physical key-up latency, a paired installed baseline and battery effects remain
unmeasured. Backend selection was pinned explicitly for this check; it does not
establish which backend a user's `auto` configuration selects.

## Signed installed-app acceptance

Runtime source: `3e93b30d0062b0ba986f2bdce3aa0a8016ea36a6`.
The canonical installer ran from a fresh Apple Terminal context after an SSH
signing probe reproduced `errSecInternalComponent`. It built with
`parakeet,metal,engine-sherpa` and installed `~/Applications/Minutes Dev.app`,
bundle ID `com.useminutes.desktop.dev`. Strict seal verification and the
installed-app CGEventTap diagnostic passed. The latter establishes Input
Monitoring/event-tap operation, not Accessibility or physical shortcut dispatch.

Signing authority was `Apple Development: Mathieu Silverstein (97C8UF8QVF)`,
TeamIdentifier `63TMLKT8HN`. Installed executable mtime was
`2026-11-10T04:16:12.939493Z`; the fresh PID was `25911`, started at
`2026-11-10T04:17:21Z`. The fixture records the installed executable hash.

An isolated Dev acceptance profile used existing Whisper Tiny, deterministic
rules cleanup, an owned TextEdit document and the public demo WAV routed through
`MMAudio Device`. Playback selected that exact virtual output per process and
verified that the system default output was unchanged. No production app was
replaced. The harness temporarily extended its own silence deadline to 30 seconds
so UI inspection could finish, then restored it to 2 seconds.

| Native operation | Observed result |
| --- | --- |
| Finish during active fixture playback | 65 ms from production stop command to final text; 90 ms to visible text; `native_ax`, typed, verified |
| Two manual Finish clicks after VAD recognition completed | 34 / 32 ms to visible delivery; `release_to_final_ms` null, so these are not release-to-decode measurements |
| Cancel during active playback | Target text and history unchanged; HUD dismissed |

The timed Finish request was issued about 2.30 seconds after playback started,
while the playback process remained active. It clicked the real HUD Finish
control, which invokes the production stop command. It is not a physical key-up
measurement. Final target text was inspected independently through AX and a
window screenshot. The timed Cancel request was similarly made about 2.26
seconds into active playback, with target text/history checked before and after.

This is one controlled timed Finish run, without a paired installed baseline or
a p95 estimate. It establishes end-to-end delivery and cancellation for this
explicit backend/model. It does not establish comparative native speed or
transcription quality: captured buffers were not paired, one live session had
stray tokens, and three input-overload warnings were logged during the QA window.
No representative acoustic, names/numbers/negation, battery or GPU-use assessment
is claimed. The selected component measurements above remain the comparative
performance evidence.

Raw content-free latency events, identity, fixture/model hashes, timing and
cancel receipt: [native acceptance](fixtures/dictation-release-native-2026-11-10.json).
