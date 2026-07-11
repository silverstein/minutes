# VAD architecture refactor — design plan

## TL;DR (post-spike, 2026-05-09)

**Option A is dead. Option B is required.** Two spike tests landed in
`crates/core/src/live_transcript.rs` (gated `#[ignore]`, run with
`cargo test ... -- --ignored`):

1. `option_a_spike_silero_state_carries_across_detect_speech_calls` —
   synthetic signal (silence + sine + silence). Length parity matched
   (94 = 47+47), max prob diff 4%, mean 0.5%. Looked promising.

2. `option_a_spike_v2_real_speech_threshold_decisions` — real speech
   from `crates/assets/demo.wav`, 100ms-chunk cadence matching production:
   - **Length mismatch**: Mode A produced 333 windows; Mode B produced 425.
     `detect_speech` pads input to fixed window sizes, so 100ms chunks
     emit 4 windows per call regardless of input length. Cannot
     accumulate cleanly.
   - **Mean prob diff: 38%.** Not noise.
   - **21.3% threshold flips** (71 of 333 windows) — 1 in 5 decisions
     would be wrong if Option A shipped.
   - **Longest flip run: 320ms** — twice the ±150ms tolerance bar.
     66 of 71 flips were `Mode A=speech, Mode B=silence`, meaning real
     speech audio would get gated as silence.

The synthetic-signal result was misleadingly clean because the
threshold-decision boundary was never close to ambiguous; real speech
hovers near the threshold, which is exactly where LSTM state loss bites.

The next sessions should implement Option B per the plan below.

## Revision history

**v2 (after codex plan-review)** — corrections from the first adversarial pass:

1. **Silero ONNX contract was wrong.** v1 said "30ms / 480 samples." Actual is 512 samples + 64-sample context input + explicit `state`/`stateN` tensor round-tripping. Getting this wrong panics at runtime with shape errors, not logic errors. No feature flag saves you. Updated.
2. **Pre-work spike added.** `whisper-rs` exposes `detect_speech` + `probabilities()` directly. A 1-hour spike on Option A (feed new-only samples, accumulate probabilities externally) might get 80% of the win without adding a second ONNX runtime host. Run the spike before committing to Option B.
3. **`ort` feature scope corrected.** v1 said "move ort out from behind diarize." That breaks `--no-default-features --features whisper` builds. v2 introduces a NEW opt-in `vad-ort` feature gate; ort stays out of defaults.
4. **Parity definition tightened.** v1 said "tolerance-based parity (X% within Yms)." Output parity is unreachable in absolute terms because the C-layer logic is inaccessible. The right bar is **zero missed speech islands; boundary drift up to Xms is acceptable**. Utterance loss is not OK; boundary drift is.
5. **Commit 1 framing.** v1 called the trait extraction a "shippable milestone." It is mechanical prep with zero behavior change. Three-commit split is still the right shape for bisect, but commit 1 ships without release-note fanfare.

## Problem

The recording sidecar runs Silero VAD on a sliding 3 to 8 second buffer every 100ms. Each call processes 30 to 80x more audio than is new in that tick. On Apple Silicon CPU it is roughly 25 to 130ms per call depending on buffer state. Bounded but wasteful.

Two visible symptoms surface from this design:

1. The whisper.cpp C-level log line `whisper_vad_detect_speech: detect speech (X.XXs duration)` fires once per 100ms, hammering stderr (mitigated in v0.16.5 by routing through tracing).
2. The CPU ceiling for the parakeet path is dominated by VAD work, not parakeet inference. The parakeet warm-daemon work in epic #163 is intended to drop transcription latency to single-digit ms per utterance, but VAD will become the new bottleneck unless this is addressed.

The bug is structural. We coupled VAD to the whisper transcription engine because `whisper-rs` ships an integrated Silero implementation. That made the implementation cheap but wrong shape: VAD should be a stream-level voice gate independent of whichever ASR engine the user picks.

## Current state (where the code lives)

- `crates/core/src/live_transcript.rs:1101-1304` — VAD constants, `SileroSidecarVad`, `RecordingSidecarVad` (Silero or energy fallback enum), `process()` per-chunk entry point
- `crates/core/src/live_transcript.rs:1262` — the offending call: `self.ctx.segments_from_samples(self.params, &self.buffer)` runs Silero over the entire buffer every chunk
- `crates/core/src/vad.rs` — energy-based fallback (`Vad::new()`, `Vad::process(rms)`)
- `crates/core/src/live_transcript.rs:1865, 1987` — the production caller in the recording sidecar loop
- `crates/core/src/dictation.rs:349` — separate caller (dictation mode), `Vad::new()` energy-only today

The Silero ONNX model itself is small (~2MB) and runs in single-digit ms per inference window. The waste is purely caller-side: we re-feed the entire buffer instead of just the new chunk.

## Design options

### Option A: Incremental Silero via whisper-rs (REVISED)

