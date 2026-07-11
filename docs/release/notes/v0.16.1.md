# Minutes 0.16.1: vocabulary, dictation engine, and macOS reliability

Minutes 0.16.1 ships a batch of dictation improvements, proper-noun vocabulary context for transcription, a rebuilt macOS permission center, main-window chrome integration, and a set of fixes (including a Windows agent-spawn regression, os error 193, reported in #205).

## What changed

### Local vocabulary (`minutes vocabulary`)

Add your own proper nouns (people's names, product names, project code names, uncommon acronyms) and Minutes passes them as a weighted hint to the transcription model before each segment. Names you care about are spelled correctly the first time.

```bash
minutes vocabulary add --person "Dieter Plaetinck" --alias "Dieter"
minutes vocabulary add --term "RxVIP"
minutes vocabulary suggest ~/meetings/2026-05-01-standup.md  # mine a meeting for candidates
minutes vocabulary list
```

The vocabulary store lives at `~/.minutes/vocabulary.toml` and feeds both batch transcription and live transcript sessions.

### Dictation engine upgrades

- **Parakeet final backend.** Dictation can now use parakeet.cpp as its transcription engine. Lower WER than Whisper at equivalent model sizes, Metal-accelerated on Apple Silicon. Opt in via Settings -> Dictation -> Backend.
- **Apple Speech backend gate.** On macOS, on-device Apple Speech Recognition is available as a zero-model-download option for dictation. Opt in via Settings -> Dictation -> Backend.
- **Linux clipboard insertion.** Dictation now works on Linux via `xdotool`/`xclip`, completing the cross-platform story.
- **Local recents memory.** Dictation accumulates a local recents log so frequently dictated phrases surface faster.
- **macOS focus and overlay reliability.** Six targeted fixes to restore the target app's focus after dictation ends, prevent the overlay from re-showing on the next recording, and ensure cleanup is single-use. Fast-path target capture now runs before the native shortcut fires, cutting perceived latency.

### macOS permission center

The desktop app now has a dedicated permission center that shows exactly which macOS entitlements (microphone, screen recording, accessibility/input monitoring) are granted, monitors for live changes, and surfaces the relevant System Settings panel on click. No more guessing why recording is silent.

### macOS main-window chrome

The macOS app integrates proper window chrome: compact header layout, brand mark aligned with sidebar content, window restored on dock-click when previously hidden, clean quit through the Tauri runtime (no more macOS crash reports from `std::process::exit`).

### Windows agent-spawn fix (issue #205)

On Windows, npm installs Claude Code as both `claude` (a Unix shebang script) and `claude.cmd` (the proper CMD wrapper). The Recall agent finder was returning the bare `claude` file first, which `CreateProcessW` can't execute, producing `os error 193: not a valid Win32 application`. The fallback search now tries `.cmd` and `.exe` before the bare name, so Recall reliably finds `claude.cmd`.

### Other fixes and cleanup

- Desktop settings toggles render correctly under all initialization orderings.
- Recall terminal scrollbar and split divider quieted.
- Recall split no longer squeezes control buttons at narrow widths.
- Call detection ignores self-audio from the app.
- Meeting list continues loading after non-fatal setup errors.
- Unnamed in-progress recordings get a name derived from start time.
- Dead `cmd_set_setting` arms for removed hotkey UI panel cleaned up.

## Who should care

- **Everyone on macOS:** permission center, focus reliability, main-window chrome.
- **Windows users:** the os error 193 Recall crash is fixed. The v0.16.1 Windows setup installer is now available.
- **Dictation users:** new backends (Parakeet, Apple Speech), Linux support, faster focus restore.
- **Anyone transcribing proper nouns:** vocabulary context brings first-transcription accuracy to names and terms you care about.
- **Linux users:** dictation clipboard insertion path is now wired.

## CLI / MCP / desktop impact

- **CLI:** `minutes vocabulary` is a new top-level command (list, add, remove, suggest, rebuild). No existing command syntax changed.
- **MCP / agent integrations:** no MCP tool contract changes.
- **Desktop:** Settings -> Dictation exposes backend selection. Permission center is new. Main-window behavior improved on macOS.

## Breaking changes or migration notes

None. The vocabulary store is opt-in; existing configs are untouched. All dictation and transcription changes are additive.

## Known issues

Windows desktop artifacts remain unsigned / advanced-user builds.

MCP auto-install verifies `SHA256SUMS`, but full signature verification is still a follow-up.

Native call capture still depends on local audio routing, permissions, and meeting-app behavior.
