# whisper-guard

[![Crates.io](https://img.shields.io/crates/v/whisper-guard.svg)](https://crates.io/crates/whisper-guard)
[![Docs.rs](https://docs.rs/whisper-guard/badge.svg)](https://docs.rs/whisper-guard)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](#license)

> The post-processing layer Whisper should have shipped with.

Whisper is excellent at speech recognition and notorious for hallucinating: looping
on silence, generating phantom `[music]` tags, drifting into a foreign script when
the audio is too quiet, gluing voice commands like *"stop recording"* onto the end
of every transcript. **whisper-guard** catches the common patterns, with defaults
tuned in production by [Minutes](https://github.com/silverstein/minutes), an OSS
meeting-memory tool that processes meeting and voice-memo audio across multiple
languages.

```rust
use whisper_guard::clean_segments;

let raw = vec![
    "Thank you.".to_string(),
    "Thank you.".to_string(),
    "Thank you.".to_string(),
    "Thank you.".to_string(),
    "What's the budget for Q3?".to_string(),
];

let (cleaned, stats) = clean_segments(&raw);
// Real content survives; the "Thank you." loop collapses to one occurrence
// plus an annotation line marking what was removed (see "Annotation lines"
// below).
assert!(cleaned.iter().any(|s| s.contains("budget")));
println!("{}", stats.summary());
// → whisper-guard: 5 → 3 segments (2 removed)
```

> **Note on `lines_removed`:** this is the **net** segment-count delta
> (`original_lines - after_command_strip`), not the raw count of input lines
> that got removed. The dedup pass inserts a single annotation line in place
> of each collapsed run, so when 5 inputs collapse to 1 line plus 1
> annotation, the net change is `5 - 2 = 3`. Use [`CleanOptions::keep_dedup_annotations`]
> to suppress those annotations entirely if you want raw "input minus output"
> semantics.

That's the whole API for the common case. No builders, no setup, no engine
coupling. Six guards run in a fixed order; opt out individually via `CleanOptions`
when you have a good reason.

### Annotation lines

When the consecutive-dedup pass collapses a hallucination loop, it leaves a single
annotation line in place of the removed run:

```text
Thank you.
[...] [repeated audio removed - 3 identical segments collapsed]
What's the budget for Q3?
```

This is intentional - readers can see at a glance that something was stripped
without losing the fact that there *was* audio there. If you want to drop the
annotation entirely (e.g. for downstream LLM input), filter the cleaned segments
yourself with `s.starts_with("[...] [repeated audio removed")` after the call.

## Why this crate exists

Whisper's decoder has well-known failure modes that param tuning alone can't fix:

| Hallucination pattern | What you see | What whisper-guard does |
|---|---|---|
| **Silence loop** | `"Thank you. Thank you. Thank you. Thank you."` (10–50x) | Collapses runs of 3+ similar real-content segments, leaves an annotation line |
| **A/B/A/B drift** | `"Yeah. So. Yeah. So. Yeah. So."` | Detects interleaved patterns when one phrase dominates a window |
| **Foreign-script ghost** | English audio → Hindi/CJK/Cyrillic phantom text | Drops segments whose script doesn't match the dominant transcript script (≥70% threshold) |
| **Noise marker accumulation** | `"[music] [music] [music] [Śmiech] [música]"` (mid-transcript) | Collapses runs of bracketed markers across any language |
| **Trailing noise tail** | Real content → `"[music] [music] [music]"` | Trims noise off the end at any count for `[music]` / `[blank_audio]` / `[silence]`; filler words (`yeah.`, `okay.`, `you`) need a 5+ run to trigger |
| **Voice-command glue** | `"…final action item. Stop recording."` | Strips trailing voice commands like `stop recording` / `end recording`, then re-runs trailing-noise trim so noise hidden behind the command is also cleaned |

Setting `entropy_thold`, `logprob_thold`, and `no_speech_thold` on `whisper-rs`
catches some of these at the param layer. None of them catch all. This crate is
what you reach for after the params still aren't enough.

### Behavior worth knowing

- **Filler-word floor.** Single trailing fillers (`Yeah.`, `Okay.`, `You.`) are
  intentionally preserved - they're often legitimate one-word closings.
  Only a 5+ run of filler at the end triggers trim. The bracketed noise tokens
  (`[music]`, `[blank_audio]`, `[silence]`) have no floor - those are never
  legitimate transcript content.
- **Foreign-script filter is majority-rule (≥70%).** Mixed-script transcripts
  where no single script dominates 70%+ will not trigger filtering - so a
  bilingual standup is safe, but a 90%-English transcript with one phantom CJK
  line will drop that line.
- **Pass order is fixed but field names are not chronological.** `CleanStats`
  fields are named after the pass that produced them, not their position in the
  pipeline. So `after_command_strip` measures the count after command-strip ran
  (which is now mid-pipeline), regardless of where it sits in the struct
  declaration.

## Install

```toml
[dependencies]
whisper-guard = "0.2"
```

### Using a forked or pinned `whisper-rs`?

The `segments` and `audio` modules are **pure Rust with no whisper-rs dependency**.
If you need a specific `whisper-rs` revision (common for Metal/CUDA tuning, custom
GPU patches, or model compatibility - looking at you, screenpipe-audio), use:

```toml
[dependencies]
whisper-guard = { version = "0.2", default-features = false }
```

…and the cleaning pipeline works regardless of which `whisper-rs` is in your tree.
The optional `whisper` feature only adds `params` presets that wrap
`whisper_rs::FullParams`.

## Usage

### Cleaning raw segments (the common path)

If you have `Vec<String>` segments straight from a transcription engine - the
output of `whisper_state.get_segment(i).to_str()`, a parakeet sidecar, or any
other source - `clean_segments` is your entry point.

```rust
use whisper_guard::{clean_segments_with_options, CleanOptions};

let segments: Vec<String> = (0..whisper_state.full_n_segments())
    .filter_map(|i| whisper_state.get_segment(i)?.to_str().ok().map(String::from))
    .collect();

// Suppress the dedup annotation lines for clean string output; segments are
// joined directly without `\n` separators, so each segment carries its own
// leading whitespace from whisper's tokenizer.
let opts = CleanOptions {
    keep_dedup_annotations: false,
    ..CleanOptions::default()
};
let (cleaned, _stats) = clean_segments_with_options(&segments, &opts);
let transcript = cleaned.join("");
```

If you'd rather keep the annotation trail for human readability, use the
zero-config `clean_segments(&segments)` - it's the same pipeline with annotations
preserved.

### Cleaning a formatted transcript

If you're already storing transcripts as a single string with timestamped lines
(`[0:00] hello world`), use `clean_transcript`:

```rust
use whisper_guard::clean_transcript;

let raw = "[0:00] Hello world\n[0:03] Hello world\n[0:06] Hello world\n[0:09] Real content";
let (cleaned, stats) = clean_transcript(raw);
println!("Removed {} hallucinated lines", stats.lines_removed);
```

### Disabling specific guards

Defaults are tuned for general speech. Opt out individually when your audio
context calls for it:

```rust
use whisper_guard::{clean_segments_with_options, CleanOptions};

let opts = CleanOptions {
    // Keep the [music] markers - this is a music podcast transcript.
    collapse_noise_markers: false,
    // Keep mixed-script content - this is a bilingual standup.
    strip_foreign_script: false,
    ..CleanOptions::default()
};

let (cleaned, _) = clean_segments_with_options(&segments, &opts);
```

The pass order is fixed (it matters for correctness - for example, foreign-script
filter runs before noise-marker collapse so that hallucinated CJK lines don't
inflate the noise density calculation). See the `CleanOptions` rustdoc for the
full pipeline.

### Audio prep (optional, separate module)

The `audio` module handles common pre-transcription needs - silence stripping,
auto-normalization for quiet microphones, and a windowed-sinc resampler:

```rust
use whisper_guard::{normalize_audio, resample, strip_silence};

let resampled = resample(&samples_44k, 44_100, 16_000);
let normalized = normalize_audio(&resampled);
let speech_only = strip_silence(&normalized, 16_000);
```

### Whisper parameter presets (optional)

Behind the `whisper` feature, `params` exposes preconfigured `FullParams` builders
matching `whisper-cli` defaults plus a streaming-tuned variant. Use these only if
you depend on `whisper-rs` directly.

```toml
[dependencies]
whisper-guard = { version = "0.2", features = ["whisper"] }
```

## Examples

Run the included examples:

```bash
cargo run --example raw_segments     # the common fork-user case
cargo run --example with_options     # advanced: opt out of specific guards
```

## Production usage

whisper-guard is the post-processing layer behind:

- **[Minutes](https://github.com/silverstein/minutes)** - OSS meeting memory.
  Processes thousands of hours of multi-language audio. Origin point of every
  guard in this crate.

Using whisper-guard in production? PR a link here.

## Compatibility

- **Rust**: 1.75+ (MSRV)
- **whisper-rs**: optional dependency at `0.16.x`. Disable defaults to use any
  forked or pinned version.
- **Platforms**: pure Rust, no platform-specific code. Runs everywhere Rust runs.

## License

MIT. See [LICENSE](LICENSE) (or the MIT field in `Cargo.toml` until LICENSE lands).

## Contributing

whisper-guard is developed inside the [Minutes monorepo](https://github.com/silverstein/minutes)
under `crates/whisper-guard/`. Issues, PRs, and ideas go there. New hallucination
patterns are especially welcome - every one we catch is one fewer surprise for
the next consumer.
