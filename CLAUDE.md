# CLAUDE.md — Minutes

> Your AI remembers every conversation you've had.

## Project Overview

**Minutes** — open-source, privacy-first conversation memory layer for AI assistants. Captures any audio (meetings, voice memos, brain dumps), transcribes locally with whisper.cpp, diarizes speakers, and outputs searchable markdown with structured action items and decisions. Built with Rust + Tauri v2 + Node.js (MCP).

**Four input modes, one pipeline:**
- **Live recording**: `minutes record` / `minutes stop` — for meetings, calls, conversations
- **Live transcript**: `minutes live` / `minutes stop` — real-time transcription with delta reads for AI coaching mid-meeting
- **Notetaking**: `minutes note "important point"` — timestamped annotations during recording
- **Folder watcher**: `minutes watch` — auto-processes voice memos from iPhone/iCloud

## Quick Start

```bash
cd ~/Sites/minutes
cargo build                          # Build Rust workspace
cargo test -p minutes-core --no-default-features  # Fast tests (no whisper model)
cargo run --bin minutes -- setup --model tiny      # Download whisper model
cargo run --bin minutes -- setup --diarization     # Download speaker diarization models (~34MB)
cargo run --bin minutes -- record    # Start recording
cargo run --bin minutes -- stop      # Stop and process
```

## Full Build (CLI + Tauri App)

```bash
./scripts/build.sh                   # Builds everything and installs CLI
./scripts/build.sh --install         # Same + copies .app to /Applications
./scripts/install-dev-app.sh         # Canonical signed dev app install to ~/Applications/Minutes Dev.app
# Or manually:
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build --release -p minutes-cli           # CLI binary
cargo tauri build --bundles app                # Tauri .app bundle
cp target/release/minutes ~/.local/bin/minutes # Install CLI
open target/release/bundle/macos/Minutes.app   # Launch app
```

**Hard rule for macOS desktop packaging and dogfooding:**

- If the work touches TCC-sensitive features, do **not** keep replacing `/Applications/Minutes.app` with local rebuilds.
- Use `./scripts/install-dev-app.sh` and test `~/Applications/Minutes Dev.app`.
- If a stable local codesigning identity exists, export `MINUTES_DEV_SIGNING_IDENTITY` before running the script.
- On this machine, the preferred identity is:
  - `Developer ID Application: Mathieu Silverstein (63TMLKT8HN)`
- Example:

```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Mathieu Silverstein (63TMLKT8HN)"
./scripts/install-dev-app.sh
```

**IMPORTANT**: After any code change, you must rebuild ALL affected targets:
- CLI changes: `cargo build --release -p minutes-cli && cp target/release/minutes ~/.local/bin/minutes`
- Tauri changes: `cargo tauri build --bundles app` then relaunch the appropriate app bundle
- TCC-sensitive desktop work (hotkeys, Screen Recording, Input Monitoring, Accessibility): `./scripts/install-dev-app.sh`
- MCP server changes: `cd crates/mcp && npm run build` (compiles TS server + builds UI, then restart MCP client sessions)
- MCP App UI only: `cd crates/mcp && npm run build:ui` (rebuild just the dashboard HTML)
- All Rust + app: `./scripts/build.sh` (add `--install` to copy .app to /Applications)
- **Don't forget the MCP server** — it's TypeScript, not Rust. `./scripts/build.sh` does NOT rebuild it. Always run `cd crates/mcp && npm run build` after touching `crates/mcp/src/index.ts` or `crates/mcp/ui/`.

## Desktop Identity Rules

For macOS permission-sensitive development, there are now two distinct desktop app identities:

- Production app:
  - name: `Minutes.app`
  - bundle id: `com.useminutes.desktop`
  - canonical install path: `/Applications/Minutes.app`
- Development app:
  - name: `Minutes Dev.app`
  - bundle id: `com.useminutes.desktop.dev`
  - canonical install path: `~/Applications/Minutes Dev.app`

Use the dev app for any work involving:

- dictation hotkeys / Input Monitoring
- Screen Recording prompts
- AppleScript / Accessibility automation
- any repeated TCC permission prompt investigation

Do not trust results from:

- `./Minutes.app`
- raw `target/debug/minutes-app`
- raw `target/release/minutes-app`
- repo-local bundle outputs launched directly from `target/`

Those identities are not stable enough for TCC debugging.

Native hotkey sanity check:

