# Minutes vs Hyprnote (Anarlog)

Last reviewed: 2026-06-10

## Quick verdict

- Choose **Hyprnote** if you want a polished local notepad for taking and enhancing your own meeting notes, and the app itself is where you want to live.
- Choose **Minutes** if you want a durable, agent-readable corpus: files on your disk, MCP tools, a CLI, and consent and provenance metadata your tools can rely on.

## Where Hyprnote wins

- The in-meeting note-taking experience is the product: you write, it listens and enhances
- Larger community today with a tight focus on the notepad job
- Simpler if you only need meeting notes (no voice memos, dictation, or agent surface)

## Where Minutes wins

- Built for what happens after the meeting: structured markdown with YAML frontmatter that agents query across months of conversations
- Broader agent surface: MCP server, CLI, SDK, live transcript reads, Claude Code plugin
- Governance lives in the data: consent basis stamped into every recording, with sensitive no-capture meetings and agent-enforced sensitivity on the roadmap

## When Minutes is not the right fit

- When you mainly want to write notes during meetings and have AI clean them up
- When MCP, CLIs, and agent memory are not part of your workflow

## Notes

Both projects are open source (MIT) and process audio locally, so the privacy floor is similar. The fork in the road is the output contract: Hyprnote's durable artifact is your enhanced notes; Minutes' durable artifact is a diarized transcript plus extracted decisions, action items, and people, written as plain files that outlive any one app. Running both is coherent.

Full comparison: https://useminutes.app/compare/hyprnote-vs-minutes
