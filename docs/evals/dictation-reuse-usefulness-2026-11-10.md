# Exact-audio dictation reuse: usefulness decision — 2026-11-10

## Decision

Close PR #1318 as a negative default-optimization experiment. Do not merge it or
enable `MINUTES_DICTATION_FINAL_REUSE`. The bounded follow-up found four reuse
hits in 80 phase-sweep pairs (5%), all at exact full-buffer alignment. Both
complete public fixtures missed in both ending modes (0/4). All 84 same-input
baseline/candidate text comparisons matched. No further native qualification is
planned for this candidate; this is a decision to stop pursuing it, not a claim
that its outstanding native acceptance checks passed.

The previous selected-hit benchmarks established that reuse can avoid a decode.
This sweep establishes how narrow that opportunity is under the existing
two-second partial schedule and 100 ms capture frames. It does **not** establish
a natural user hit rate or a release-to-visible speedup. The original evaluation
and signed-app evidence remain in
[the November 6 report](dictation-final-reuse-2026-11-06.md).

## Method and source boundary

The runtime candidate is `6534df23b044884e1c211da41f32b154037e5df3`, with prior
evidence at branch head `480e85e46dea7d64d4491fb300101365d1a28d54`. Current main
was checked at `3a69ff467a5004c7ec97d746c5b49284c294b3b7`. Its capture chunker
and energy VAD match the candidate, and its dictation feed/finalize/reset routing
has no intervening changes apart from the unmerged reuse candidate itself.

The new example runs the actual production `ChunkAccumulator`, `Vad`, and
`StreamingWhisperSession`. Its routing mirrors `dictation::run`: feed every
VAD-speaking chunk, including hangover; finalize and reset at the first
non-speaking chunk; flush pending speech on explicit stop. It supplies 171-sample
callbacks to the chunker, which emits its normal 1,600-sample / 100 ms frames.
RMS uses the same f64 sum-of-squares formula as capture.

This is **unpaced component replay**, not an invocation of `dictation::run` or
native capture. It excludes device initialization, resampling, channel backlog,
concurrent release during decoding, PID/recovery paths, OS shortcuts, cleanup,
and insertion. It uses explicit English, an empty recognition hint, the existing
Tiny model, the default 30-second partial window, and Linux CPU execution. All
clips are under the default utterance cap. No configuration, private audio,
history, shared desktop input, or new model downloads are used.

For each public fixture, 20 successive 100 ms cutoffs cover a full two-second
partial interval. `demo.wav` uses 4.0–5.9 seconds; the shorter dictation fixture
uses 2.0–3.9 seconds. A complete-fixture case is also included. Each cutoff has
two modes:

- **Release:** stop at the cutoff after complete capture frames have been emitted.
- **VAD end:** append one second of zeros, allowing the real VAD to end speech.

The same segmented sample buffers feed baseline and candidate. Model setup and
one warm decode are excluded. Pair order alternates by cutoff. There is one
pair per case; timing differences on misses are not evidence of a speedup.
Prefix cutoffs can truncate words, so transcript equality here checks
regression, not transcription accuracy. The uniform phase grid is deliberately
constructed and must not be presented as representative user behavior.

The decision rule was preserved before execution in the private protocol
receipt: if text-producing reuse occurs in at most 10% of the phase sweep and
remains confined to exact alignment, close this prototype instead of spending
more time on native qualification. Broader behavior would have retained the
draft for a targeted native study. This is a project prioritization threshold,
not a universal performance standard.

## Results

| Fixture / ending | Phase-sweep hits | Complete-fixture hits | Exact hit cutoff | Fed audio at hit |
| --- | ---: | ---: | ---: | ---: |
| Demo / release | 1/20 | 0/1 | 4.0 s | 4.0 s |
| Demo / VAD end | 1/20 | 0/1 | 5.5 s | 6.0 s |
| Dictation / release | 1/20 | 0/1 | 2.0 s | 2.0 s |
| Dictation / VAD end | 1/20 | 0/1 | 3.5 s | 4.0 s |

All 84 pairs produced text, each with one utterance, and all texts matched.
Each group's only alignment-eligible case reused successfully. Every other case
ran normal final recognition. The VAD's five hangover chunks shift the hit
phase by 500 ms; they do not broaden its eligibility window.

The four hit controls avoided Linux CPU final decodes of 953.960, 992.370,
496.635, and 530.703 ms respectively. Cached finalization measured below the
microsecond timer resolution. These are decode-stage observations on this
shared Linux host, not native latency. Previously measured selected M4 Max Metal
hits avoided approximately 52 ms with Tiny and 114 ms with Small; this follow-up
does not turn those selected values into typical Mac savings.

Raw pairs: [demo](fixtures/dictation-reuse-usefulness-demo-2026-11-10.json),
[dictation](fixtures/dictation-reuse-usefulness-dictation-2026-11-10.json).
Summary: [grouped results](fixtures/dictation-reuse-usefulness-summary-2026-11-10.json).

## Reproduction and checks

Use an already installed model; the example never downloads one:

```sh
cargo run -p minutes-core --no-default-features --features whisper,streaming \
  --example dictation_reuse_usefulness -- \
  /absolute/path/to/ggml-tiny.bin crates/assets/demo.wav 1

cargo run -p minutes-core --no-default-features --features whisper,streaming \
  --example dictation_reuse_usefulness -- \
  /absolute/path/to/ggml-tiny.bin tests/eval/fixtures/audio/apple-speech-dictation.wav 1

cargo test -p minutes-core --no-default-features --features whisper,streaming \
  --example dictation_reuse_usefulness
```

Three harness regressions passed: VAD hangover invalidates an aligned release;
a partial capture frame is not fabricated on stop; an internal pause finalizes
before the next utterance. Both actual-model sweeps exited successfully with
all text comparisons equal. The original candidate's broader runtime checks
remain recorded separately; this follow-up changes only the example and evidence.

| Follow-up check | Result |
| --- | --- |
| Actual-model phase/full-fixture replay | 84/84 text pairs identical; both invocations exited 0 |
| Harness regressions | 3 passed |
| Core model-free library, serial | 2,167 passed; 4 ignored |
| Model-free CLI build | Passed |
| Strict core Clippy, `pocketstation-capture,whisper`, all targets | Passed |
| Rust formatting, diff whitespace and report evidence links | Passed |

## Next useful investigation

Profile the work remaining at release before selecting another optimization.
In particular, partial recognition currently executes synchronously inside the
capture loop. A release arriving during that call may wait before the loop
observes stop; this replay does not simulate that wait. Measure that contribution
alongside audio-queue age, final recognition, cleanup and verified insertion.

Use the existing `release_to_final_ms` and `release_to_visible_ms` observations
when available, but preserve their clock boundaries. The existing
`speech_end_to_final_ms` clock begins at `DictationEvent::Processing`, after VAD
has declared speech ended; it does not include the acoustic endpoint/hangover.
Automatic silence-stop delivery and explicit release also need separate results.
Resolve and record the actual backend first: `auto` can select installed Sherpa
on eligible Macs or fall back to Whisper. No engine change, larger model, faster
partial cadence, speculative text stitching, or reduced VAD hangover follows
from this negative result.
