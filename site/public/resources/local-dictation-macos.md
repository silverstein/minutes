# Local dictation on macOS: the complete guide

Last reviewed: 2026-07-11

Dictation is the most personal audio there is, and it's where cloud processing is least necessary: modern Macs transcribe speech locally faster than you can talk.

## Quick answer

- Occasional sentence → **built-in macOS dictation** (already installed; Apple processes many languages on-device on Apple Silicon, with auto-punctuation)
- Heavy daily dictation with per-app formatting → **superwhisper** (local models by default, closed source, subscription)
- File transcription first, dictation included → **MacWhisper** (one-time Pro purchase; dictation is in the direct-download version only, not the App Store build)
- Dictation as part of a conversation-memory system → **Minutes** (open source, free; text typed at your cursor — or clipboard via CLI — plus a timestamped daily note, alongside meetings/memos/live transcription, all agent-searchable)

Careful if "local" is your requirement: several popular dictation apps (Wispr Flow best known) process speech in the cloud — a different privacy contract.

## Setting up Minutes dictation

Install, run `minutes setup --model tiny` once, bind the hotkey in the menu bar app. Speak: text is inserted at your cursor (CLI mode uses the clipboard), and a timestamped copy is appended to your daily note in ~/meetings. Text you dictate into other apps vanishes into those apps; text that also lands in your own files compounds — your agents can answer "what was that idea I had last Tuesday?"

## How to choose

Volume decides it. A few sentences a week: built-in is enough. Hours daily where per-app formatting saves time: superwhisper earns its subscription. Mostly files, occasional dictation: MacWhisper. Dictation as one piece of capturing meetings/memos/ideas into a private searchable archive: Minutes, free and open source.

## Sources

- https://support.apple.com/guide/mac-help/use-dictation-mh40584/mac
- https://superwhisper.com
- https://goodsnooze.gumroad.com/l/macwhisper
- https://github.com/silverstein/minutes
- https://useminutes.app/compare/superwhisper-vs-minutes