```bash
./scripts/diagnose-desktop-hotkey.sh "$HOME/Applications/Minutes Dev.app"
```

See [docs/DESKTOP-DEVELOPMENT.md](/Users/silverbook/Sites/minutes/docs/DESKTOP-DEVELOPMENT.md) for the full workflow.

For dictation shortcut work:

- prioritize the `Standard shortcut (recommended)` path first
- treat the raw-key `Caps Lock` / `fn` path as advanced and permission-heavy
- do not call the raw-key path “done” just because the monitor is active; require visible feedback or logged event delivery

### Open-source contributor note

This repo is public, so local scripts must not assume the maintainer's Apple
certificate or local notarization credentials.

- `./scripts/install-dev-app.sh` works without Apple credentials by falling
  back to ad-hoc signing
- for more stable TCC-sensitive testing, contributors can export
  `MINUTES_DEV_SIGNING_IDENTITY` to any consistent local codesigning identity
- release signing / notarization is maintainer-only and should be configured
  explicitly via environment variables, not by hardcoded defaults in scripts

## Pre-Commit Checklist

**Run this mental checklist before every commit from this repo.** Not every item applies to every commit — check which areas your changes touch and verify those.

| Area | When to check | How to verify |
|------|---------------|---------------|
| **Manifest tools sync** | Any new/renamed/removed MCP tool | Compare `manifest.json` tools array against `server.tool()` and `registerAppTool()` calls in `crates/mcp/src/index.ts` |
| **Manifest description** | New user-facing features | Read `long_description` in `manifest.json` — does it mention the new capability? |
| **Manifest version** | Version bumps | `manifest.json` version must match all other version sources |
| **MCP server rebuild** | Any change to `crates/mcp/src/` or `crates/mcp/ui/` | `cd crates/mcp && npm run build` |
| **cargo fmt** | Any Rust change | `cargo fmt --all -- --check` |
| **cargo clippy** | Any Rust change | `cargo clippy --all --no-default-features -- -D warnings` |
| **SDK rebuild** | Any change to `crates/sdk/src/` | `cd crates/sdk && npm run build` |
| **Mutual exclusion** | Any change to recording/dictation/live transcript start paths | Verify all three modes check each other's PID/state: `live_transcript::run` checks recording+dictation PIDs, `cmd_record`/`capture::record_to_wav` checks live PID, `dictation::run` checks live PID, Tauri `cmd_start_*` checks `live_transcript_active`+`recording`+`dictation_active` |
| **Tauri command duplication** | Changes to live transcript start/stop logic | Both `cmd_start_live_transcript` and `handle_live_shortcut_event` must use the shared `try_acquire_live` + `run_live_session` functions. Do NOT duplicate logic. |
| **Desktop app identity** | Any Tauri packaging, dogfooding, Screen Recording, Input Monitoring, Accessibility, call capture, hotkey, or repeated-permission work | Use `./scripts/install-dev-app.sh`, not `rm -rf /Applications/Minutes.app && cp ...`. If a local signing identity exists, export `MINUTES_DEV_SIGNING_IDENTITY` first. Test `~/Applications/Minutes Dev.app`, not `/Applications/Minutes.app`. |
| **README accuracy** | New/removed tools, features, crates, or CLI commands | Tool/resource counts, crate list in Architecture, feature sections, and CLI examples in README.md must reflect the current state. Check: tool count matches `manifest.json`, crate list matches `ls crates/*/`, module count matches `ls crates/core/src/*.rs` |
| **Website accuracy** | New/removed tools, CLI commands, features, or version bumps | Tool count, CLI command count, test count, and feature descriptions in `site/app/page.tsx` must reflect the current state. DMG download link must reference the current version. Redeploy with `cd site && vercel deploy --yes --prod --scope evil-genius-laboratory` |
| **npm dep versions** | Version bumps | `crates/mcp/package.json` `minutes-sdk` dep must reference a version that's actually published on npm. Check with `npm view minutes-sdk versions --json` |
| **Release notes drafted** | Version bumps / releases | Every release is a visibility moment in followers' GitHub feeds. Draft compelling release notes BEFORE creating the release. No empty releases — ever. See Release Checklist step 5. |
| **Release warranted?** | New/removed MCP tools, new CLI commands, user-facing features | Manifest changes (new tools, updated description) don't reach Claude Desktop users until a release is cut and `.mcpb` is uploaded. If the change is user-visible, plan a release. |
| **marketplace.json plugin version bumped** | Any change to `.claude/plugins/minutes/` that should reach users | `.claude-plugin/marketplace.json` → `plugins[0].version` is the **single field Claude Code reads** to decide what "latest" means for plugin-only releases. Bumping `plugin.json` alone is not enough — if `marketplace.json` still advertises the old version, `/plugin update minutes@minutes` reports "already at latest" and users silently miss everything shipped. Keep all three version surfaces in lockstep: `marketplace.json` `plugins[0].version`, `.claude/plugins/minutes/plugin.json`, `.claude/plugins/minutes/.claude-plugin/plugin.json`. |
| **Plugin release upgrade snippet in notes** | Any plugin-only release (push-to-main, no tag) | Plugin releases don't auto-deliver to existing users. Every release-note body must include the two-command refresh sequence: `/plugin marketplace update minutes` then `/plugin update minutes@minutes`, then restart Claude Code. Without this, pre-release adopters stay silently stuck on the version they installed originally — their local marketplace mirror is a git clone that only pulls when explicitly asked. See `notes-release-plugin-v0.8.0.md` for the template. |

