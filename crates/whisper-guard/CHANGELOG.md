# Changelog

All notable changes to whisper-guard will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.0] - unreleased

### Added

- **`clean_segments(&[String]) -> (Vec<String>, CleanStats)`** - the new entry
  point for callers who have raw transcript segments (e.g. straight from
  `whisper_state.get_segment(i).to_str()`). No timestamp wrapping required.
  This is now the recommended entry point for fork-using consumers and any
  caller that hasn't already serialized to a timestamped string.
- **`clean_segments_with_options(&[String], &CleanOptions)`** - same as above
  with caller-controlled per-pass toggles.
- **`CleanOptions`** struct with bool fields for each cleaning pass. Defaults
  match the production tuning used by Minutes. `CleanOptions::all()` /
  `CleanOptions::none()` constructors for convenience.
- **`CleanOptions::keep_dedup_annotations`** - controls whether the
  `[...] [repeated audio removed - N identical segments collapsed]` placeholder
  lines are emitted. Default `true` (preserves the human-readable trail). Set
  to `false` for clean output streams headed to an LLM, joined into a flat
  string, or otherwise consumed by code rather than read by a human.
- **`CleanStats::summary()`** - compact human-readable one-liner for logging
  (`"whisper-guard: 47 → 32 segments (15 removed)"`).
