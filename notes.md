## What's new

**Dictation that types where your cursor is.** Hold your dictation shortcut, speak, release — the transcript lands directly in whatever app you're in (Notes, Slack, a browser field), not just the clipboard. A quiet pill shows a live waveform while you talk, and it fails honestly: if the mic is dead or Accessibility isn't granted, it says so instead of pretending. Set it under Settings → Dictation ("Where dictated text goes"); the classic copy-to-clipboard mode is still there.

**Your prep notes and drafts are now in the app.** A new Documents pane surfaces every prep, debrief, and draft — including the ones your AI assistant writes — in one place, with a calm read-first / click-to-edit viewer that autosaves. `Cmd/Ctrl+B` collapses the meeting list to give the terminal (or a document) the full window.

**Settings, reorganized.** The settings panel is now five navigable tabs (General, Capture, Transcription, AI & Privacy, Advanced) instead of one long scroll — thanks to @maosuarez.

**Honest degradation, everywhere it matters.** A run of reliability fixes so Minutes never quietly hands you a worse result than it claims:
- Live transcription no longer silently falls back to a different engine and reports "healthy" — if it can't honor your configured engine, it says so.
- A call recording whose remote side wasn't captured is now flagged (degraded status + a clear warning) instead of producing a mic-only transcript that looks complete.
- Experimental VAD/engine paths that fail to initialize now fall back gracefully instead of hanging or crashing a recording.

**Better transcription under the hood.** Sherpa (the newer parakeet-v3 engine) now links statically on macOS and segments on speech boundaries instead of fixed 15-second windows — closing most of its quality gap with the default Parakeet engine. Speaker-attribution "Confirm" buttons in the meeting review now actually work (they relied on a dialog Tauri's webview doesn't support). Entity resolution got a real evaluation harness and safer name-variant merging.

**Windows note.** If `process_audio` hangs for you on Windows, this release adds stage-by-stage diagnostic tracing (`~/.minutes/logs/process-audio-trace.jsonl`) to pinpoint exactly where — see #415.

## Install / update

The desktop app updates itself: open Minutes and it pulls v0.20.0 on next launch, or grab the DMG from the assets below.

- **DMG**: download from the release assets below
- **CLI**: `brew install silverstein/tap/minutes` or `cargo install minutes-cli`
- **MCP**: `npx minutes-mcp` (or update the Claude Desktop extension)

## Claude Code plugin

This release also updates the Minutes plugin (Microsoft 365 / Outlook calendar source for `/minutes-prep`, thanks to the new calendar integration). Plugin updates don't auto-deliver — refresh with:

```
/plugin marketplace update minutes
/plugin update minutes@minutes
```

then restart Claude Code.
