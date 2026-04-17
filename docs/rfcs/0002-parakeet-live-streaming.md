# RFC 0002: Parakeet support for standalone live transcript

- **Status**: Draft
- **Authors**: @silverstein
- **Related**: `docs/PARAKEET.md#scope`, `docs/designs/parakeet-warm-server-sidecar-2026-04-14.md`, `docs/designs/parakeet-perf-2026-04-14.md`
- **Created**: 2026-04-17

## Summary

Wire the Parakeet transcription engine into the standalone live transcript path (`minutes live` and desktop Live Mode) so that users with `engine = "parakeet"` get parakeet-produced JSONL lines instead of a silent whisper fallback. Reuse the existing warm `example-server` socket and mirror the accumulator pattern already shipping in the recording sidecar. Defer dictation to a follow-up RFC.

## Motivation

`docs/PARAKEET.md#scope` documents a real gap: `engine = "parakeet"` is honored for batch transcription, folder-watcher memos, and the recording-sidecar live path, but NOT for standalone live (`minutes live`) or dictation. Users who configure parakeet and run `minutes live` silently get whisper instead, with a `tracing::warn!` and no user-visible signal that the engine choice was ignored.

This gap exists because:

1. Parakeet landed incrementally: batch first, then warm sidecar, then recording-sidecar live.
2. Standalone live was deferred pending the warm socket.
3. The warm socket landed (`d0c13b3`, `parakeet_sidecar.rs`) and the perf investigation (`docs/designs/parakeet-perf-2026-04-14.md`) validated ~12.6× realtime on tdt-600m fp32, but the live-transcript integration was never cut.

Parakeet users today have one of three suboptimal experiences:

- They set `engine = "parakeet"` thinking it covers their live coaching workflow, and silently get whisper without noticing.
- They run `minutes record` (which now uses parakeet via the recording sidecar after `afdbf91`) but can't use `minutes live` with parakeet.
- They accept the whisper fallback as a "known limitation" and forgo the accuracy advantage (tdt-600m matches whisper large-v3 at 14× fewer parameters).

With the warm sidecar infrastructure in place, the remaining work is mechanical: reuse the recording-sidecar's VAD-gated accumulator pattern in the standalone live path.

## Non-goals

- Dictation (`dictation.rs`). Dictation emits mid-utterance rolling partials via `DictationEvent::PartialText` (for the overlay typing effect). That requires re-transcribing a growing buffer on a 2-second cadence against the sidecar socket, which is a meaningfully different design question. Deferred to a future RFC.
- Mid-utterance partials in the JSONL itself. The current standalone live path calls `StreamingWhisper::feed` every 2s but discards the partial (`live_transcript.rs:577-579`). JSONL only receives one line per VAD-gated utterance. This RFC preserves that contract for parakeet; it does not introduce rolling partials to the JSONL.
- Changes to the warm sidecar protocol, socket location, or lifecycle. This RFC consumes the existing `transcribe_via_global_sidecar` / `warmup_global_sidecar` APIs as-is.
- Cross-engine fallback during a live session. The recording-sidecar implementation already falls back to whisper on parakeet failure mid-session; this RFC mirrors that behavior.

## Prior art within this repo

- **Recording sidecar parakeet path** (`live_transcript.rs:875-896`, commit `afdbf91`). VAD-gated accumulator that writes a temp WAV on utterance-end and routes through `crate::transcribe::transcribe()`, which itself routes to the warm sidecar if `parakeet_sidecar_enabled = true`. This is the pattern this RFC ports into the standalone path.
- **Warm sidecar design** (`docs/designs/parakeet-warm-server-sidecar-2026-04-14.md`). Full lifecycle: lazy spawn, Unix socket, stderr ring buffer, startup health check, per-process child with `~/.minutes/tmp/parakeet-sidecar-<pid>-<model>.sock`, graceful shutdown.
- **Warmup helper** (`transcription_coordinator.rs:378` — `warmup_active_backend`). Already handles both sidecar-enabled and subprocess-enabled cases; this RFC calls it at live-session start to amortize first-utterance cost.

## Current state

`live_transcript::run` (the standalone path, `live_transcript.rs:470-620`):

1. Loads a whisper context unconditionally.
2. Uses `StreamingWhisper::feed` every audio chunk (re-transcribes accumulated buffer every 2s, discards result).
3. Uses `StreamingWhisper::finalize` on VAD-end and writes one JSONL line per utterance.

`run_sidecar_inner_mpsc` (the recording-sidecar path, `live_transcript.rs:937+`):