## Release Checklist

**When shipping a new version, walk through every item in order.**

### 1. Version bump (all 6 must match)
```bash
# Bump in: Cargo.toml, crates/cli/Cargo.toml, tauri/src-tauri/tauri.conf.json,
#          crates/mcp/package.json, crates/sdk/package.json, manifest.json
# Also bump the version string in crates/mcp/src/index.ts (McpServer({ version }))
# Also bump the minutes-core dep version in crates/cli/Cargo.toml
# Verify:
grep version Cargo.toml tauri/src-tauri/tauri.conf.json crates/mcp/package.json \
  crates/sdk/package.json manifest.json && grep 'version:' crates/mcp/src/index.ts
```

### 2. Manifest sync
- Tools in `manifest.json` match tools registered in `crates/mcp/src/index.ts`
- `long_description` reflects current capabilities
- `keywords` are current

### 3. MCP runtime deps
All `import` statements in `crates/mcp/src/index.ts` must have their packages in `dependencies` (not `devDependencies`) in `package.json`. Smoke-test: `node -e "require('./crates/mcp/dist/index.js')"`

### 4. Build everything
```bash
cd crates/mcp && npm run build       # MCP server + dashboard UI
cargo fmt --all -- --check           # Rust formatting
cargo clippy --all --no-default-features -- -D warnings  # Rust lints
```

**macOS desktop note:**
- For local TCC-sensitive dogfooding before release, rebuild the dev app with:
```bash
export MINUTES_DEV_SIGNING_IDENTITY="Developer ID Application: Mathieu Silverstein (63TMLKT8HN)"
./scripts/install-dev-app.sh --no-open
```
- Do not treat a raw local `/Applications/Minutes.app` copy as the canonical test surface for permission-sensitive features.

### 5. Write release notes
Every release shows up in followers' GitHub feeds — this is free awareness. Write notes BEFORE creating the release. No release should ever ship with an empty body.
- Summarize what shipped and why it matters (not commit messages — outcomes)
- Include install instructions (cargo install, DMG, npx)
- Match the voice of past releases (see v0.8.0, v0.8.1 for examples)
- Save to a temp file: `notes.md`

### 6. Commit, push, create release
```bash
git push origin main                                          # Push commits first
gh release create vX.Y.Z -t "vX.Y.Z: Short Title" -F notes.md --target main  # Creates tag + release with notes, triggers CI
```
**IMPORTANT**: `gh release create` creates the tag on the remote and triggers CI. Do NOT `git tag` locally — that causes a race where CI creates the release before notes exist. The release must exist with notes BEFORE CI workflows run.

### 7. Build and upload .mcpb
```bash
mcpb pack . minutes.mcpb
gh release upload vX.Y.Z minutes.mcpb --clobber
```

### 8. Publish npm packages
```bash
cd crates/sdk && npm publish --access public --registry https://registry.npmjs.org
cd crates/mcp && npm publish --access public --registry https://registry.npmjs.org
```
**IMPORTANT**: `crates/mcp/package.json` must depend on `"minutes-sdk": "^X.Y.Z"` (npm version), NOT `"file:../sdk"` (local path). Check before publishing. If 2FA blocks publish, use a granular access token with "Bypass 2FA" enabled.

### 9. Redeploy landing page
```bash
cd site && npm install && vercel deploy --yes --prod --scope evil-genius-laboratory
```

