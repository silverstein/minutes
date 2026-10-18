# Minutes vs MacWhisper

Last reviewed: 2026-07-18

Both transcribe locally on your Mac. MacWhisper can offer Whisper and Parakeet there; Minutes offers local engines according to platform, build, and model availability. The difference is the shape of the job: MacWhisper is the best drag-and-drop file transcriber on macOS; Minutes is a conversation memory layer your AI agents can query.

## Quick verdict

- Choose **MacWhisper** if your job is transcribing files — interviews, podcasts, videos, YouTube links — with the most polished Mac GUI, subtitle export, and a one-time price (see the official direct and App Store pricing).
- Choose **Minutes** if your job is remembering conversations — meetings and memos into a private, diarized, searchable archive for Claude and other agents — open source and free.

## At a glance

- Core job — MacWhisper: file in, transcript out (batch, subtitles, YouTube and media-file URLs); Minutes: capture conversations, diarize, keep a structured archive
- Transcription — both on-device; MacWhisper supports Whisper and Parakeet, while Minutes uses local engines according to platform, build, and model availability
- Optional cloud AI — MacWhisper: BYO API keys (or fully local via Ollama/LM Studio); Minutes: explicit opt-in only (Claude via MCP, local LLM, or BYO-key cloud — off by default)
- Output — MacWhisper: per-file exports (txt/srt/vtt/md/pdf/docx); Minutes: markdown corpus with YAML frontmatter, action items, decisions
- Speakers — MacWhisper: automatic speaker recognition (Pro); Minutes: diarization + confidence-aware attribution that learns names
- Agent surface — MacWhisper: CLI + workflow automations, no MCP we could find; Minutes: MCP (34 tools), CLI, SDK, Claude Code plugin
- Open source — MacWhisper: no; Minutes: MIT
- Platforms — MacWhisper: macOS (14+ for the App Store build) and iOS; Minutes: macOS and Windows desktop apps; Linux CLI
- Pricing — MacWhisper: free tier, paid editions; check current official pricing; Minutes: free

## Where MacWhisper wins

- Unmatched file-transcription ergonomics: batches, YouTube/media-file URLs, podcast transcription with per-speaker files, filler-word removal, real subtitle workflow with auto-translation
- Straightforward one-time pricing with lifetime updates; capable free tier (100 languages)
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

## Release spot check

Release spot check, October 7, 2026: reviewed the linked official product pages for current workflow, provider, platform, and pricing claims. This is not a new hands-on benchmark. Minutes captures, transcribes, and stores conversation records locally. Engine availability depends on your platform, build, and installed models. If you choose a cloud summarizer or connect a cloud assistant, authorized meeting context can reach that provider. File sync and backups you configure are separate data boundaries.