1. Loads a whisper context for fallback.
2. Checks `recording_sidecar_supports_parakeet(engine)` at startup → sets `parakeet_live_enabled`.
3. On speech: either appends to `parakeet_utterance_samples` (parakeet) OR feeds to `StreamingWhisper` (whisper).
4. On VAD-end or max-utterance: either calls `transcribe_with_parakeet_for_live_sidecar` (parakeet) OR `streaming.finalize()` (whisper). On parakeet failure, logs, flips `parakeet_live_enabled = false`, emits scope warning, and falls back to whisper for the remainder of the session.

The scope warning (`PARAKEET_RECORDING_LIVE_SCOPE_WARNING`) currently fires whenever `engine = "parakeet"` AND parakeet support is not compiled in. After this RFC, the same message becomes accurate for both paths, so the constant name drops the `RECORDING_` prefix.

## Design

### Runtime engine dispatch in `live_transcript::run`

Mirror `run_sidecar_inner_mpsc` structure in `run`:

1. Add `parakeet_live_enabled` boolean at session start, gated on `cfg(feature = "parakeet")` and `config.transcription.engine == "parakeet"`.
2. Add `parakeet_utterance_samples: Vec<f32>` as the accumulator (feature-gated).
3. On VAD-gated speech: dispatch to parakeet accumulator or whisper streaming.
4. On VAD-end or max-utterance: dispatch to `transcribe_with_parakeet_for_live_sidecar` or `streaming.finalize`.
5. On parakeet failure: flip the flag, log, emit the scope warning, fall back to whisper for the remainder.

### Shared helper extraction

`transcribe_with_parakeet_for_live_sidecar` and `transcribe_with_whisper_for_live_sidecar` are currently `#[cfg(feature = "parakeet")]`-gated and sit above `run_sidecar_inner_mpsc`. They already operate on `&[f32]` samples + `&Config` (parakeet) or samples + `&WhisperContext` + language (whisper). Both callable from `run` as-is. No extraction needed — same function, two callers.

### First-utterance warmup

The recording sidecar today lets the first utterance pay the full sidecar startup cost (spawn subprocess, load model, wait for socket). That's acceptable because recordings typically run for minutes; the first VAD-end utterance takes longer but subsequent ones are fast.

For standalone live, users often start `minutes live`, speak one short sentence, and expect a JSONL line quickly. To avoid a 10-30s wait on first utterance:

- At session start, if `engine == "parakeet"` and the parakeet feature is enabled, call `transcription_coordinator::warmup_active_backend(config)` before entering the main loop.
- If warmup fails, log and fall back to whisper immediately (don't block the session).
- Log warmup elapsed_ms for operator visibility.

This is additive: existing whisper sessions are unchanged.

### Error handling and fallback semantics

Match the recording-sidecar's approach exactly:

- Parakeet failure on a single utterance → log error, flip `parakeet_live_enabled = false`, emit the runtime-fallback warning to stderr, re-attempt the same utterance through whisper so the JSONL line still lands.
- Subsequent utterances in the same session use whisper until restart.
- No automatic recovery within the session — operator restarts the session to retry parakeet. This matches the recording-sidecar behavior and keeps the implementation simple.
- **Warmup failure is advisory, not fatal.** If `warmup_active_backend` returns an error at session start, we log it and continue with `parakeet_live_enabled = true`. The per-utterance `crate::transcribe::transcribe()` path already falls back sidecar→subprocess gracefully, so forcing whisper here would be strictly worse than the per-utterance degradation path. Each utterance still gets a chance to succeed via the parakeet binary.
- **Warmup is only called when the sidecar is enabled.** When `parakeet_sidecar_enabled = false`, warmup would be a throwaway subprocess invocation that leaves no hot backend behind, so we skip it entirely to avoid a 4-5s startup stall for no benefit.

### Warning surfaces

Two distinct user-visible messages, each with a clear trigger:

**Scope warning** (`PARAKEET_LIVE_SCOPE_WARNING`): fires at session start when the user configured `engine = "parakeet"` but the binary was built without the `parakeet` Cargo feature.
- Message: `"this build does not include parakeet; live transcription uses whisper (see docs/PARAKEET.md#scope)"`
- Source field: `"standalone"` or `"recording-sidecar"`

**Runtime fallback warning** (`PARAKEET_LIVE_FALLBACK_WARNING`): fires when the parakeet engine IS compiled in but fails mid-session (transcribe error). The session transparently switches to whisper for the remainder.
- Message: `"parakeet live transcription failed; falling back to whisper for this session (see docs/PARAKEET.md#scope)"`
- Source field + detail: identifies which path failed and why

Both write to stderr, tracing, and the JSON structured log.

### Silent transcript corruption on reconnect — fixed

The initial implementation did not reset the utterance accumulator on device reconnect / stream disconnect, which would have spliced pre-reconnect and post-reconnect audio into one JSONL line. Codex caught this during adversarial review. Current behavior: on successful reconnect, discard any partial utterance (both the whisper `StreamingWhisper` buffer and the parakeet `Vec<f32>` accumulator), log `samples_discarded = N`, and continue cleanly. Rationale: pre-reconnect audio is unreliable when the mic drops out mid-speech.

### Minimum utterance length

Sub-1s VAD blips are dropped before reaching the parakeet sidecar/subprocess, matching `StreamingWhisper::MIN_TRANSCRIBE_SAMPLES` (16,000 samples = 1 second at 16kHz). This avoids temp-file churn and latency spikes on noisy inputs. Exposed as `PARAKEET_LIVE_MIN_SAMPLES` for test assertions.

## Implementation plan

### Phase 1: standalone live parakeet (this RFC)

- `crates/core/src/live_transcript.rs`:
  - Extend `run` with parakeet dispatch mirroring `run_sidecar_inner_mpsc`.
  - Rename `PARAKEET_RECORDING_LIVE_SCOPE_WARNING` → `PARAKEET_LIVE_SCOPE_WARNING` and update message.
  - Call `warmup_active_backend` at session start when `engine == "parakeet"`.
- `crates/core/src/transcription_coordinator.rs`: no code change; `warmup_active_backend` already handles both sidecar and subprocess lanes.
- `docs/PARAKEET.md`: update `## Scope` section to list standalone live as parakeet-wired. Keep dictation listed as whisper-only.
- `CLAUDE.md`: no change. The architecture description already covers the shared helper pattern.

### Phase 2: dictation (future RFC)

Dictation needs rolling mid-utterance partials for the overlay typing effect (`DictationEvent::PartialText`). Options sketched, not selected:

- Option A: add a `StreamingParakeet` that re-sends the growing buffer to the warm socket every 2s. Socket traffic ~2× per session vs current, acceptable given warm-socket per-call is ~240ms for a 3s buffer.
- Option B: degrade UX in dictation — no mid-utterance partials on parakeet, overlay shows "Transcribing..." until VAD-end, then the full utterance appears. Simpler but visibly worse UX.
- Option C: parakeet for final-text write, whisper tiny for mid-utterance partials only. Dual-engine session. Hybrid complexity.

Defer until live-transcript parakeet is shipping and we have operator signal on which option users accept.

## Test plan

Unit tests (in `live_transcript.rs` test module):

- `standalone_run_uses_parakeet_when_engine_parakeet_and_feature_enabled` — assert dispatch path via instrumented helper (mock `transcribe_with_parakeet_for_live_sidecar`).
- `standalone_run_falls_back_to_whisper_on_parakeet_failure` — inject failure, assert `parakeet_live_enabled` flips, assert whisper path produces the line.
- `standalone_run_without_parakeet_feature_emits_scope_warning` — assert warning fires once, not per-utterance.

Integration test (`tests/integration/`):

- `live_parakeet_smoke.rs` (feature-gated on `parakeet`) — spawn `minutes live`, feed a known WAV, assert JSONL contains parakeet-produced text (marker: parakeet-specific tokenization differences).

Smoke test (manual, documented in notes file):

1. `MINUTES_PARAKEET_SERVER_BINARY=/path/to/example-server`
2. `parakeet_sidecar_enabled = true` in config
3. `cargo run --release -p minutes-cli --features parakeet -- live`
4. Speak 2-3 utterances
5. Assert JSONL lines appear, check `~/.minutes/logs/minutes.log` for `parakeet-sidecar: using warm server path` entries

## Rollout

No feature flag. Controlled by existing `transcription.engine` and `transcription.parakeet_sidecar_enabled` config knobs.

Default behavior unchanged:

- `engine = "whisper"` (default) → whisper, same as today
- `engine = "parakeet"` + parakeet feature compiled in → parakeet on both record and live
- `engine = "parakeet"` + parakeet feature NOT compiled in → whisper with one-time scope warning per session
- `engine = "parakeet"` + `parakeet_sidecar_enabled = false` → per-utterance subprocess (slow; likely unusable for live; same code path as batch today). Documented as "works but not recommended for live — enable the sidecar."

Release note language: "`minutes live` and desktop Live Mode now honor `engine = "parakeet"`. If you've been setting `engine = "parakeet"` and wondering why live mode felt different from record mode, this closes the gap. Enable `parakeet_sidecar_enabled = true` for acceptable latency."

## Open questions (resolved during implementation + adversarial review)

1. **Sidecar contention between concurrent sessions.** RESOLVED. Existing mutual-exclusion code in `pid.rs` and `cmd_record` / `cmd_live` handlers blocks simultaneous record + live in the same user session, so the multi-sidecar scenario cannot arise through the CLI. Each process still gets its own per-PID socket (`parakeet-sidecar-<pid>-<model>.sock`), so cross-process contention would be memory-bounded, not racy. Not a correctness issue.

2. **Warmup timeout.** RESOLVED. We emit `[minutes] Warming parakeet sidecar... (first-run cold start can take 10-30s)` on stderr before calling `warmup_active_backend`, then `[minutes] parakeet sidecar ready (Nms)` on success or `[minutes] parakeet sidecar warmup failed (...)` on failure. No config knob needed. If a user hits repeated warmup timeouts, the per-utterance subprocess fallback still works.

3. **Short utterances (<1s).** RESOLVED. Applied a 1s floor (`PARAKEET_LIVE_MIN_SAMPLES = 16_000`) in `transcribe_with_parakeet_for_live_sidecar`. Sub-1s buffers return `Ok(None)` without writing a temp WAV or spawning a subprocess. Matches whisper behavior. Two unit tests cover the floor (`parakeet_live_helper_drops_subsecond_utterances`, `parakeet_live_helper_threshold_edges`).

4. **fp16 reactivation.** UNCHANGED. The warm sidecar's fp16 crash (`docs/designs/parakeet-perf-2026-04-14.md:111`) is gated by `parakeet_fp16_blacklist` in `parakeet_sidecar.rs`. Live mode inherits that blacklist automatically. Nothing to do in this RFC.

## Adversarial review findings (codex + Claude)

Summary of issues found and resolved before landing:

| Finding | Source | Severity | Resolution |
|---------|--------|----------|-----------|
| Warmup failure forced whole-session whisper fallback even when per-utterance subprocess would have worked | codex | blocker (regression) | Made warmup advisory; `parakeet_live_enabled` stays `true` on warmup error so per-utterance code can try |
| Accumulator not cleared on device reconnect → silent transcript corruption | codex | blocker | Clear both whisper and parakeet buffers on successful reconnect, log `samples_discarded` |
| `finalize_live_utterance` returned `false` on write failure but 4 shutdown sites discarded the return value → silent data loss | codex | blocker | Added `finalize_on_exit` wrapper that logs `tracing::error!` on write failure even during shutdown |
| Scope-warning function called on runtime fallback was silently suppressed when parakeet feature was compiled in | codex + Claude | important | Split into `PARAKEET_LIVE_SCOPE_WARNING` (compile-time) and `PARAKEET_LIVE_FALLBACK_WARNING` (runtime); both reach stderr |
| Warmup ran even when `parakeet_sidecar_enabled = false`, producing a useless 4-5s stall | codex | important | Gated warmup on `parakeet_sidecar_enabled = true` |
| RFC promised user-visible "Warming parakeet sidecar..." stderr line; code didn't emit it | codex | drift | Implemented on stderr before + after warmup |
| RFC promised 1s minimum-audio floor; code did not apply it | codex | drift | Added `PARAKEET_LIVE_MIN_SAMPLES = 16_000` + two tests |
| Fallback path had zero test coverage | Claude | important | Added `scope_and_fallback_warnings_are_distinct_messages`, `parakeet_live_helper_drops_subsecond_utterances`, `parakeet_live_helper_threshold_edges` |
| Docs presented parakeet live as unconditional once feature is on, but still requires whisper feature for runtime fallback | codex | doc drift | Added note to `docs/PARAKEET.md#scope` clarifying whisper is required |

Net result: the implementation is stricter about silent-failure modes than the recording-sidecar pattern it was originally copied from — and the recording-sidecar itself was patched to use the new `emit_live_engine_fallback_warning` for its runtime fallback sites.

## Acknowledgments

- The warm sidecar design (`docs/designs/parakeet-warm-server-sidecar-2026-04-14.md`) and perf benchmark (`docs/designs/parakeet-perf-2026-04-14.md`) did the hard research. This RFC is a mechanical extension of that work to the standalone live path.
- The recording-sidecar parakeet wiring (`afdbf91`) proves the accumulator pattern works end-to-end.