### 10. Update Homebrew tap formula if CLI changed
The formula lives at `silverstein/homebrew-tap` → `Formula/minutes.rb`. Update the `tag:` to the new version:
```bash
# Fetch current SHA, update via GitHub API
SHA=$(gh api repos/silverstein/homebrew-tap/contents/Formula/minutes.rb --jq '.sha')
# Edit Formula/minutes.rb: change tag: "vX.Y.Z" → new version
# Push via API or clone+commit+push
```
Verify: `brew update && brew info silverstein/tap/minutes` should show the new version.

## GitHub Discussions

Discussions are enabled at `silverstein/minutes` as the community Q&A surface. Issues are for bugs and feature requests; Discussions are for usage questions, setup help, and show-and-tell.

**When to check Discussions:**
- Before closing an issue that's really a question — convert it to a Discussion instead (`gh issue transfer` or manually)
- When a bug report smells like a usage question (wrong device, config confusion, platform quirk) — answer and suggest reposting as a Discussion
- After shipping a release — scan Q&A for questions the release may have answered, and reply with the fix/upgrade path

**When to point users to Discussions:**
- README and error messages that suggest "ask for help" should link to Discussions, not Issues
- Issue templates should nudge Q&A to Discussions

**Quick commands:**
```bash
gh api repos/silverstein/minutes/discussions --jq '.[].title'   # List recent
gh issue list --label question                                    # Find issues that should be discussions
```

## Contributor PRs

**Always merge contributor PRs through GitHub's merge flow.** Use `gh pr merge <number>` or the GitHub merge button. Never cherry-pick/rebase to main manually and then close the PR via API. That makes GitHub display the PR as "Closed" (red) instead of "Merged" (purple), which looks like a rejection to the contributor.

If there are merge conflicts, resolve them on the PR branch and merge through GitHub. If you need to rebase, push to the PR branch first, then merge.

**Never rewrite a contributor's PR without communicating first.** If their approach needs changes, iterate via review comments on their PR. Don't ship a competing implementation silently.

## Project Structure

```
minutes/
├── PLAN.md                    # Master plan (survives compaction — read this first)
├── CLAUDE.md                  # This file
├── BUILD-STATUS.md            # Build progress tracker
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── core/src/              # 33 Rust modules — the engine
│   │   ├── capture.rs         # Audio capture (cpal), device categorization, loopback detection
│   │   ├── resample.rs        # Shared mono-downmix + 16kHz decimation resampler (used by capture + streaming)
│   │   ├── transcribe.rs      # Whisper.cpp transcription (delegates to whisper-guard for anti-hallucination, optional nnnoiseless denoise)
│   │   ├── diarize.rs         # Speaker diarization + attribution types (pyannote-rs, or energy-based from per-source stems)
│   │   ├── summarize.rs       # LLM summarization + speaker mapping (ureq HTTP client)
│   │   ├── voice.rs           # Voice profile storage and matching (voices.db, enrollment, cosine similarity)
│   │   ├── pipeline.rs        # Orchestrates the full flow + structured extraction
│   │   ├── notes.rs           # Timestamped notetaking during/after recordings
│   │   ├── watch.rs           # Folder watcher (settle delay, dedup, lock)
│   │   ├── markdown.rs        # YAML frontmatter + shared parsing utilities
│   │   ├── search.rs          # Walk-dir search + action item queries
│   │   ├── config.rs          # TOML config with compiled defaults
│   │   ├── pid.rs             # PID file lifecycle (flock atomic)
│   │   ├── events.rs          # Append-only JSONL event log for agent reactivity
│   │   ├── device_monitor.rs  # Audio device change detection (CoreAudio listener + auto-reconnect)
│   │   ├── streaming_whisper.rs # Progressive transcription (partial results every 2s)
│   │   ├── streaming.rs       # Streaming audio capture (AudioStream, MultiAudioStream for multi-source)
│   │   ├── logging.rs         # Structured JSON logging
│   │   ├── error.rs           # Per-module error types (thiserror)
│   │   ├── calendar.rs        # Calendar integration (upcoming meetings)
│   │   ├── daily_notes.rs     # Daily note append for dictation/memos
│   │   ├── dictation.rs       # Dictation mode (speak → clipboard + daily note)
│   │   ├── live_transcript.rs # Live transcript mode (real-time JSONL + WAV, delta reads, AI coaching)
│   │   ├── health.rs          # System health checks (model, mic, disk, watcher)
│   │   ├── hotkey_macos.rs    # macOS global hotkey registration
│   │   ├── screen.rs          # Screen context capture (screenshots)
│   │   ├── vad.rs             # Voice activity detection
│   │   ├── vault.rs           # Obsidian/Logseq vault sync
│   │   ├── knowledge.rs       # Knowledge base adapters (wiki/PARA/Obsidian) + fact writing
│   │   └── knowledge_extract.rs # Structured fact extraction from meeting frontmatter
│   ├── whisper-guard/          # Standalone anti-hallucination toolkit (segment dedup, silence strip, whisper params)
│   ├── cli/                   # CLI binary — 33 commands
│   ├── reader/                # Lightweight read-only meeting parser (no audio deps)
│   ├── assets/                # Bundled assets (demo.wav)
│   └── mcp/                   # MCP server — 26 tools + 7 resources + MCP App dashboard
│       └── ui/                # Interactive dashboard (vanilla TS, builds to single-file HTML)
├── site/                      # Landing page (Next.js + Remotion demo player)
├── tauri/                     # Tauri v2 menu bar app + singleton AI Assistant
├── .claude/plugins/minutes/   # Claude Code plugin — 18 skills + 1 agent + 2 hooks
└── tests/integration/         # Integration tests (including real whisper tests)
```

