# Lightweight dictation finalization experiment — 2026-11-06

## Scope and decision

Inspired by [Phonon's recognition scheduling](https://github.com/Infatoshi/phonon/blob/2e9a5b724845b7b5881701f06c08d00deccc3e5f/sidecar/stream_asr.py), this experiment tests whether completing recognition work during speech can reduce finalization cost without adding a refiner or downloading another speech model. The Minutes candidate only reuses a successful full-buffer Whisper decode when no additional samples have been fed into that utterance. It does not stitch text, trim silence, infer an immutable prefix, or promote a bounded-window partial. The default remains off pending native qualification and a useful measured hit rate.

`MINUTES_DICTATION_FINAL_REUSE=1` opts a dictation process into the candidate. Other values and an unset variable preserve ordinary finalization. The flag applies only to Whisper finalization, including a configured final engine's Whisper fallback. It does not change the selected final engine, its availability gates, cleanup, insertion, or model downloads. The experiment retains one extra transcript string and schedules no additional recognition passes. It adds no model or audio copy.

## Current path and limits

Minutes already preloads Whisper and the eligible Parakeet v3 recognizer, emits Whisper partials during speech, and finalizes utterances on VAD boundaries. Whisper partials run after two seconds of new speech audio; finalization normally performs another recognition pass. Long partials use a bounded recent audio window, whereas finalization uses the full utterance. A completed partial is eligible for exact-audio reuse only while it covers that entire utterance. Any nonempty subsequent feed invalidates it, even one sample. A session binds the recognizer for its lifetime so cached output cannot cross models. Reset, aborted decoding, empty/failed results, and disabling the candidate do not promote cached text. Duplicate partials suppressed by the existing preview deduplication conservatively miss the cache.

This is deliberately narrower than Phonon's context-rich trailing-window recognition. Most releases between the existing two-second partial boundaries will still execute the full final pass. Reusing a result after new speech would require a separate overlap/alignment quality evaluation. Releasing after a pause can already be fast because Minutes may have finalized that utterance before release.

## Measurement method

Source baseline: `origin/main` at `2e64cd4932851644b3f0829ecfa07d8052fda897`. Host: Linux x86_64 shared build host, Rust 1.95.0, CPU Whisper, existing tiny model. The existing local latency reporter found zero dictation samples on this host. No meeting library, dictation history, private audio, or remote desktop was read.

The checked-in benchmark replays a supplied public PCM16 mono 16 kHz WAV in 100 ms audio chunks through the production session, using identical samples for baseline and candidate. It warms the recognizer once, then alternates pair order across iterations. Cases include an aligned four-second release, another 100/500 ms of audio, the whole fixture, a one-second partial-window cap, a too-short utterance, and silence. It records feed and finalization time separately, exact text equality, and actual cache reuse. Timing is unpaced engine replay in a development build on a shared VM, not hotkey-to-visible-text or Apple Silicon performance. Cache-hit controls are selected deliberately and cannot estimate the natural release hit rate.

Reproduce the engine comparison with an already installed model (add `metal` to the feature list for an Apple Silicon Metal build):

```sh
cargo run -p minutes-core --no-default-features --features whisper,streaming \
  --example dictation_finalization_benchmark -- \
  /absolute/path/to/ggml-tiny.bin crates/assets/demo.wav 3
```

The benchmark prints fixture transcripts in its JSON. Use public fixtures for shared evidence. It loads only the explicit model and WAV arguments and does not consult user configuration. It exits nonzero if any baseline/candidate text differs; text equality is a regression check, not evidence of transcription accuracy against human references.

## Verification method

The model-backed regression runs the same production session against repository audio. It counts recognition passes to verify a cache hit executes zero final recognition passes and each invalidated/full-window-mismatch case performs the ordinary final pass. It also exercises reset, disable-after-feed, and cancellation. Run it with an explicit existing model:

```sh
MINUTES_TEST_WHISPER_MODEL=/absolute/path/to/ggml-tiny.bin \
  cargo test -p minutes-core --no-default-features --features whisper,streaming \
  --lib streaming_whisper::tests::final_reuse_preserves_text_and_rejects_incomplete_or_invalidated_decodes \
  -- --exact --ignored --test-threads=1
```

Native acceptance and natural hit-rate measurement are tracked in Beads as `minutes-n843`, discovered from prototype task `minutes-f7pj`, rather than implied by the engine replay. A signed Minutes Dev build must exercise actual release, cancellation, and target delivery before promoting the candidate. The existing `dictation_latency` events separate release-to-final, release-to-visible, clipboard restoration, and verification. The opt-in candidate additionally records content-free `dictation_final_reuse` events with reuse/produced-text booleans and finalization duration. These events do not include transcript text, audio, model paths, or target identity.

## Observed engine results

All 21 baseline/candidate pairs produced identical text (including matching absence of text). The selected aligned-release case reused the completed decode on all three runs; each other case fell back conservatively. The candidate performed no extra recognition during feed. Three deliberately aligned hits out of these 21 selected controls are not an estimate of real-world hit rate.

| Case | Reused / runs | Baseline final p50 (ms) | Candidate final p50 (ms) |
| --- | ---: | ---: | ---: |
| `aligned_4s` | 3 / 3 | 969.226 | 0.000 |
| `new_audio_100ms` | 0 / 3 | 997.463 | 986.351 |
| `new_audio_500ms` | 0 / 3 | 968.684 | 973.487 |
| `whole_fixture` | 0 / 3 | 597.696 | 608.299 |
| `bounded_suffix` | 0 / 3 | 959.926 | 975.433 |
| `too_short` | 0 / 3 | 0.000 | 0.000 |
| `silence` | 0 / 3 | 516.004 | 512.048 |

The aligned case removes a roughly 969 ms duplicate CPU decode in this fixture. The cached path measured below one microsecond at the timer's resolution; that is not a claim of zero end-to-end latency. Cases with another 100/500 ms of speech still pay for final recognition. Differences between baseline and candidate miss timings are shared-host noise across three pairs, not an established speedup or regression. This narrow result does not justify changing the default or claiming Phonon-like release latency.

Raw public-fixture pairs: [dictation-final-reuse-2026-11-06.json](fixtures/dictation-final-reuse-2026-11-06.json).

Reproduction identity: tiny model, 77691713 bytes, SHA-256 `be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21`.

Reproduction identity: public WAV, 340104 bytes, SHA-256 `d32616b3f1a5a669c35885d815f9e82b1447d50d07a1892c4e08e710f7a85c8b`.

## Apple Silicon follow-up

The exact runtime candidate `6534df23b044884e1c211da41f32b154037e5df3` was replayed on an Apple M4 Max with 128 GB RAM running macOS 26.6, using the existing Tiny and Small models and `whisper,streaming,metal`. This repeats the same unpaced development-build benchmark and public fixture; it is not an installed-app or natural speaking test. All 42 baseline/candidate pairs produced identical text. The deliberately aligned case reused on all six runs across the two models; every other case missed conservatively.

| Model / case | Reused / runs | Baseline final p50 (ms) | Candidate final p50 (ms) |
| --- | ---: | ---: | ---: |
| Tiny / `aligned_4s` | 3 / 3 | 52.121 | <0.001 |
| Tiny / `new_audio_100ms` | 0 / 3 | 50.157 | 50.292 |
| Tiny / `whole_fixture` | 0 / 3 | 61.089 | 62.474 |
| Tiny / `bounded_suffix` | 0 / 3 | 50.368 | 50.462 |
| Small / `aligned_4s` | 3 / 3 | 113.725 | <0.001 |
| Small / `new_audio_100ms` | 0 / 3 | 114.270 | 112.724 |
| Small / `whole_fixture` | 0 / 3 | 180.605 | 183.063 |
| Small / `bounded_suffix` | 0 / 3 | 115.892 | 114.814 |

The Mac result bounds the opportunity more usefully than the shared Linux host: the selected hit avoids about 52 ms with Tiny or 114 ms with Small. It does not establish a meaningful benefit for typical dictation. The explicit installed-Tiny regression also passed on this Mac (1 passed, 0 failed), including invalidation and cancellation controls.

The canonical installer produced a signed target-only Minutes Dev bundle with a strict seal on the same Mac. Signing from SSH first failed with `errSecInternalComponent`; one minimal probe reproduced the failure. That same probe and the canonical build succeeded from a fresh Apple Terminal instance. This is signing-context evidence, not a permission grant or installed-app acceptance. The intended identity is `com.useminutes.desktop.dev`, with signing authority `Apple Development: Mathieu Silverstein (97C8UF8QVF)` and observed TeamIdentifier `63TMLKT8HN`.

## Signed installed-app follow-up — 2026-11-09

Silverbook became reachable again. The missing launch/build receipts confirmed the interrupted November 6 installation command did not reach the host. The retained runtime checkout was still clean at `6534df23b044884e1c211da41f32b154037e5df3`. Current `main` had advanced without changes to the dictation runtime files; this qualification remains bound to the candidate commit rather than implying a main-branch release.

Before replacing the idle Dev app, its newer November 7 signed bundle was preserved in the task's private artifact directory. The canonical installer then completed successfully from a fresh Apple Terminal instance. The installed bundle passed the strict deep seal check, and the installed-binary hotkey diagnostic returned exit 0 with `input_monitoring_granted: true` and `CGEventTap started successfully for this process identity.` This proves the event-tap diagnostic, not actual shortcut handling, microphone authorization, or destination insertion.

| Installed-app evidence | Observed value |
| --- | --- |
| Runtime source | `6534df23b044884e1c211da41f32b154037e5df3` |
| Installed path | `~/Applications/Minutes Dev.app` |
| Identifier | `com.useminutes.desktop.dev` |
| Authority / TeamIdentifier | `Apple Development: Mathieu Silverstein (97C8UF8QVF)` / `63TMLKT8HN` |
| Installed executable SHA-256 | `946578c8aa667375dea8f0d8ffb6b49875ea02c679f81a00563e325ade1baa8b` |
| Installed executable modification time | `2026-11-09T05:13:50.188936Z` |
| Fresh isolated launch | PID `82539`, started `2026-11-09T05:14:10Z` |
| Strict signature seal / native hotkey probe | Passed / exit 0 |

The acceptance harness uses a separate config/data root, an exact pinned virtual input (`MMAudio Device`), a dedicated TextEdit document, and a unique shortcut. A private playback helper verified per-process routing of the repository fixture to that virtual output while leaving the system default output unchanged. The signed app preloaded the existing Tiny model in about 1.34–1.37 seconds across the three isolated launches. That is a diagnostic preload duration, not end-to-end startup performance.

No dictation capture was established by the injected shortcut attempts (first F12, then K, each with Control/Option/Shift). A separate unmodified injected key changed the owned TextEdit field and was confirmed by AX readback; this does not prove that the OS dispatched the registered global chord. The initial foreground request also needed a retry before the driver independently verified the exact target. Actual shortcut handling, microphone capture, cancellation, and insertion remain unresolved. The app's screen-share protection produced a black app-window screenshot; its AX tree was inspectable, and the dedicated TextEdit screenshot rendered normally. No permission toggles or input-driver changes were made.

At Mat's coordination request, native QA was released to the Silverbook lane qualifying PR #1319. The isolated profile reported recording and processing false, no capture PID marker existed, the custom Dev process (last PID `93287`) quit, and the task's native-driver session ended. The installed Dev bundle at handoff was this candidate; the normal user config/data and production app were not installation targets. Further native installs/launches wait for explicit handback from the coordinating lane.

Raw public-fixture Mac replay pairs are checked in as [Tiny](fixtures/dictation-final-reuse-mac-tiny-2026-11-06.json) and [Small](fixtures/dictation-final-reuse-mac-small-2026-11-06.json). Private signing/runtime receipts remain in `~/.codex/artifacts/minutes-dictation-reuse-mac-20261106/` on the Mac, with retrieved evidence also preserved on the build host. `minutes-n843` remains open for installed-app behavior and useful natural hit-rate evidence. The default remains off.

## Final local checks

| Check | Result |
| --- | --- |
| Core library, no default features, serial | 2,167 passed; 4 ignored |
| Feature execution profile `speech` | 143 passed; 6 ignored |
| Explicit installed-model final-reuse regression | 1 passed; no ignored tests in that invocation |
| Strict core Clippy, `pocketstation-capture,whisper`, all targets | Passed |
| Core check, `streaming,engine-sherpa` | Passed |
| CLI builds with Whisper and without default features | Both passed |
| Coverage auditor tests and feature mapping audit | 16 tests passed; mapping audit passed (PyYAML supplied through an isolated uv environment) |
| Rust formatting, diff checks, versions, generated agent docs | Passed; generated site test-count drift is explicitly non-failing in the existing checker |

The model-dependent regression remains ignored in ordinary CI because it requires an explicit installed model. It was executed separately here. A signed installed Mac build, fresh-process verification, native event-tap diagnostic, and Metal replay were completed as described above. Actual shortcut behavior, app microphone capture, physical key-release timing, destination insertion, real-speaker accuracy, natural reuse hit rate, and battery measurement remain unverified. No default promotion or release follows from these checks.
