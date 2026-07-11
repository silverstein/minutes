# Cowork and Dispatch Research

Last updated: 2026-03-19

This note captures what we can verify today about Claude Cowork and Dispatch from current public documentation, plus what that means for Minutes.

## Verified

### Cowork runtime model

- Cowork is a research preview that brings Claude Code's agentic architecture into Claude Desktop for knowledge work beyond coding.
- Cowork is available on Claude Desktop for macOS and Windows x64 on paid plans.
- Cowork runs on the user's computer, not purely in the cloud.
- Cowork can access local files, coordinate sub-agents, and run long-lived tasks.
- Cowork stores conversation history locally on the user's computer.
- Cowork requires the desktop app to remain open while work is happening.

### Plugin and connector model

- Cowork has a first-class plugin model.
- Plugins in Cowork bundle skills, connectors, and sub-agents.
- Plugins are installed from the Cowork UI and can also be uploaded manually.
- User-added plugins are saved locally to the machine.
- The Customize surface in Cowork explicitly groups plugins, skills, and connectors together.
- Plugins can expose slash-command style skills inside Cowork.

### Dispatch model

- Dispatch gives the user one continuous Cowork conversation that is reachable from both phone and desktop.
- Dispatch works by having Claude operate on the desktop computer using the files, connectors, and plugins already configured in Cowork.
- Dispatch requires both the Claude Desktop app and the Claude mobile app.
- The user's desktop must be awake and the Claude Desktop app must be open.
- Dispatch currently has one continuous thread, not multiple independent threads.
- Dispatch currently has no completion notifications on mobile or desktop.

## Verified Sources

- Cowork overview: <https://support.claude.com/en/articles/13345190-get-started-with-cowork>
- Cowork plugins: <https://support.claude.com/en/articles/13837440-use-plugins-in-cowork>
- Dispatch / mobile task assignment: <https://support.claude.com/en/articles/13947068-assign-tasks-to-claude-from-anywhere-in-cowork>
- Claude Code plugin reference: <https://docs.claude.com/en/docs/claude-code/plugins-reference>
- External field report: <https://www.macstories.net/stories/hands-on-with-claude-dispatch-for-cowork/>

## Local Findings on This Machine

The local Claude Desktop install gives us a few stronger signals than the public docs alone:

- Claude stores installed extension packages under:
  - `~/Library/Application Support/Claude/Claude Extensions/`
- Per-extension settings live under:
  - `~/Library/Application Support/Claude/Claude Extensions Settings/`
- The machine already has multiple installed extensions with manifests and MCP-style server packaging, including:
  - Anthropic-built extensions like Filesystem, Notes, Word, Excel, PowerPoint
  - third-party / local extensions with `manifest.mcpb.json`
- A sampled installed extension manifest includes:
  - declared tool metadata
  - a `server.mcp_config` block with command/args/env
  - user-configurable fields
- `claude_desktop_config.json` on this machine contains only high-level preferences and does not appear to be the place where arbitrary user MCP servers are being surfaced into Cowork.
- Local Cowork / agent-mode session artifacts contain:
  - `enabledMcpTools`
  - a `cowork_plugins/` directory
- A sampled local session had 200+ enabled tool IDs and **did not** contain any Minutes tool IDs.
- There is currently **no installed Minutes extension** under `~/Library/Application Support/Claude/Claude Extensions/`.

Current local conclusion:

- The strongest available evidence on this machine points toward an extension/plugin-oriented integration model for Cowork, not “raw Minutes MCP server automatically appears everywhere.”
- Minutes should be prepared to package a Cowork-facing extension or plugin surface if we want first-class Cowork behavior.
- The repo now includes a proof-of-life Cowork extension bundle scaffold under `integrations/claude-cowork-extension/` plus `scripts/build_cowork_extension.sh` so the next verification step is install-and-check, not inventing the package format from scratch.
- As of 2026-04-09, the local extension bundle build itself is verified via `scripts/build_cowork_extension.sh` and `scripts/check_cowork_extension.sh`. What remains unverified is the in-Cowork install/runtime path, not the bundle build.

## Important Unknowns

These are the biggest things the public docs do not fully settle:

### 1. Are raw user MCP servers automatically available inside Cowork?

Our plan text currently assumes that "MCPB tools are automatically available in Cowork."

What we can verify:

- Cowork definitely supports plugins.
- Cowork plugins definitely bundle skills, connectors, and sub-agents.
- The Claude Code plugin reference documents MCP server packaging for Claude Code plugins.

What we cannot yet verify from current public docs:

- Whether a user's existing non-plugin MCP server configuration is surfaced inside Cowork automatically.
- Whether plugin-bundled MCP servers are fully supported in Cowork the same way they are in Claude Code.
- Whether Cowork exposes MCP tools directly, or only indirectly through plugin skills and agent flows.

Current conclusion:

- We should not assume that a bare user-configured MCP server is enough for first-class Cowork support.
- The safer product bet is to expect that Minutes may need a Cowork-oriented plugin package or skill surface, even if the underlying logic still rides on MCP.

### 2. How reliable is Dispatch for multi-step, local-file-heavy workflows?

What we can verify:

- Dispatch reuses the desktop's configured access surface.
- Dispatch is still explicitly a research preview.
- There are currently no completion notifications.

What we cannot verify from public docs:

- Success rate for long-running local workflows like "start recording now, wait, process later, return summary."
- Whether background/offline transitions interrupt a recording workflow cleanly.
- How gracefully Cowork/Dispatch handles local permission prompts or desktop sleep during a task.

Current conclusion:

- Dispatch-triggered recording should be treated as an experiment, not a default user path, until we test it manually.

## Product Implications for Minutes

### What we can build confidently now

- Keep the core Minutes tool surface local-first and desktop-native.
- Continue exposing durable artifacts and intelligence through CLI and MCP.
- Treat Cowork as a workflow layer on top of Minutes, not a replacement runtime.
- Plan around Cowork skills / plugin packaging rather than assuming a transparent MCP pass-through.

### What should remain research-first

- Remote recording from Dispatch.
- Mobile post-processing summaries through Dispatch.
- Any workflow that depends on Cowork reliably surfacing Minutes tools without packaging work.

### What this means for roadmap sequencing

Recommended order:

1. Prove how Minutes should appear inside Cowork:
   - direct MCP exposure if it works
   - plugin-wrapped skills if it does not
2. After that, test a narrow Dispatch proof-of-life flow:
   - trigger something simple
   - observe state propagation
   - verify failure modes
3. Only then attempt phone-triggered live recording.

## Recommended Follow-on Work

### High confidence

- Package the most important Minutes workflows as Cowork-friendly skills and plugin-facing commands.
- Add a manual Cowork proof-of-life checklist before deeper automation.

### Avoid assuming

- Do not assume Dispatch is good enough yet for the flagship "phone starts Mac recording" story.
- Do not assume arbitrary MCP tools are visible in Cowork without plugin packaging.

## Current Recommendation

For Minutes, the next frontier should not be "build everything for Cowork and hope the surface exists."

It should be:

1. verify the actual Cowork integration surface for Minutes with a local plugin-oriented test,
2. then run a narrow Dispatch proof-of-life workflow,
3. then decide whether recording, summaries, and follow-up automation should be MCP-native, plugin-native, or hybrid.
