Your AI remembers every conversation you've had. Now it remembers every thought you speak.

**v0.6.0 adds dictation** — hold a key, speak, and your words land in your clipboard AND your AI's memory. Every dictation tool forgets what you said. Minutes remembers it. "What did I dictate into Slack about the pricing page?" just works.

## What's new

### Dictation mode

Speak naturally. Text goes to your clipboard after each pause. Every utterance is also logged to your daily note and saved as a searchable markdown file — so your AI can recall it later.

Three ways to dictate:

```bash
# Terminal
minutes dictate                  # Text → clipboard after each pause
minutes dictate --stdout         # Text → stdout (pipe it anywhere)
minutes dictate --note-only      # Text → daily note only (no clipboard)
```

Or via MCP tools — Claude can start dictation for you:
- `start_dictation` / `stop_dictation` (15 MCP tools total now)

Configurable in the Tauri app under **Settings > Dictation**: whisper model, silence timeout, daily note logging.

### Under the hood

- Streaming audio API + energy-based VAD detect when you start and stop speaking
- Whisper model preloaded for the session — no per-utterance load cost
- Separate `dictation.pid` with recording conflict detection (dictation yields to recording)
- Failed audio preserved to `~/.minutes/dictation-failed/` for recovery
- `[dictation]` config section with 10 fields and sensible defaults

### Native hotkey (coming soon)

CGEventTap module built and ready (`hotkey_macos.rs`) — captures Caps Lock or any key at the hardware level, 0% CPU idle. Disabled in the UI pending code-signed builds for Input Monitoring permission. The infrastructure is there; shipping it once the signing story is sorted.

### Also in this release

- Windows PID lock fix — atomic write through locked file handle (#16)
- macOS build fix — `MACOSX_DEPLOYMENT_TARGET=11.0` for whisper-rs-sys (#14)
- Streaming audio API + VAD in minutes-core (foundation for dictation and future features)
- Calendar timeout fix — no more startup deadlocks (#12, #13)
- MCP dependency fix — `@modelcontextprotocol/ext-apps` now in runtime deps (#18)
- Tauri performance — calendar/readiness checks deferred 2s so UI is interactive instantly
- Claude Code plugin marketplace support — `/plugin install minutes`
- `minutes-sdk` npm package for agent framework integrations

### Install / upgrade

```bash
# CLI (from source)
cargo install --git https://github.com/silverstein/minutes.git minutes-cli

# MCP server (zero-install for Claude Desktop)
npx minutes-mcp@0.6.0

# Tauri app — download from releases
```

### What's next

- Real-time streaming whisper (text appears as you speak)
- Cross-device dictation (iPhone voice memo → clipboard on Mac)
- Native hotkey with signed builds