Keep `whisper-rs` as the Silero host. Use the lower-level `detect_speech` + `probabilities()` API (which I missed in v1), feed only new-since-last-call samples per chunk, accumulate the per-window speech probabilities externally, apply our own min-speech / min-silence / pad logic to derive segment boundaries.

**Pros:** smallest diff, no new dependency, reuses model file resolution. If this works, the entire problem is solved without a second ONNX runtime host.

**Cons:** depends on whether whisper-rs's internal Silero state resets per `detect_speech` call. If it does, accumulating probabilities externally is mathematically not the same as running the LSTM with persistent state — boundary detection will drift in ways we can't tune. The spike has to confirm probability stability across calls before we commit.

**Verdict:** **must spike before Option B.** ~1 hour of code: build a small test that calls `detect_speech` on `samples[0..N]` then on `samples[N..2N]` and compares the merged probability vector to a single call on `samples[0..2N]`. If the merge matches within tolerance, Option A is the answer. If it diverges, Option B is necessary.

### Option B: Independent Silero via `ort` (ONNX Runtime) (REVISED)

Load the Silero ONNX model directly through the `ort` crate (already in the dep tree behind the diarize feature). Implement a stateful VAD that maintains the LSTM `state`/`stateN` tensors across calls, processes 512-sample chunks at 16kHz with a 64-sample context input prepended on each call, returns per-window speech probabilities.

**Pros:** correct shape. VAD becomes truly streaming — cost per chunk is O(new audio), not O(buffer). Decoupled from whisper. Same Silero model file we already download.

**Cons:** real implementation. Critical contract details (corrected from v1):
- **512 samples per inference** at 16kHz, not 30ms/480
- **64-sample context tensor** prepended; carry the trailing 64 samples of each call into the next
- **`state` and `stateN` LSTM tensors** round-tripped between calls (input + output)
- Shape errors at inference time are runtime panics, not graceful degradation, so the contract has to be exactly right

Plus the existing whisper-rs threshold + min-speech + min-silence + speech-pad logic to port. ~200-300 lines of Rust + tests. New feature `vad-ort` (NOT folded into default; pulling ort into `--no-default-features --features whisper` builds would be a regression for users who deliberately opted out of diarize).

**Verdict:** the right answer architecturally if Option A's spike fails. Real refactor.

### Option C: Backend-swappable `Vad` trait + multiple impls

Define `trait Vad { fn process(&mut self, samples: &[f32], rms: f32) -> VadResult; }`. Keep the existing energy and whisper-Silero impls behind it. Add an `ort`-Silero impl in option B's design. Caller picks via config.

