# Minutes v0.11.0: Command palette, faster whisper, better call capture

## The keyboard-first launcher ships. Whisper uses your GPU now (it wasn't before). Two contributors fixed bugs you'll actually notice.

v0.11.0 ships the command palette as the headline feature, plus a round of fixes that change what Minutes feels like in daily use. The capture cues sound like real objects instead of compiler beeps. Native Meet calls stop labeling you as SPEAKER_00. The screen share privacy toggle, which had been quietly broken since it shipped, actually does the thing now.

## The command palette

Press `⌘⇧K` from anywhere on macOS. The palette opens centered on your active monitor. Type `rec` and hit Enter to start recording. Type `note`, hit Enter, type your annotation, hit Enter again. It drops a timestamped note into the active session. Type `search pricing` and you're searching transcripts.

Twenty commands ship in this release: recording start/stop/note, dictation start/stop, live transcript start/stop/read, search transcripts, research topic, find open action items, find recent decisions, open latest meeting, open latest meeting from today, show upcoming meetings, open meetings/memos/assistant folders, copy meeting markdown, and rename current meeting.

The palette knows what state you're in. Stop-recording only appears while you're recording. Rename-current-meeting only shows up when the assistant has a meeting selected. The list re-fetches automatically when state changes elsewhere. Start a recording from the tray menu while the palette is open and you'll see it react in under a frame.

The five most recently executed commands float to the top when the query is empty, with their original payload intact. Search "pricing" once, and tomorrow you can re-run that exact search by pressing `⌘⇧K`, Enter. The recents store remembers the query, not just the command name.

`Rename current meeting` is fail-closed. It parses your meeting frontmatter as YAML, refuses to touch any file whose `title` field isn't a plain-string scalar (no folded blocks, no anchors, no aliases), edits exactly the title line, validates by re-parsing, rolls back if the post-write parse fails, then renames the file on disk to match the new slug. User-added body sections are preserved exactly. Original file modes are preserved, so an Obsidian-synced `0644` file stays `0644`. If you've ever been bitten by a rename tool that quietly mangled your YAML, this is the opposite of that.

The shortcut defaults on for both fresh installs and upgrades. The first time v0.11.0 launches and registers `⌘⇧K`, you get a one-time macOS notification telling you the binding is live and pointing you at Settings to disable it if it conflicts with your IDE. `⌘⇧K` overlaps with Finder's "Connect to Server" and Firefox's Web Console, so the Settings overlay also lets you pick `⌘⇧O` or `⌘⇧U` instead. None of those collide with the other Minutes shortcuts.

## Whisper actually uses your GPU now

If you built Minutes with `--features metal` (or CoreML, or CUDA) and your log printed `whisper_backend_init_gpu: no GPU found`, that was real. Every one of the five `WhisperContext` creation sites in the workspace was using `WhisperContextParameters::default()`, which leaves GPU enablement up to whisper.cpp's C defaults. Those defaults didn't know your binary was compiled with Metal support.

v0.11.0 routes all five sites through a single `whisper_context_params()` helper that sets `use_gpu = true` when a GPU backend feature is compiled in. On Apple Silicon with the default Metal build, transcription is now meaningfully faster on the same binary. The feature was shipping in your build flags and not reaching the runtime. PR #93, contributed by @gregoire22enpc.

## Native Meet calls: your name, not SPEAKER_00

Five fixes to native Meet call capture and attribution, driven by dogfood on real calls:

- **Local speaker mapped from native stems.** When the diarizer correctly found two speakers but couldn't tell which one was you, the attribution layer now uses the per-source stem identity to deterministically map the local speaker. Your name gets labeled as your name instead of SPEAKER_00.
- **Short second-source labels preserved.** Brief interjections from the second audio source were being stripped as noise. On fast back-and-forth calls, whole sides of conversation were vanishing. They stay now.
- **Non-speech events stay anonymous.** `[laughter]`, `[cough]`, `[music]` were sometimes being wrapped in a speaker label as if someone had "said" them. They pass through as anonymous events, which is how downstream readers already expected them.
- **Speaker fallback hardened, config text preserved.** When L1 attribution suggestions couldn't be confirmed, the fallback path was occasionally dropping the original config hints. Both survive the fallback now.
- **Stop desktop-owned captures via sentinel.** Recordings started by the native desktop helper could get orphaned if the stop path didn't reach the helper process. A sentinel file now signals stop across the process boundary.

## The screen share privacy toggle was broken

The "Hide windows from screen sharing" toggle in Settings was a button that flipped its own label and did nothing else. The value was never persisted to config, never handed to the backend, and never applied to any Tauri window or the tray. Turning it on and restarting the app silently reset it. Turning it on mid-session didn't actually hide anything.

