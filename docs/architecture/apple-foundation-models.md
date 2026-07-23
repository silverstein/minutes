# Apple Foundation Models Summarization

Minutes can summarize meetings fully on-device using Apple's Foundation
Models framework (the local Apple Intelligence model) on macOS 26+. This is
the first summarization engine that fills in summaries, action items, and
decisions with **zero setup, zero API keys, and zero network traffic**.

## Requirements

- macOS 26 (Tahoe) or newer
- Apple Intelligence enabled in System Settings
- Apple Silicon
- Xcode Command Line Tools (for the one-time `swiftc` helper compile)

## How it works

The implementation mirrors the `apple-speech` helper pattern:

1. `crates/core/resources/apple-fm-helper.swift` is embedded in the binary
   at compile time (`include_str!`).
2. On first use, minutes-core writes it to `~/.minutes/lib/` and compiles it
   once with `swiftc` into `~/.minutes/bin/apple-fm-helper` (0700).
3. The helper exposes two commands with a JSON-on-stdout contract:
   - `capabilities` → availability report (schemaVersion 1)
   - `generate --input-file <path>` → generation result
4. Prompts travel via a 0600 temp file, never argv, so transcript content
   cannot appear in the process list.
5. `crates/core/src/apple_fm.rs` wraps this with a cached availability probe
   (`is_available()`) and a `generate()` call with a 240s timeout.

On macOS versions older than 26 (or with Apple Intelligence disabled) the
helper still compiles and reports unavailability cleanly; Minutes falls back
per the engine rules below. There is no network path anywhere in this module.

## Engine selection

```toml
[summarization]
engine = "apple"     # explicit: Apple Foundation Models only
# or
engine = "auto"      # prefers Apple FM when available, else agent CLI, else none
```

- `engine = "apple"` (alias `"apple-fm"`): use Foundation Models; errors
  surface as processing warnings if unavailable.
- `engine = "auto"`: privacy-first ordering — **Apple FM first** (on-device),
  then an installed agent CLI (claude/codex/gemini/opencode/agent, which round-trips
  through that provider's cloud), then skip.
- The compiled default remains `engine = "none"` (no summarization).

Title refinement and the summarize path both honor the engine; model hints
report as `apple:foundation-models` in processing logs and frontmatter.

## Context window

Foundation Models exposes a ~4k-token context window. Chunking caps at
`APPLE_FM_MAX_CHUNK_TOKENS` (3000) regardless of `chunk_max_tokens`, using
the same map-reduce flow as the other engines.

## Testing

The subprocess contract is unit-tested on every platform via the
`MINUTES_APPLE_FM_HELPER` env override, which points resolution at a stub
script (see `apple_fm.rs` tests). Real end-to-end generation requires
hardware with Apple Intelligence — see the dogfood checklist below.

## Dogfood checklist (macOS 26 hardware)

1. `cargo build --release -p minutes-cli` and install per CLAUDE.md.
2. Set `engine = "apple"` in `~/.config/minutes/config.toml`.
3. `minutes process <some .wav>` — first run compiles the helper (a few
   seconds), then summarization runs locally. Verify `summary`,
   `action_items`, and `decisions` frontmatter are populated.
4. Check `~/.minutes/logs/minutes.log` for `apple:foundation-models` model
   hints and no network egress.
5. Try `engine = "auto"` with no agent CLI on PATH and confirm Apple FM is
   selected.