## Development Commands

```bash
# Build (macOS 26 needs C++ include path for whisper.cpp)
export CXXFLAGS="-I$(xcrun --show-sdk-path)/usr/include/c++/v1"
cargo build

# Test
cargo test -p minutes-core --no-default-features   # Fast (no whisper model)
cargo test -p minutes-core                          # Full (needs tiny model)

# Lint
cargo clippy --all --no-default-features -- -D warnings
cargo fmt --all -- --check

# MCP server (TS server + interactive dashboard UI)
cd crates/mcp && npm install && npm run build       # tsc + vite single-file build
npx vitest run                                      # 30 reader.ts unit tests
node test/mcp_tools_test.mjs                        # 8 MCP integration tests
```

## Key Architecture Decisions

- **Rust** for the engine — single 6.7MB binary, cross-platform, fast
- **whisper-rs** (whisper.cpp) for transcription — local, Apple Silicon optimized, params match whisper-cli defaults (best_of=5, entropy/logprob thresholds)
- **ffmpeg preferred for audio decoding** — shells out to ffmpeg for m4a/mp3/ogg when available (identical to whisper-cli's pipeline). Falls back to symphonia (pure Rust) when ffmpeg isn't installed. This matters for non-English audio — symphonia's AAC decoder produces subtly different samples that trigger whisper hallucination loops (issue #21).
- **Silero VAD** (via whisper-rs) — ML-based voice activity detection integrated directly into whisper's transcription params. Prevents hallucination loops by skipping silence segments. Auto-downloaded during `minutes setup`.
- **whisper-guard** crate — standalone anti-hallucination toolkit extracted from minutes-core. 6-layer defense: Silero VAD gating, no_speech probability filtering (>80% = skip), consecutive segment dedup (3+ similar collapsed), interleaved A/B/A/B pattern detection, foreign-script hallucination detection, language-agnostic noise marker collapse (`[Śmiech]`, `[music]`, `[risas]`, etc.), trailing noise trimming. Publishable to crates.io independently.
- **nnnoiseless** (optional) — pure Rust RNNoise port for noise reduction. Behind `denoise` feature flag, controlled by `config.transcription.noise_reduction`. Processes at 48kHz with first-frame priming. Batch path only (not streaming).
- **pyannote-rs** for speaker diarization — native Rust, ONNX models (~34MB), no Python. Works in CLI, Tauri desktop app, and via MCP. Behind the `diarize` Cargo feature flag.
- **Speaker attribution** — confidence-aware system mapping SPEAKER_X labels to real names. Four levels: L0 (deterministic 1-on-1 via calendar+identity), L1 (LLM suggestions capped at Medium confidence), L2 (voice enrollment in `voices.db`), L3 (confirmed-only learning). Wrong names are worse than anonymous — only High-confidence attributions rewrite transcript labels. `speaker_map` in YAML frontmatter is the canonical attribution data. Voice profiles stored in `~/.minutes/voices.db` (separate from `graph.db` which wipes on rebuild).
- **symphonia** for audio format conversion — m4a/mp3/ogg → WAV, pure Rust (fallback when ffmpeg unavailable)
- **Windowed-sinc resampler** (32-tap Hann) — alias-free 44100→16000 downsampling for WAV inputs
- **ureq** for HTTP — pure Rust, no secrets in process args (replaced curl)
- **fs2 flock** for PID files — atomic check-and-write, prevents TOCTOU races
- **Tauri v2** for desktop app — shares `minutes-core` with CLI, ~10MB
- **Markdown + YAML frontmatter** for storage — universal, works with everything
- **Structured extraction** — action items + decisions in frontmatter as queryable YAML
- **No API keys needed** — Claude summarizes conversationally via MCP tools
- **Live transcript** — per-utterance whisper → JSONL append with PidGuard flock for session exclusivity. Delta reads via line cursor or wall-clock duration. Optional WAV preservation for post-meeting reprocessing. Agent-agnostic: JSONL readable by any agent, MCP tools for Claude, CLAUDE.md context injection for Codex/Gemini.

## Key Patterns

- All audio processing is local (whisper.cpp + pyannote-rs + Silero VAD). ffmpeg recommended but optional.
- Claude summarizes via MCP when the user asks (no API key needed)
- Optional automated summarization via Ollama (local), Mistral, or cloud LLMs
- Config at `~/.config/minutes/config.toml` (optional, compiled defaults work)
- Tauri assistant uses a singleton workspace at `~/.minutes/assistant/`
- `CLAUDE.md` holds general assistant instructions; `CURRENT_MEETING.md` is the active meeting focus for "Discuss with AI"
- Meetings: `~/meetings/` | Voice memos: `~/meetings/memos/`
- `0600` permissions on all output (sensitive content)
- PID file + flock for recording state (`~/.minutes/recording.pid`)
- Watcher: settle delay, move to `processed/`/`failed/`, lock file
- JSON structured logging: `~/.minutes/logs/minutes.log`
- 100% doc comment coverage on all pub functions

## Test Coverage

~290 tests total:
- 49 whisper-guard unit tests (resample, normalize, strip_silence, dedup_segments, dedup_interleaved, collapse_noise_markers, strip_foreign_script, trim_trailing_noise, clean_transcript + 1 doctest)
- 130 core unit tests (all modules including screen, calendar, config, watch, streaming whisper, vault, dictation, live_transcript, health, vad, hotkey, device_monitor, diarize)
- 10 integration tests (pipeline, permissions, collisions, search filters)
- 33 Tauri unit tests (commands, call detection, call capture)
- 11 CLI tests
- 6 reader crate tests (search, parse)
- 30 reader.ts unit tests (vitest — frontmatter parsing, listing, search, actions, profiles; reader lives in crates/sdk/src/reader.ts)
- 8 MCP integration tests (CLI JSON output, TypeScript compilation)
- 1 hook unit test (post-record hook)

## Claude Ecosystem Integration

- **MCP Server**: 26 tools + 7 resources for Claude Desktop / Cowork / Dispatch (`npx minutes-mcp` for zero-install)
- **Claude Code Plugin**: 18 skills (7 capture + 1 search + 4 lifecycle + 2 coaching + 3 knowledge + 1 intelligence) + meeting-analyst agent + SessionStart + PostToolUse hooks
- **Interactive meeting lifecycle**: `/minutes-brief` → `/minutes-prep` → record → `/minutes-tag` → `/minutes-debrief` → `/minutes-mirror` → `/minutes-weekly` with skill chaining via `.brief.md` / `.prep.md` files; `/minutes-graph` for cross-meeting entity queries
- **Conversational summarization**: Claude reads transcripts via MCP, no API key needed
- **Auto-tagging + alerts**: PostToolUse hook tags meetings with git repo, checks for decision conflicts, surfaces overdue action items
- **Proactive reminders**: SessionStart hook checks calendar for upcoming meetings and recommends `/minutes-brief` (fast) or `/minutes-prep` (goal-setting) based on time-to-meeting
- **Desktop assistant**: Tauri AI Assistant is a singleton session that can switch focus into a selected meeting without spawning parallel assistant workspaces
- **Live coaching**: Tauri Live Mode toggle starts real-time transcription; the assistant workspace `CLAUDE.md` auto-updates so the connected Recall session, Claude Desktop/Code, or any other agent can read the live JSONL file and coach mid-meeting. There is no dedicated transcript/coaching panel in Tauri v1; the coaching happens through the assistant chat surface.