PR #97 from @cathrynlavery is a three-layer fix: the setting now persists through `config.rs`, the toggle invokes the backend on change, and the persisted value is applied to every Tauri window and the tray on launch. Visible call sessions also route through the call intent path so the privacy choice is honored end-to-end. This is the kind of bug only a new set of eyes catches. Thanks Cathryn.

## Desktop app: design pass

The Tauri menu bar app got a 17-phase design overhaul. Webview pinned to dark, vibrancy material switched from the adaptive Sidebar to HudWindow so white text stays readable over bright apps behind you. Body composites a `rgba(22,22,24,0.78)` overlay on top of the vibrancy material. Footer row laid out with flex so Recall stops getting pushed off the right edge. Accent recalibrated from `#4da8d9` (default-Bootstrap blue) to `#6391b3` (a desaturated slate that fits the editorial-amber and serif-display identity). Real SVG icons replacing every Unicode glyph in the chrome (the search icon was, improbably, U+2315 ⌕ Telephone Recorder; it is now an actual magnifying glass). Tabular numerics on meeting times. Vision OS easing `cubic-bezier(0.32, 0.72, 0, 1)` on every button, focus ring, and pane transition. Brand-mark breathing animation that switches to a warmer red pulse when recording. Shimmering skeleton loading state replacing the old `⏳ Loading...`. Detail card upgraded to layered material with `backdrop-filter: blur(24px)`. Staggered enter/exit transitions on the recording → processing → saved banner sequence using `@starting-style` and `transition-behavior: allow-discrete`.

The polish pass also caught a real bug: the Open button on the inline processing card was being shown the moment `job.outputPath` existed, but the file was still being written. Hovering caused it to flicker (the renderer rebuilds the card on every status poll), and clicking did nothing. It's now gated on `job.state === 'transcript-only' || 'complete'`.

## New capture cues

The start, stop, complete, and error cues are no longer additive sine. The old ones sounded like a 1985 C compiler beep, with stop and complete confusingly close (both ending on F5) and a dissonant minor-2nd cluster on error. The new cues come from the ElevenLabs Sound Effect V2 API, post-processed through ffmpeg loudnorm and fades:

- **start**: soft mallet on a crystal singing bowl
- **stop**: felt mallet on hollow wood
- **complete**: leather book closing
- **error**: low muted wooden knock

`scripts/generate_cues_elevenlabs.mjs` and `scripts/install_cues.mjs` ship in the repo if you want to regenerate them with your own taste.

## Tests

- 25 palette registry and recents tests in `minutes-core`
- 16 `markdown::rename_meeting` tests covering folded scalars, anchors, aliases, literal blocks, CRLF endings, slug collisions, special-character escapes, post-write rollback, and Unix file-mode preservation
- 23 Tauri palette tests including 11 `ActionResponse` envelope tests, 3 shortcut validation tests, and the cross-collision smoke test
- 18 config tests including the upgrade migration

All green. `cargo fmt --all -- --check` clean. `cargo clippy --all --no-default-features -- -D warnings` clean.

## Install

```bash
# macOS desktop app
brew install --cask silverstein/tap/minutes

# CLI only
brew install silverstein/tap/minutes    # or: cargo install minutes-cli

# MCP server (zero-install, works with Claude Desktop, Cursor, Codex, Gemini CLI)
npx minutes-mcp@0.11.0
```

For Claude Desktop, grab the `.mcpb` from this release's assets and install via the extensions drawer.

## Who should definitely upgrade

- **Anyone running Minutes with a Metal, CoreML, or CUDA build of whisper.** You were running on CPU. You don't have to anymore.
- **Anyone whose Meet calls have been showing up labeled SPEAKER_00.** Your name shows up now.
- **Anyone who turned on "Hide windows from screen sharing" and assumed it worked.** It didn't. It does now.
- **Anyone who lives in keyboard launchers** (Raycast, Spotlight, Alfred, the JetBrains "Find Action" muscle memory). The palette is for you.

## Contributors

- **@gregoire22enpc** (#93): found the whisper GPU bug. Every `WhisperContext` creation site in the workspace was leaving GPU enablement to whisper.cpp's C defaults regardless of what Cargo features were compiled in. Caught it on the first read.
- **@cathrynlavery** (#97): fixed the screen share privacy toggle. The setting existed in the UI but wasn't persisted, wasn't wired to the backend, and wasn't applied to any of the Tauri windows or the tray. The end-to-end wiring in this release is hers.

Full changelog: https://github.com/silverstein/minutes/compare/v0.10.4...v0.11.0
