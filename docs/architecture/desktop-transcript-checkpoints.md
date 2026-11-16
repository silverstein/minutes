# Desktop native-call transcript checkpoints

Native Mac meeting capture uses the API from
[transcript checkpoints](transcript-checkpoints.md) to prepare about 120 seconds
of each source while the call records. This reduces transcription after Stop.
It does not publish a live transcript or replace the recording sidecar.

`recording.transcript_checkpoints` defaults to `true`. Set it to `false` in the
config file, or turn off **Transcribe during calls** in Transcription settings.
A setting change applies to the next recording. Microphone-only capture and
non-meeting modes do not start this worker.

## Capture and Stop isolation

`capture_checkpoint::CaptureCheckpoint` owns an independent stem reader and a
decoder thread. Both use background QoS on macOS. They only read the stem files;
the capture writer never feeds them or waits for them. The reader reuses
`stem_tail::StemTail` with bounded 256 KiB reads and a canonical resampling mode.
It retains one open span per source, sends at most two queued audio spans with
`try_send`, and keeps at most 256 completed results. A full queue drops that span and stops cutting new spans for the call.

Each cut is a cut of the decode plan: the point after a decode group where the
final pass's VAD chunker resets, on its 100 ms grid (see
[decode groups](transcript-checkpoints.md#decode-groups)). A span that starts
there gets the same chunks and groups as the whole-stem pass, so Whisper decodes
the same audio and the text is the same. A group whose end is not known yet has
no cut and waits. The first choice, from 105 seconds on, is the cut with the
longest pause after it. A span that reaches 180 seconds takes the latest cut in
the span. Only with none at all does it fall back to the quietest 100 ms window.
The final pass rejects such a span if its groups differ. The next span begins at
that cut, on the same source clock.
The canonical reader uses the final WAV decoder's sinc resampler, retaining its
phase and lookahead across polls. It does not use the live sidecar's decimator.
The final decoder can normalize quiet files by their whole-file peak. Until the
reader sees a peak of at least 0.1 on a source, it skips its spans because the
final gain is not yet known. Subsequent digest validation remains authoritative.

Hints are built on the decoder thread using the recording start time, title,
pre-context, and the same calendar selection and hint builder as final
processing. That calendar lookup never runs on capture or Stop. A calendar
match already present during capture can reuse spans. If a match appears or
changes later, or the title, context, vocabulary, settings, or audio change,
the final pass deliberately rejects the old spans and transcribes again.
Rejection logs name the failed condition, including settings or decode hints.
The host never changes a fingerprint to make text produced with old hints look
current. Recording remains recoverable
through the ordinary full transcription path.

On normal Stop, the host stops the optional worker before finalizing capture.
It takes only completed results without joining either thread or waiting for a
decode. The same handoff applies to helper exit and finalization errors.
Dropping the handle also signals Stop. Decoder errors and caught Rust panics
end the optional worker. The two sources share one decoder, so a total
real-time factor above 0.5 cannot keep up. After three spans, a total above 0.5
disables remaining span work for that call. One span slower than real time
disables it at once, from the first span on. A single slow span alone does
not: Whisper's temperature fallback can make one hard chunk slow. A process-wide lease
prevents repeated recordings from accumulating stalled decoders. Native aborts
and process crashes are outside the Rust panic boundary.

At Stop, logs report `accepted` (completed spans offered to the job), `rejected`
(offered spans not completed), and `skipped` (queue, quiet-audio, or decoder
failures). These are producer counts. The final pass separately logs its
actual accepted count and every rejection reason. The desktop also saves these
receipts in `~/.minutes/logs/minutes.log`: `capture_checkpoint_model_load`,
`capture_checkpoint_stop`, `transcript_checkpoint_reuse`, and
`transcript_checkpoint_rejected`. The Stop summary uses a bounded queue to a
single background logger; a stalled log sink cannot block Stop.

The additive `transcript_checkpoint::SpanTranscriber` session freezes config
and decode hints and lazily loads one Whisper context for all microphone and
system spans in the meeting. The existing `transcribe_span` function remains
available for standalone callers. Non-Whisper engines retain their existing
batch dispatch. Stop does not cancel an in-progress native decode; its result is
discarded, and it can retain resources until it returns.

## Reproducible file replay

Use synthetic or public speech in paired 16 kHz mono signed 16-bit WAVs of equal
length, longer than 120 seconds and at most ten minutes. Include pauses and
speech in both the completed span and the final tail. Create a fresh directory
with an isolated config containing the desired model and an absolute
`transcription.model_path`. No production config changes are needed.

```bash
MINUTES_DATA_DIR=/absolute/isolated/replay \
MINUTES_CONFIG_PATH=/absolute/isolated/replay/config.toml \
cargo run --release -p minutes-core --no-default-features \
  --features whisper,metal --example replay_capture_checkpoints -- \
  /absolute/voice.wav /absolute/system.wav 4
```

The example appends both stems over time through the same checkpoint producer
as desktop native capture. It takes the nonblocking Stop snapshot, queues and
processes a real job, then compares transcript words with a whole-file pass.
It prints only counts, timing, and comparison results. Check logs for completed
spans during replay, final accepted counts, and no rejection messages. Faster
replay requires a model that finishes before Stop; use speed `1` if necessary.
This is a correctness check, not a call latency or battery benchmark.

## Long-call measurement

In signed Minutes Dev, use the same model, input devices, language, title,
context, and call material for both runs. Disable the optional live sidecar in
both runs if isolating checkpoint cost, or retain the same sidecar setting.

1. With the toggle off, record a call of at least 20 minutes. Note the instant
   Stop is clicked, the first transcript artifact write, and job completion.
2. Repeat with the toggle on. Record Stop counts and the final pass's offered,
   accepted, and covered-seconds log fields. Compare Stop-to-transcript and
   Stop-to-complete times. The unfinished tail should be transcribed normally.
3. Retain both stem pairs. Reprocess the second pair with no checkpoint using
   exactly the same config and hints. Compare transcript words and speaker
   coverage, inspecting every difference at a span cut. Whisper variation can
   change output even with identical input; report differences explicitly.
4. In Activity Monitor, record average and peak CPU, memory, and Energy Impact
   during each call and after Stop. For battery, use equal-duration unplugged
   runs at the same charge range, display brightness, and background workload;
   record charge loss and call duration. Repeat before drawing a battery claim.
5. Confirm recording duration, both stem durations, normal Stop responsiveness,
   and the final transcript after an optional-worker failure. The deterministic
   stalled and panicking decoder tests cover this boundary without risking a
   real call.