**Pros:** clean abstraction. Future engines (parakeet's native VAD if it ever exposes a stream API, webrtc-vad, etc.) plug in the same way. Tests can mock VAD.

**Cons:** mostly bookkeeping if we're not actually swapping engines today. The trait extraction is cheap, but it does not by itself solve the per-chunk re-scan cost. Need option B's implementation as the trait's first new member.

**Verdict:** worth doing alongside option B, not as a substitute. If we're going to write the `ort`-Silero impl, putting it behind a trait makes future evolution easier and tests possible.

### Option D: Shrink the active buffer or accept the cost

Drop `SIDECAR_VAD_ACTIVE_BUFFER_MS` from 8000 to something like 2000 or 3000. Cuts per-chunk work by 2-4x with no architectural change.

**Pros:** ~10-line change. Ships next session.

**Cons:** does not address the fundamental waste, just narrows it. Active buffer length affects how much trailing audio gets re-validated for VAD decisions, so this might subtly change what the system considers "still speaking" near the end of an utterance. Need to trace whether the existing 8s window length is load-bearing for any utterance-finalization logic.

**Verdict:** tactical. Worth as a stopgap if option B is more than a session away. Not a real fix.

## Recommendation (REVISED)

**Phase 0: Run the Option A spike first** (~1 hour). Probe whether `whisper-rs::detect_speech` + `probabilities()` produces stable output across incremental calls. If yes, Option A is the entire solution: rewrite `SileroSidecarVad::process` to accumulate probabilities externally, compute segments from the running probability buffer, no new dependency, no trait extraction needed. Ship as a single ~50-line commit.

**Phase 1: If Option A fails, do Option B+C together.** Implement the `Vad` trait in `crates/core/src/vad.rs`, extract energy and whisper-Silero impls behind it, add `ort`-backed streaming Silero as the third member with the corrected 512-sample / 64-context / state-tensor contract. Default config still points at whisper-Silero; ort impl is opt-in via `transcription.vad_engine = "ort-silero"` until parity testing is complete.

Keep option D in mind as the rollback path: if a streaming impl misbehaves in a way we cannot diagnose quickly, dropping the active buffer to 3000ms is the emergency lever.

## Blast radius

**Touched files:**
- `crates/core/src/vad.rs` — trait extraction, energy impl moved behind it
- `crates/core/src/live_transcript.rs:1095-1304` — `SileroSidecarVad` becomes a trait impl, `RecordingSidecarVad` enum becomes a `Box<dyn Vad>` or stays an enum with the new variant added
- `crates/core/src/silero_vad.rs` (NEW) — ort-backed streaming Silero impl with LSTM state
- `crates/core/src/dictation.rs:349` — dictation's energy-only VAD becomes the trait, can opt into Silero too
- `crates/core/Cargo.toml` — new opt-in `vad-ort = ["dep:ort", "dep:ndarray"]` feature. ort stays out of defaults so `--no-default-features --features whisper` builds keep working without it (the prior plan v1 broke this case).
- `crates/core/src/transcribe.rs` and other ASR-side callers — should be unaffected because VAD is on the recording side

**Test surface:**
- Unit tests for the new ort-Silero impl: state persists across calls, threshold + min-speech + min-silence work correctly, matches whisper-Silero output on a fixed audio fixture within tolerance
- Trait property tests: each impl returns valid `VadResult` for a given input, no panics on edge cases (empty samples, NaN, all-zero buffer)
- Existing `live_transcript.rs::tests` should keep passing without modification (the trait swap is intended to be a black-box replacement)

## Migration strategy (REVISED)

**If Option A spike succeeds:** single ~50-line commit, ships immediately. No trait extraction, no feature flags. Done.

**If Option A spike fails, Option B+C path:**

1. **Trait extraction (mechanical prep, no behavior change).** Energy and whisper-Silero behind the trait. Tests confirm green. Ships green by definition because it's a black-box swap; not a release-note milestone.
2. **Add ort-Silero impl** behind the new `vad-ort` feature, default still pointing at whisper-Silero. Tests confirm output parity vs whisper-Silero on fixture audio per the parity definition below. Ship.
3. **Flip the default** in a third commit after dogfooding the ort impl on a real recording session. Whisper-Silero stays available as fallback for ort-load failures. Ship.
4. **Remove whisper-Silero** in a future major release if confidence holds. Optional, not part of this arc.

This sequence lets us bisect any regression to a single commit.

### Parity definition (codex-tightened)

Output parity between ort-Silero and whisper-Silero is unreachable in absolute terms because the C-layer logic for `min_speech_duration` + `min_silence_duration` + `speech_pad` is internal to whisper-rs and we'd be reimplementing it from scratch.

The right bar is:
- **Zero missed speech islands.** Every contiguous speech region whisper-Silero detects must also be detected by ort-Silero. Utterance loss is a release-blocking regression.
- **Boundary drift up to ±150ms tolerable.** ort-Silero may start a segment up to 150ms before/after whisper-Silero starts the same segment. Beyond that, investigate.
- **No phantom speech.** ort-Silero must not detect speech in regions whisper-Silero deems silent. False positives during silence cause needless ASR work and hallucination risk.

The fixture: a 30s recording with three known utterances separated by clear pauses, plus a 30s recording of pure silence with one short speech token in the middle. Both manually annotated. Tests assert the parity bar across both.

## Tie to parakeet warm-daemon (epic #163)

The parakeet warm-daemon work (Frikallo/parakeet.cpp#19, downstream Minutes integration not yet ticketed) is the parakeet path's transcription-side answer to per-utterance latency. With both this VAD refactor and the warm-daemon swap landed:

- Per-chunk VAD: O(new audio) instead of O(buffer) — saves ~50ms/sec of CPU
- Per-utterance ASR: O(utterance) at ~50ms instead of process spawn at ~1s

Compound improvement is the difference between live coaching being usable mid-call versus mid-pause.

## Tradeoffs that could upset Mat

1. **Three commits and a feature flag, not one diff.** The trait extraction alone is a 200-line PR that doesn't fix the user-visible problem. Worth it for the bisect property; might feel like overhead.
2. **Dependency surface grows.** ort gains a new caller outside of diarize. Build time goes up slightly for users who currently build with --no-default-features (still fine because diarize was a default feature, ort is already linked).
3. **Two Silero implementations live in the tree** during the migration window. Doubles the surface for "VAD bug" reports until the whisper-Silero variant retires.
4. **The test fixture for output parity** between ort-Silero and whisper-Silero needs a real audio sample with known speech/silence boundaries. Manageable but not zero work.

## Risks I want codex to specifically attack

1. **Is the "30ms windows + LSTM state" model of Silero correct, or am I wrong about the contract?** The implementation hinges on this. Fundamental misread invalidates the entire option B design.
2. **Does whisper-rs really lack a stateful VAD API?** The 30-min spike on option A could uncover something I missed in `WhisperVadContext` and make this whole plan over-engineered.
3. **ort's threading model**: the recording sidecar processes chunks on a single dedicated thread. ort sessions are not always Send. Need to confirm we can build the session once and call `run()` from the same thread without contention.
4. **Output parity tolerance**: whisper-Silero's `min_speech_duration` + `min_silence_duration` + `speech_pad` produce specific segment boundaries. My ort impl needs to reproduce those exactly, not approximately. If parity is impossible (different threshold semantics, different smoothing), the migration strategy of "ship behind a flag, dogfood, flip default" is what catches it, but the doc should acknowledge the risk explicitly.
5. **Anything else** structurally wrong with the plan, or a simpler path I missed.
