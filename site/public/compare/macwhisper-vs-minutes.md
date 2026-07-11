# Minutes vs MacWhisper

Last reviewed: 2026-07-11

Both transcribe locally on your Mac, and both can run Whisper or Parakeet models. The difference is the shape of the job: MacWhisper is the best drag-and-drop file transcriber on macOS; Minutes is a conversation memory layer your AI agents can query.

## Quick verdict

- Choose **MacWhisper** if your job is transcribing files — interviews, podcasts, videos, YouTube links — with the most polished Mac GUI, subtitle export, and a one-time price (€64 direct; the App Store channel sells subscriptions plus a pricier one-time lifetime unlock).
- Choose **Minutes** if your job is remembering conversations — meetings and memos into a private, diarized, searchable archive for Claude and other agents — open source and free.

## At a glance

- Core job — MacWhisper: file in, transcript out (batch, subtitles, YouTube and media-file URLs); Minutes: capture conversations, diarize, keep a structured archive
- Transcription — both on-device; both support Whisper and Parakeet engines
- Optional cloud AI — MacWhisper: BYO API keys (or fully local via Ollama/LM Studio); Minutes: explicit opt-in only (Claude via MCP, local LLM, or BYO-key cloud — off by default)
- Output — MacWhisper: per-file exports (txt/srt/vtt/md/pdf/docx); Minutes: markdown corpus with YAML frontmatter, action items, decisions
- Speakers — MacWhisper: automatic speaker recognition (Pro); Minutes: diarization + confidence-aware attribution that learns names
- Agent surface — MacWhisper: CLI + workflow automations, no MCP we could find; Minutes: MCP (31 tools), CLI, SDK, Claude Code plugin
- Open source — MacWhisper: no; Minutes: MIT
- Platforms — MacWhisper: macOS (14+ for the App Store build) and iOS; Minutes: macOS menu bar app + CLI (open source)
- Pricing — MacWhisper: free tier, Pro €64 one-time direct; Minutes: free

## Where MacWhisper wins

- Unmatched file-transcription ergonomics: batches, YouTube/media-file URLs, podcast transcription with per-speaker files, filler-word removal, real subtitle workflow with auto-translation
- Honest one-time pricing with lifetime updates; capable free tier (100 languages)
- iOS companion app

## Where Minutes wins

- Builds an archive, not just outputs: every conversation becomes structured markdown, greppable over months
- Agent-native: your assistant searches meetings, tracks commitments, builds person profiles from local files
- Open source (MIT), free — auditable Rust, which matters when "local" is a compliance requirement

## A fair test

Open your transcription tool's output folder. If it's a pile of exports you rarely revisit, MacWhisper is more polished. If you wish that pile were a queryable memory, that wish is the entire reason Minutes exists. Plenty of people should own both — they're neighbors, not rivals.

## Sources

- https://www.macwhisper.com/
- https://apps.apple.com/us/app/whisper-transcription/id1668083311
- https://useminutes.app/for-agents · https://useminutes.app/docs/mcp/tools
- https://github.com/silverstein/minutes
- https://useminutes.app/writing/whisper-cpp-vs-parakeet-cpp
