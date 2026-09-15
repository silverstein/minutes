# minutes

[![GitHub stars](https://img.shields.io/github/stars/silverstein/minutes?style=social)](https://github.com/silverstein/minutes)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![crates.io](https://img.shields.io/crates/v/minutes-cli.svg)](https://crates.io/crates/minutes-cli)
[![npm](https://img.shields.io/npm/v/minutes-mcp.svg)](https://www.npmjs.com/package/minutes-mcp)

[useminutes.app](https://useminutes.app)

Minutes is a free, open-source conversation memory app for the AI you already use. Record and transcribe meetings, calls, voice memos, and dictation on your device, then let Claude Code, Codex, Cursor, or another MCP client search the history you choose to share. Your records are Markdown files you own in `~/meetings/`.

Capture, transcription, and storage run locally. If you choose a cloud AI assistant or summarizer, the meeting context you authorize is sent to that provider.

**Your conversations. Your memory. Ready for the AI you use.**

<p align="center">
  <img src="docs/assets/demo.gif" alt="minutes demo; record, dictate, phone sync, AI recall" width="750">
</p>

## Install

Choose the desktop app or one standalone CLI installation. The desktop app
includes a CLI: use **About Minutes → Set up CLI** to add it to your PATH.

```bash
brew install --cask silverstein/tap/minutes  # Desktop app, with bundled CLI
brew install silverstein/tap/minutes         # Standalone CLI alternative
cargo install minutes-cli                   # Standalone CLI via Cargo
```

For an agent connection, add the MCP server separately with
`claude mcp add minutes -- npx -y minutes-mcp` (or run `npx minutes-mcp`
from another MCP client).

If Homebrew reports an untrusted tap, run `brew trust silverstein/tap` once.

## Ask your agent

Add Minutes to your agent, seed five sample meetings (no mic needed), then ask about them:

```bash
claude mcp add minutes -- npx -y minutes-mcp
minutes demo --full
```

> What did we decide about monthly billing, and did it stick?

The answer spans two meetings a month apart: launched on Feb 28, reversed on Mar 25.

A direct MCP call uses the same local meeting files:

```text
search_meetings({"query":"monthly billing decision"})
```

Record a real one with `minutes record`, then `minutes stop` to transcribe. `minutes demo --clean` removes the samples.
Setup: [Codex, Gemini CLI, and Claude Desktop](docs/integration/clients.md#any-mcp-client-claude-code-codex-opencode-gemini-cli-claude-desktop-or-your-own-agent),
[Cursor](docs/integration/cursor-agent.md),
[OpenCode](docs/integration/clients.md#opencode-cli),
[Pi](docs/integration/pi-agent.md),
[Mistral Vibe](docs/integration/clients.md#mistral-vibe), and
[Cowork or Dispatch](docs/integration/clients.md#cowork--dispatch).

### Works with

Claude Code · Codex · Cursor · OpenCode · Pi · Gemini CLI · Claude Desktop ·
Mistral Vibe · Cowork · Dispatch · Obsidian · Logseq · any MCP client

## How it compares

| Workflow | Granola | Otter.ai | Anarlog (formerly Hyprnote) | Minutes |
|---|---|---|---|---|
| Product | Hosted meeting notepad | Hosted meeting assistant | Local meeting notepad | Local conversation memory |
| Agent access | Hosted MCP | Hosted integrations | CLI + MCP | Local files, MCP, CLI, SDK |
| Primary storage | Hosted workspace | Hosted workspace | Local SQLite + files | Local Markdown + YAML |
| Local AI | Cloud transcription | Cloud transcription | With local providers | Local transcription; AI provider of your choice |

Reviewed September 5, 2026 against [Granola MCP](https://docs.granola.ai/help-center/sharing/integrations/mcp), [Otter privacy and security](https://otter.ai/privacy-security), and the [Anarlog repository](https://github.com/fastrepl/anarlog). Anarlog's community app is MIT; its enterprise components use a commercial license. See [the detailed comparison](https://useminutes.app/compare/hyprnote-vs-minutes) and [open-source alternatives](https://useminutes.app/resources/open-source-alternatives-to-granola-ai) for fit and limitations.

## Who it's for

Agent-first users treat Minutes as the capture layer of a local second brain.
They search meetings through Claude Code, Cursor, Codex, OpenCode, MCP, or the
CLI, then fold useful context into a vault or wiki they own.

Desktop notetaker users record and read results in the app. They value reliable
setup, clear processing and recovery, Recall, documents, Coach, and summary
quality. See the full [persona notes](docs/personas.md).

## Surfaces

| Surface | What it provides | Docs |
|---|---|---|
| Desktop app | Menu bar capture, Recall, documents, and Coach. | [Install](docs/install.md#desktop-app) |
| CLI (58 commands) | Local recording, processing, search, import, and automation. | [Commands](docs/features.md) |
| MCP server (34 tools) | Local meeting tools and resources for any MCP client. | [MCP reference](docs/integration/agent-integrations.md) |
| Claude Code plugin (23 skills) | Prep, capture, live help, debrief, and memory workflows. | [Client setup](docs/integration/clients.md#claude-code-plugin) |
| SDK | TypeScript access to meeting files without MCP. | [Agent architecture](docs/architecture/README.md#building-your-own-agent-on-minutes) |

## Output format

Meetings are plain markdown with structured YAML frontmatter:

```yaml
---
title: Q2 Pricing Discussion with Alex
type: meeting
date: 2026-03-17T14:00:00
duration: 42m
context: Discuss Q2 pricing
action_items:
  - assignee: mat
    task: Send pricing doc
    due: Friday
    status: open
decisions:
  - text: Test monthly billing with 10 advisors
---
```

See the [frontmatter schema](docs/architecture/frontmatter-schema.md). Files work
with [Obsidian](https://obsidian.md), grep, and any markdown tool.

## Privacy & consent

- Transcription and speaker processing run on-device. Audio stays on your machine.
- Sensitive meetings save typed markers without audio and default to restricted.
- Consent reminders, acknowledgement, and provenance help you disclose recording.
- Text leaves the machine only when you send authorized context to a cloud agent or summarizer.

See the [security documentation](docs/security/) and
[consent enforcement design](docs/architecture/consent-enforcement.md).

## Docs

- [Features and commands](docs/features.md)
- [Install, setup, updating, and troubleshooting](docs/install.md)
- [Agent and MCP client integrations](docs/integration/clients.md)
- [Phone voice memo pipeline](docs/phone-voice-memo-pipeline.md)
- [Summarization and automation](docs/architecture/summarization.md)
- [Configuration](docs/configuration.md)
- [Architecture and agent development](docs/architecture/README.md)
- [Switching from Granola and importing archives](docs/switching-from-granola.md)
- [Frontmatter schema](docs/architecture/frontmatter-schema.md)
- [Security](docs/security/)
- [Personas](docs/personas.md)
- [MCP tools](https://useminutes.app/docs/mcp/tools)
- [Error reference](https://useminutes.app/docs/errors)
- [Agent index](https://useminutes.app/llms.txt)
- [Full agent index](https://useminutes.app/llms-full.txt)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Minutes is MIT licensed and staying that way: no relicensing, no paid tier for anything in this repo.

MIT. Built by [Mat Silverstein](https://github.com/silverstein), founder of
[X1 Wealth](https://x1wealth.com).

## Star History

[![Star History Chart](https://star-history.dera.page/svg?repos=silverstein/minutes&type=Date)](https://star-history.dera.page/#silverstein/minutes&Date)


## 🌐 Web Resources & Aesthetic Symbols Index
- [SYM 1F48C](https://matrix-glitch-text-37.pages.dev/symbol/sym-1f48c/)
- [SYM 1FA75](https://pastel-moe-emoticons-80.pages.dev/symbol/sym-1fa75/)
- [FLUTTERING BUTTERFLY](https://sleek-line-symbols-51.pages.dev/symbol/fluttering-butterfly/)
- [SYM 1D477](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1d477/)
- [SYM 1D485](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1d485/)
- [SYM 1F47B](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1f47b/)
- [SYM 1D499](https://theeduplaycampen.pages.dev/symbol/sym-1d499/)
- [SYM 26AF](https://cyberpunk-clan-tags-43.pages.dev/symbol/sym-26af/)
- [SYM 1D44E](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d44e/)
- [SYM 26A2](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26a2/)
- [SYM 1D499](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d499/)
- [SYM 1D454](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d454/)
- [BORDERS DIVIDERS](https://theeduplaycampen.pages.dev/vi/borders-dividers/)
- [SYM 1D43F](https://theeduplaycampen.pages.dev/symbol/sym-1d43f/)
- [SYM 268C](https://theeduplaycampen.pages.dev/symbol/sym-268c/)
- [SYM 26BE](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26be/)
- [SYM 1D466](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d466/)
- [FLORAL HEART VINE](https://theeduplaycampen.pages.dev/symbol/floral-heart-vine/)
- [LATIN CROSS HEAVY](https://coquette-aesthetic-symbols-86.pages.dev/symbol/latin-cross-heavy/)
- [SYM 2656](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-2656/)
- [SYM 1D46D](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d46d/)
- [SYM 263F](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-263f/)
- [PISCES ZODIAC FISHES](https://coquette-aesthetic-symbols-86.pages.dev/symbol/pisces-zodiac-fishes/)
- [DAGGER CROSS SYMBOL](https://coquette-aesthetic-symbols-86.pages.dev/symbol/dagger-cross-symbol/)
- [SYM 1D45D](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d45d/)
- [SYM 1D4A1](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1d4a1/)
- [SYM 1D47B](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d47b/)
- [SYM 265D](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-265d/)
- [SYM 1D479](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d479/)
- [TELUGU RIBBON BOWLET](https://theeduplaycampen.pages.dev/symbol/telugu-ribbon-bowlet/)
- [SYM 1D495](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1d495/)
- [SYM 1F643](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1f643/)
- [HEAVY STAR](https://angelic-bow-symbols-42.pages.dev/symbol/heavy-star/)
- [SYM 1D412](https://theeduplaycampen.pages.dev/symbol/sym-1d412/)
- [ZODIAC CELESTIAL](https://theeduplaycampen.pages.dev/zodiac-celestial/)
- [SYM 1D48C](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1d48c/)
- [SYM 1D400](https://theeduplaycampen.pages.dev/symbol/sym-1d400/)
- [SYM 1D472](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d472/)
- [INSTAGRAM BIO](https://theeduplaycampen.pages.dev/pt/instagram-bio/)
- [SYM 260C](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-260c/)
- [SYM 2657](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-2657/)
- [SYM 1D453](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d453/)
- [STARS](https://coquette-aesthetic-symbols-86.pages.dev/ru/stars/)
- [SYM 1F494](https://theeduplaycampen.pages.dev/symbol/sym-1f494/)
- [SYM 1D459](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d459/)
- [SYM 1D456](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d456/)
- [SYM 1D499](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1d499/)
- [SYM 1D43E](https://theeduplaycampen.pages.dev/symbol/sym-1d43e/)
- [SYM 1F498](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1f498/)
- [SYM 1F635 200D 1F4AB](https://theeduplaycampen.pages.dev/symbol/sym-1f635-200d-1f4ab/)
- [SYM 1F47D](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1f47d/)
- [WHITE HEART](https://angelic-bow-symbols-42.pages.dev/symbol/white-heart/)
- [SYM 265E](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-265e/)
- [SYM 1D46E](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d46e/)
- [SYM 26C3](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26c3/)
- [SYM 1D47A](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d47a/)
- [SYM 26A5](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26a5/)
- [SYM 1D468](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d468/)
- [TAURUS ZODIAC BULL](https://coquette-aesthetic-symbols-86.pages.dev/symbol/taurus-zodiac-bull/)
- [HIGH VOLTAGE LIGHTNING](https://coquette-aesthetic-symbols-86.pages.dev/symbol/high-voltage-lightning/)
- [SYM 265D](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-265d/)
- [WHITE FLORETTE BLOSSOM](https://theeduplaycampen.pages.dev/symbol/white-florette-blossom/)
- [SYM 2725](https://theeduplaycampen.pages.dev/symbol/sym-2725/)
- [SYM 1D488](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1d488/)
- [SYM 1F600](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1f600/)
- [SYM 1F972](https://theeduplaycampen.pages.dev/symbol/sym-1f972/)
- [BEAMED EIGHTH NOTES](https://theeduplaycampen.pages.dev/symbol/beamed-eighth-notes/)
- [SYM 26E5](https://theeduplaycampen.pages.dev/symbol/sym-26e5/)
- [SYM 1D474](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d474/)
- [SYM 1D4A2](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d4a2/)
- [STARS](https://coquette-aesthetic-symbols-86.pages.dev/es/stars/)
- [SYM 26EC](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26ec/)
- [SYM 1D45C](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d45c/)
- [SYM 1D463](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d463/)
- [SYM 1FAE8](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1fae8/)
- [SYM 1D42A](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d42a/)
- [SWIMMING FISH RIGHT](https://theeduplaycampen.pages.dev/symbol/swimming-fish-right/)
- [RIGHT MATHEMATICAL WHITE SQUARE BRACKET](https://pearl-girly-fonts-86.pages.dev/symbol/right-mathematical-white-square-bracket/)
- [SYM 26B3](https://angelic-bow-symbols-42.pages.dev/symbol/sym-26b3/)
- [SYM 26E6](https://theeduplaycampen.pages.dev/symbol/sym-26e6/)
- [SYM 1D4A0](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d4a0/)
- [SYM 1D47F](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d47f/)
- [SYM 26AA](https://angelic-bow-symbols-42.pages.dev/symbol/sym-26aa/)
- [SYM 1D431](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d431/)
- [SYM 26A7](https://pearl-girly-fonts-86.pages.dev/symbol/sym-26a7/)
- [SYM 2613](https://nordic-minimal-fonts-67.pages.dev/symbol/sym-2613/)
- [SYM 1F498](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1f498/)
- [SYM 2621](https://clean-dot-aesthetic-48.pages.dev/symbol/sym-2621/)
- [DAGGER BLADE](https://sleek-line-symbols-51.pages.dev/symbol/dagger-blade/)
- [NATURE FLOWERS](https://sleek-line-symbols-51.pages.dev/vi/nature-flowers/)
- [SYM 1D480](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d480/)
- [SYM 26C9](https://cyberpunk-clan-tags-43.pages.dev/symbol/sym-26c9/)
- [LEO ZODIAC LION](https://coquette-aesthetic-symbols-86.pages.dev/symbol/leo-zodiac-lion/)
- [SYM 26EB](https://sleek-line-symbols-51.pages.dev/symbol/sym-26eb/)
- [SYM 1D441](https://sleek-line-symbols-51.pages.dev/symbol/sym-1d441/)
- [HEARTS](https://angelic-bow-symbols-42.pages.dev/pt/hearts/)
- [SCORPIO ZODIAC SCORPION](https://clean-dot-aesthetic-48.pages.dev/symbol/scorpio-zodiac-scorpion/)
- [BRACKETS](https://clean-dot-aesthetic-48.pages.dev/ru/brackets/)
- [RIGHT HEAVY BRACKET BOX](https://pearl-girly-fonts-86.pages.dev/symbol/right-heavy-bracket-box/)
- [BLACK STAR](https://clean-dot-aesthetic-48.pages.dev/symbol/black-star/)
- [MUSIC WEATHER](https://coquette-aesthetic-symbols-86.pages.dev/vi/music-weather/)
- [ARROWS LINES](https://theeduplaycampen.pages.dev/pt/arrows-lines/)
- [SYM 1F914](https://clean-dot-aesthetic-48.pages.dev/symbol/sym-1f914/)
- [WHITE HEART](https://pearl-girly-fonts-86.pages.dev/symbol/white-heart/)
- [SYM 26E0](https://angelic-bow-symbols-42.pages.dev/symbol/sym-26e0/)
- [NATURE FLOWERS](https://clean-dot-aesthetic-48.pages.dev/ru/nature-flowers/)
- [BLACK FOUR POINT STAR](https://clean-dot-aesthetic-48.pages.dev/symbol/black-four-point-star/)
- [ANTICLOCKWISE OPEN CIRCLE ARROW](https://sleek-line-symbols-51.pages.dev/symbol/anticlockwise-open-circle-arrow/)
- [SYM 1D498](https://sleek-line-symbols-51.pages.dev/symbol/sym-1d498/)
- [SYM 1D41F](https://sleek-line-symbols-51.pages.dev/symbol/sym-1d41f/)
- [AESTHETIC STARDUST COMBO](https://clean-dot-aesthetic-48.pages.dev/symbol/aesthetic-stardust-combo/)
- [STAR OPERATOR](https://sleek-line-symbols-51.pages.dev/symbol/star-operator/)
- [SYM 1D46D](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1d46d/)
- [SYM 1D407](https://sleek-line-symbols-51.pages.dev/symbol/sym-1d407/)
- [OPEN CENTRE STAR](https://clean-dot-aesthetic-48.pages.dev/symbol/open-centre-star/)
- [SYM 1D480](https://sleek-line-symbols-51.pages.dev/symbol/sym-1d480/)
- [SYM 1F972](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1f972/)
- [MUSIC SHARP SIGN](https://theeduplaycampen.pages.dev/symbol/music-sharp-sign/)
- [KAOMOJI](https://sleek-line-symbols-51.pages.dev/pt/kaomoji/)
- [INSTAGRAM BIO](https://clean-dot-aesthetic-48.pages.dev/pt/instagram-bio/)
- [SYM 26EB](https://pearl-girly-fonts-86.pages.dev/symbol/sym-26eb/)
- [SYM 26F1](https://clean-dot-aesthetic-48.pages.dev/symbol/sym-26f1/)
- [SYM 1FAE1](https://theeduplaycampen.pages.dev/symbol/sym-1fae1/)
- [SYM 1F62C](https://clean-dot-aesthetic-48.pages.dev/symbol/sym-1f62c/)
- [SYM 1F911](https://coquette-aesthetic-symbols-86.pages.dev/symbol/sym-1f911/)
- [SYM 26BA](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-26ba/)
- [SYM 1D400](https://angelic-bow-symbols-42.pages.dev/symbol/sym-1d400/)
- [SYM 26F0](https://cyberpunk-clan-tags-43.pages.dev/symbol/sym-26f0/)
- [SYM 1D496](https://lace-heart-kaomoji-64.pages.dev/symbol/sym-1d496/)
- [SYM 1F47D](https://clean-dot-aesthetic-48.pages.dev/symbol/sym-1f47d/)