- **`serde` Cargo feature** - adds `Serialize`/`Deserialize` derives to
  `CleanStats` and `CleanOptions` so they can flow through serde-based
  logging, config files, IPC, etc. Off by default (zero new dependencies for
  consumers who don't need it).
- **README.md** - full crate documentation, install instructions for both
  default-features and fork-friendly setups, before/after hallucination
  examples, and pointers to runnable examples.
- **CHANGELOG.md** - this file.
- **`examples/raw_segments.rs`** - end-to-end demonstration of the common
  fork-user call site. Runnable via `cargo run --example raw_segments`.
- **`examples/with_options.rs`** - advanced configuration showing how to
  disable specific guards.
- **`tests/clean_segments_integration.rs`** - public-API integration tests
  covering the fork-user path, opt-out behavior, idempotency, and pathological
  100k-segment inputs.

### Changed

- **`clean_transcript` now delegates to `clean_segments` internally** - no
  behavior change for callers, but the cleaning pipeline now lives in exactly
  one place. Future pass additions automatically apply to both entry points.
- Top-level rustdoc rewritten to lead with the `Vec<String>` use case (more
  common in practice) and to call out the fork-friendly `default-features = false`
  install path explicitly.
- `CleanStats` doc clarified: when a pass is disabled via `CleanOptions`, its
  `after_*` field carries the count from the previous (enabled) pass - making
  pass-level deltas safe to compute without checking which passes ran.
- `CleanStats::lines_removed` doc clarified: this is the **net** segment count
  delta (`original_lines - after_command_strip`), not the raw count of input
  lines that were dropped. The dedup pass inserts an annotation line per
  collapsed run, so the net change can be smaller than the raw drop count.
  Suppress annotations via `keep_dedup_annotations: false` for raw
  "input minus output" semantics.

### Behavior worth knowing (carried from 0.1.x or new)

- **Filler-word floor at 5+.** Single trailing fillers (`Yeah.`, `Okay.`, `You.`)
  are preserved as legitimate one-word closings. Always-noise tokens have no
  floor (new in 0.2.0).
- **Foreign-script filter is majority-rule (≥70%).** Mixed-script content
  survives when no single script dominates 70%+.

### Fixed

- **Trailing always-noise tokens (`[music]`, `[blank_audio]`, `[silence]`,
  `music`) are now trimmed at any count.** Previously a 5-line floor applied
  to all noise patterns; the floor protected legitimate single-word filler
  closings (`Yeah.`, `Okay.`, `You.`) but unnecessarily preserved bracketed
  noise tokens that are never legitimate transcript content. The split is now
  explicit - see `is_always_noise` vs filler check in `trim_trailing_noise`.
- **Trailing noise hidden behind a voice command is now cleaned up.** Pipeline
  order changed: `strip_trailing_commands` runs before `trim_trailing_noise`,
  exposing markers that were previously stranded behind commands like
  `"…[music] [music] Stop recording."`. Combined with the next fix this means
  the full sequence is reliably stripped.
- **`dedup_consecutive` now skips always-noise tokens.** Previously a run of
  identical `[music]` markers would be collapsed to `[music] + annotation` by
  consecutive-dedup, which then prevented the trim pass from cleaning them up.
  Always-noise tokens now flow past dedup unchanged so the noise-aware passes
  (`trim_trailing_noise`, `collapse_noise_markers`) can handle them as a class.
- **`collapse_noise_markers` now runs LAST in the pipeline.** Previously it ran
  before trim, which converted trailing noise runs into `marker + annotation`
  and blocked trim. Now trim has first crack at trailing noise; whatever
  survives in the middle gets collapsed.

### Pipeline order (0.2.0)

The full order, with the rationale, is documented in the `CleanOptions` rustdoc
and the `clean_segments_with_options` source. Summary:

```
1. dedup_consecutive  (skips always-noise tokens)
2. dedup_interleaved
3. strip_foreign_script
4. strip_trailing_commands  ← was 6
5. trim_trailing_noise      ← was 5
6. collapse_noise_markers   ← was 4
```

`CleanStats` field names map to the pass that produced them, NOT to chronological
position. So `after_command_strip` is the count after command-strip ran (which is
now mid-pipeline), regardless of where it sits in the struct declaration. The
final segment count is `after_noise_markers` (the last pass that runs).

### Notes for upgraders

This release is **fully backwards-compatible at the API level**. `clean_transcript`,
`CleanStats`, and all individual pass functions keep their existing signatures.
The new `clean_segments` API is additive. Existing 0.1.x callers can upgrade
to 0.2.0 without any code changes.

**Behavior changes** (output may differ from 0.1.x in these specific cases):
- Short trailing always-noise runs (1–4 `[music]`/`[blank_audio]`/etc.) that
  previously survived are now trimmed.
- Noise hidden behind a trailing voice command is now cleaned up.
- `CleanStats` field ordering vs pipeline ordering is no longer aligned;
  delta arithmetic between adjacent fields may now produce negative numbers.

### Stability policy

- `CleanStats` is `#[non_exhaustive]`. New stats fields can be added in any
  release without breaking pattern-matching consumers - but you'll need to
  use `..` if you destructure.
- `CleanOptions` is **not** `#[non_exhaustive]`, on purpose. Functional record
  update (`..CleanOptions::default()`) is the primary ergonomic pattern for
  this struct. New `CleanOptions` fields will be added as minor-version bumps
  with an entry in this file. Defensive callers should always use
  `..CleanOptions::default()` to insulate themselves.

## [0.1.2] - 2026-04-02

- Bumped for crates.io publish.
- Includes language-agnostic noise marker collapse (`[Śmiech]`, `[música]`,
  `[risas]`, etc.).
- Foreign-script hallucination rejection on low-signal audio.
- `whisper-rs` integration via optional `whisper` feature.

## [0.1.1] - 2026-03-27

- Guard against `sample_rate=0` panic in audio processing.

## [0.1.0] - 2026-03-27

- Initial extraction from minutes-core.
- Six-layer hallucination defense: consecutive dedup, interleaved A/B/A/B
  detection, foreign-script filter, noise marker collapse, trailing trim,
  voice command strip.
- Audio module: silence stripping, normalization, windowed-sinc resampling.
- `clean_transcript(&str) -> (String, CleanStats)` entry point.

[0.2.0]: https://github.com/silverstein/minutes/releases/tag/whisper-guard-v0.2.0
[0.1.2]: https://github.com/silverstein/minutes/releases/tag/whisper-guard-v0.1.2
[0.1.1]: https://github.com/silverstein/minutes/releases/tag/whisper-guard-v0.1.1
[0.1.0]: https://github.com/silverstein/minutes/releases/tag/whisper-guard-v0.1.0
