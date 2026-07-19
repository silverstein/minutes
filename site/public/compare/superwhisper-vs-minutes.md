# Minutes vs superwhisper

Last reviewed: 2026-07-11

superwhisper and Minutes agree on the thing this category usually gets wrong: your voice should be transcribed on your device, not in someone's cloud. The difference is the job. superwhisper is a polished dictation tool — speak, and clean text lands in whatever app you're typing in. Minutes treats dictation as one input to a bigger system: an open-source conversation memory that records meetings, diarizes speakers, and writes markdown files your AI agents can query.

## Quick verdict

- Choose **superwhisper** if you want the most refined dedicated dictation experience — custom per-app modes, 100+ languages, iOS and Windows support — and you're happy paying a subscription for a closed-source tool.
- Choose **Minutes** if dictation is one mode of a bigger need — recording meetings, keeping voice memos, and building a private, searchable memory of your conversations that Claude and other agents can use — and you want it open source and free.

## At a glance

- Core job — superwhisper: speak, get clean formatted text where you're typing; Minutes: capture conversations, transcribe and diarize them, keep a searchable markdown record
- Where transcription runs — superwhisper: on-device by default, optional cloud models (recommended on Intel Macs); Minutes: on-device always (sealed local whisper.cpp), no cloud path
- AI formatting — superwhisper: predefined and custom modes using local or cloud models; Minutes: optional and explicit (Claude via MCP or a local LLM you configure)
- Durable output — superwhisper: text inserted into the app you're using; Minutes: markdown files with YAML frontmatter, action items, and decisions
- Meetings and speakers — superwhisper: meeting recording and file transcription; Minutes: diarized speakers, confidence-aware attribution, action items, meeting lifecycle
- Agent/MCP surface — superwhisper: none we could find; Minutes: MCP server (31 tools), CLI, SDK, Claude Code plugin
- Open source — superwhisper: no; Minutes: yes, MIT
- Platforms — superwhisper: macOS, Windows, iOS; Minutes: macOS menu bar app + CLI (open source)
- Pricing — superwhisper: free tier, Pro subscription, lifetime and enterprise options; Minutes: open source and free

## Where superwhisper wins

- More polished dictation: per-app custom modes (email vs Slack vs prose), 100+ languages
- Wider platform reach today: macOS, Windows, iOS
- Simpler purchase if you never record meetings and never want a transcript archive

## Where Minutes wins

- A memory layer, not just an input method: meetings and memos become diarized, searchable markdown with action items — a record you own
- Open source (MIT) and free: read the capture, transcription, and storage code instead of trusting a privacy page
- Agent-native: Claude, Codex, and any MCP client query your conversation history through 31 MCP tools, a CLI, an SDK, and a Claude Code plugin

## A useful test

A month from now, will you want to ask an assistant "what did I say about this?" If no, you want a dictation tool. If yes, you want a memory layer — dictation included.

## When Minutes is not the right fit

- When you want best-in-class dictation UX on iOS or Windows today, or per-app formatting modes are your daily feature
- When markdown files, a CLI, and agent workflows are complexity you don't want — a single-purpose dictation app is legitimately simpler

## Sources

- https://superwhisper.com
- https://useminutes.app/for-agents
- https://useminutes.app/docs/mcp/tools
- https://github.com/silverstein/minutes
