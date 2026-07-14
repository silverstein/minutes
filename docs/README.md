# Minutes Documentation Structure

Documentation is organized by purpose under `docs/`. This index describes each folder and how to contribute new docs.

## Folder Organization

### `docs/architecture/`
Technical subsystem documentation: transcription engines, audio capture, config reference, schema definitions.
- When to add: New technical subsystem, design of a component, configuration reference
- Examples: `parakeet.md`, `config.md`, `apple-speech.md`, `audio-devices.md`

### `docs/checklists/`
Pre-flight checklists and regression tests for critical processes.
- When to add: Verification checklist, regression guard, quality gate
- Examples: `pre-commit.md`, `compatibility-checklist.md`, `call-capture-regression-checklist.md`

### `docs/development/`
Developer guides, HOWTOs, build status, and productivity documentation.
- When to add: Development workflow, debugging guide, setup instructions, progress tracking
- Examples: `desktop-development.md`, `debug-dictation-hotkey.md`, `todos.md`, `build-status.md`

### `docs/investigations/`
One-off investigations, evaluation results, handoff notes, and research documents.
- When to add: Single-use debugging session, evaluation of an approach, handoff to another contributor
- Examples: `call-capture-handoff-2026-04-08.md`, `auto-update-evaluation.md`, `cristy-blank-transcript-investigation.md`

### `docs/release/`
Release process, platform-specific release guidance, version channels, and release notes history.
- When to add: Release procedures, platform-specific instructions, version history, release tooling
- Subfolders:
  - `docs/release/notes/` — Versioned release notes (one file per version)
- Examples: `procedure.md`, `platform-macos.md`, `platform-windows.md`, `channels.md`

### `docs/integration/`
Agent integration guides and agent-specific documentation.
- When to add: Integration with a new agent system, agent capability documentation
- Examples: `agent-integrations.md`, `pi-agent.md`

### `docs/plans/`, `docs/designs/`, `docs/rfcs/`, `docs/eval/`
Already organized by type — keep as-is. Plans are forward-looking implementation plans, designs are spike results and benchmarks, RFCs are design proposals, and eval contains evaluation results.

## Root-Level Docs (do NOT move these)

- **`docs/coach.md`** — Product guide for the live Coach copilot, controls, privacy, and graceful degradation
- **`PLAN.md`** — Master project plan (architecture, vision, competitive landscape). Referenced as the "read this first" doc in CLAUDE.md.
- **`README.md`** — Project landing page
- **`CLAUDE.md`** — Developer guide (tool setup, build commands, architecture decisions, ecosystem integration)
- **`AGENTS.md`** — Agent-specific instructions
- **`DESIGN.md`** — Design system (fonts, colors, spacing, visual direction)
- **`CONTRIBUTING.md`** — Contributing guidelines

## Naming Conventions

- Filenames: lowercase with hyphens (`desktop-development.md`, not `Desktop-Development.md`)
- Internal links in docs use relative paths when moving between docs (`../release/channels.md` from `development/`)
- Cross-repo links use full GitHub URLs (`https://github.com/silverstein/minutes/blob/main/docs/architecture/parakeet.md`)

## Adding a New Doc

1. Determine its purpose and find the appropriate folder
2. Name it descriptively in lowercase with hyphens
3. If it references other docs, use relative paths (`../folder/doc.md`)
4. Update any docs that link to it, or add a reference in the appropriate section's README
5. If it's referenced in CLAUDE.md or README.md, update those links
6. Do NOT commit until all cross-references are verified to be correct
