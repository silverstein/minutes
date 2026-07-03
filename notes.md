# v0.12.2: Live transcription that actually works on quiet audio

## Why this release exists

A meeting today produced 11 fragments over 40 minutes of live transcript — mostly whisper placeholder tokens like `[typing]` and `[BLANK_AUDIO]`. Meanwhile the post-recording batch transcript cleanly recovered 2,259 words from the same WAV. Same audio, two different code paths, 200x quality gap.

Three problems were working against live mode:

1. The recording sidecar used a simple energy-threshold VAD. Quiet audio (-44 dB RMS when you're on headphones and the other person barely bleeds through) fell below the threshold and got gated out before whisper ever saw it.
2. `engine = "parakeet"` in the config was effectively a lie. It controlled the post-recording batch path but live-during-record was hard-coded to whisper.
3. Whisper's quiet-audio placeholder tokens (`[typing]`, `[Musik]`, `[BLANK_AUDIO]`) were flowing straight into the live JSONL output, trashing agent context.

v0.12.2 fixes all three.

## Fixes

- **Silero VAD in the recording sidecar.** Same ML-based model the batch path uses, already on disk from `minutes setup`. Graceful fallback to the old energy VAD if Silero initialization fails. Adds gating stats to the finalize log so you can see how many samples were fed vs gated.
- **`engine = "parakeet"` now applies to live transcription during `minutes record`.** Per-utterance dispatch to Parakeet through the existing `crate::transcribe::transcribe` entry point, which reuses the warm sidecar socket when `parakeet_sidecar_enabled = true`. Mid-session Parakeet failures flip the session back to whisper with a clear warning instead of silently breaking.
- **Whisper placeholder tokens no longer make it into live JSONL.** The streaming write path now routes text through `whisper-guard`'s `collapse_noise_markers` and `strip_foreign_script` filters before writing. Empty results are dropped instead of producing blank utterances.
- **Clear docs on which code paths use which engine.** `docs/PARAKEET.md` gains a Scope section listing exactly where Parakeet is wired today: batch transcription, folder watcher memos, and recording-sidecar live transcription. Standalone `minutes live` and dictation still use whisper and are documented as such.

## For users on v0.12.1

If you set `engine = "parakeet"` and tried live mode, you got whisper with a weak VAD. This release is the fix. The desktop app auto-update and `brew upgrade silverstein/tap/minutes` will pick it up.

## Install

**CLI:**
```bash
brew upgrade silverstein/tap/minutes
# or
cargo install minutes-cli
```

**MCP server (zero-install):**
```bash
npx minutes-mcp@latest
```

**Desktop app:** download the DMG from [useminutes.app](https://useminutes.app) or let the auto-updater pick it up.

**Claude Desktop:** grab `minutes.mcpb` from this release and drag it into Claude Desktop's Extensions settings.
