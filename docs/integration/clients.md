# Claude integration

minutes is a native extension for the Claude ecosystem. **No API keys needed** — Claude summarizes your meetings when you ask, using your existing Claude subscription.

```
You: "Summarize my last meeting"
Claude: [calls get_meeting] → reads transcript → summarizes in conversation

You: "What did Alex say about pricing?"
Claude: [calls search_meetings] → finds matches → synthesizes answer

You: "Any open action items for me?"
Claude: [calls list_meetings] → scans frontmatter → reports open items
```

### Any MCP client (Claude Code, Codex, OpenCode, Gemini CLI, Claude Desktop, or your own agent)

Minutes exposes a standard MCP server. Point any MCP-compatible client at it:

```json
{
  "mcpServers": {
    "minutes": {
      "command": "npx",
      "args": ["minutes-mcp"]
    }
  }
}
```

#### Transports

`minutes-mcp` speaks stdio by default, which is what the config above uses: the client spawns the server as a subprocess and owns its lifetime. Nothing changes for existing setups.

Pass `--transport http` to run one long-lived Streamable HTTP server instead, so several clients share a single process rather than each spawning their own:

```bash
minutes-mcp --transport http --port 7373
```

Then point clients at the endpoint:

```json
{
  "mcpServers": {
    "minutes": {
      "type": "http",
      "url": "http://127.0.0.1:7373/mcp"
    }
  }
}
```

Flags: `--port` (default `7373`; `0` asks the OS for a free port and the chosen one is printed to stderr), `--max-sessions` (default 16). Each accepts an environment variable instead: `MINUTES_MCP_TRANSPORT`, `MINUTES_MCP_PORT`, `MINUTES_MCP_MAX_SESSIONS`. `GET /health` reports live session count.

Request bodies are JSON-RPC envelopes, so they are capped at 4 MiB; anything larger gets a JSON-RPC error with HTTP 413. A client that disconnects without sending `DELETE /mcp` leaves its session behind, so a session with no request in flight and no open stream is reclaimed after 30 minutes idle and answers 404 after that. A client holding an open stream is never reclaimed, however quiet it is.

HTTP mode has no authentication, so the bind address is always `127.0.0.1` and there is no flag to change it. Only this machine can reach the endpoint, and it rejects requests whose `Host` or `Origin` is not loopback. That second check matters: binding to loopback does not by itself stop a web page you have open from POSTing to a local port.

To reach it from another machine, front it with a reverse proxy and put authentication there. The proxy has to rewrite `Host` to the upstream address, since the original name would fail the loopback check. In Caddy:

```
reverse_proxy 127.0.0.1:7373 {
  header_up Host {upstream_hostport}
}
```

Native MCP clients send no `Origin`, so nothing else is needed for them. A browser-based client would also need its `Origin` handled at the proxy, which gives up the CSRF defense described above.

Canonical MCP reference now lives at:

- <https://useminutes.app/docs/mcp/tools>
- <https://useminutes.app/docs/mcp/tools.md>
- <https://useminutes.app/llms.txt>

The MCP surface currently includes recording control, meeting search/retrieval, policy-bound person profiles, structured insights, live transcript reading, dictation, QMD integration, and an interactive dashboard resource. Tool names, resource URIs, and prompt templates are generated from the live product surface instead of hand-maintained in this README.

**Interactive dashboard (Claude Desktop):** tools render an inline interactive UI via [MCP Apps](https://modelcontextprotocol.io/specification/2025-03-26/server/utilities/apps) — meeting list with filter/search, detail view with fullscreen + "Send to Claude" context injection, bounded relationship maps, and consistency reports. Text-only clients see the same data as plain text.

### Claude Code inside the Minutes desktop app

Opening the assistant Terminal automatically exposes the bundled Minutes skills,
including `/minutes-brief`, `/minutes-prep`, `/minutes-search`, and
`/minutes-weekly`. No marketplace installation is required inside the app.

Minutes refreshes a managed project-skill directory at
`~/.minutes/assistant/.minutes-skill-pack/.claude/skills` from the generated
portable pack, including its scripts and references. It passes the pack's root
to Claude Code with `--add-dir`. Helpers resolve from the assistant workspace's
`.agents/skills/minutes` mirror. Your workspace and global `.claude` settings,
custom skills, and legacy configuration symlinks remain user-owned.

This uses Claude Code's documented [additional-directory skill discovery](https://code.claude.com/docs/en/skills#load-skills-from-a-directory-outside-the-project).
The pack does not register another MCP server or enable plugin hooks. Private
Recall Chat retains its separate provider isolation. To use Minutes in Claude
Code outside the desktop app, install the [Minutes plugin](agent-integrations.md).

After upgrading from a version affected by missing skills, end the assistant
session and open a new Terminal session once so it receives the added directory.
If your configured Claude arguments exclude the `project` setting source or
enable safe/bare mode, Claude intentionally does not load project skills.

### OpenCode CLI

Minutes now ships a project-local OpenCode integration layer:

- `.opencode/skills/minutes-*` for OpenCode's one-level skill discovery
- `.opencode/commands/minutes-*.md` so you can run native slash commands like `/minutes-brief`
- the same portable runtime helpers used by the Codex/Gemini skill pack

OpenCode also reads this repo's `AGENTS.md`, so the project rules carry over automatically.

For MCP tools in OpenCode, the official CLI flow is:

```bash
opencode mcp add
```

Choose a local stdio server and point it at:

```bash
npx minutes-mcp
```

If you're wiring OpenCode against this repo before the next npm release is cut,
point it at the repo-local entrypoint instead:

```bash
npm --prefix /absolute/path/to/minutes/crates/mcp exec tsx src/index.ts
```

For the native skill/command workflow, just launch OpenCode in this repo:

```bash
opencode
```

Then use commands like:

```text
/minutes-brief
/minutes-prep Alex
/minutes-debrief
/minutes-weekly
/minutes-video-review /absolute/path/to/demo.mp4
```

### Pi coding agent

Minutes works with Mario Zechner's `pi` coding agent in two places:

- `engine = "agent"` can call `pi` directly for local meeting summarization.
- The desktop Recall panel can launch Pi when `[assistant].agent = "pi"`.
- Pi auto-discovers this repo's existing `.agents/skills/minutes/` skill pack, so there is no separate `.pi/skills` tree to keep in sync.

Install Pi, log in or configure a provider, then set:

```toml
[summarization]
engine = "agent"
agent_command = "pi"
```

Minutes invokes Pi in non-interactive, no-tools mode with a private prompt file. Configure provider/model defaults in Pi itself; Minutes does not currently forward extra `[summarization]` CLI flags. That keeps summarization opt-in and prevents the agent from writing to the repo while it is turning a transcript into notes.

For the interactive Recall panel, Minutes launches Pi directly and passes `[assistant].agent_args` through. Pi still owns provider auth and model selection: use Pi's `/login` and `/model` flows first. If a GitHub Copilot model reports that personal access tokens are unsupported, refresh the Pi Copilot login instead of adding a GitHub PAT to Minutes.

This is separate from Inflection's Pi chatbot/model. Inflection's Pi models are optimized for warmth and emotional intelligence, but the Inflection API terms say not to send regulated personal data. Meeting transcripts often contain personal data, so Minutes does not route transcripts to Inflection by default.

### Mistral Vibe

Add Minutes to your `~/.vibe/config.toml`:

```toml
[[mcp_servers]]
name = "minutes"
transport = "stdio"
command = "npx"
args = ["minutes-mcp"]
```

Minutes tools are available in Vibe as `minutes_*` (e.g. `minutes_start_recording`, `minutes_search_meetings`).

### Claude Code (Plugin)

Install the plugin from the marketplace:
```bash
# First-time install
claude plugin marketplace add silverstein/minutes
claude plugin install minutes
# Restart Claude Code to load skills, hooks, and the meeting-analyst agent
```

**Upgrading?** `claude plugin marketplace add` is a no-op when the marketplace is already on disk — it won't fetch new versions. To pick up new skills and hooks after a release, refresh the marketplace mirror first, then update the plugin:
```bash
claude plugin marketplace update minutes    # git pulls the local marketplace mirror
claude plugin update minutes@minutes        # installs the new version into the cache
# Restart Claude Code to apply
```

23 skills, 1 agent, 2 hooks:
```
├── Capture:      /minutes-record, note, list, recap, cleanup, verify, setup
├── Search:       /minutes-search
├── Lifecycle:    /minutes-brief, prep, debrief, weekly
├── Coaching:     /minutes-tag, mirror
├── Knowledge:    /minutes-ideas, lint, ingest
├── Intelligence: /minutes-graph
├── Artifacts:    /minutes-video-review
├── Agent:        meeting-analyst (cross-meeting intelligence)
└── Hooks:        SessionStart meeting briefings + PostToolUse recording alerts
```

**Meeting lifecycle skills** — inspired by [gstack](https://github.com/garrytan/gstack)'s interactive skill pattern:

```
/minutes-brief                      → fast one-pager (or fired automatically by hook 15 min before calls)
  ↓
/minutes-prep "call with Alex"      → deeper relationship brief + talking points + goal-setting
  ↓
minutes record → minutes stop       → hook alerts if decisions conflict with prior meetings
  ↓
/minutes-tag won|lost|stalled       → 5-second outcome label (unlocks mirror correlation)
  ↓
/minutes-debrief                    → "You wanted to resolve pricing. Did you?"
  ↓
/minutes-mirror                     → talk-time, hedging, what your winning meetings have in common
  ↓
/minutes-weekly                     → themes, decision arcs, stale items, Monday brief
  ↓
/minutes-video-review <video-or-url> → durable artifact bundle from a Loom, ScreenPal, or local walkthrough
  ↓
/minutes-graph "everyone who mentioned Stripe"  → cross-meeting entity queries
```

For the stable public agent-facing docs surface, use:

- <https://useminutes.app/for-agents>
- <https://useminutes.app/docs/mcp/tools>
- <https://useminutes.app/docs/errors>

### Minutes Desktop Assistant

The Tauri menu bar app includes a built-in AI Assistant window backed by the
same local meeting artifacts. It runs as a singleton assistant session:

- `AI Assistant` opens or focuses the persistent assistant window
- `Discuss with AI` reuses that same assistant and switches its active meeting focus
- Recall writes matching `CLAUDE.md` and `AGENTS.md` instructions into its assistant workspace so Claude-style and AGENTS.md-aware terminal agents get the same meeting context
- Updates remain manual until a hosted release manifest and rollback UX exist; auto-update stays off

### Cowork / Dispatch
The currently verified path for Cowork is plugin-oriented, not “raw MCP automatically appears everywhere.” Minutes ships a Cowork extension scaffold under `integrations/claude-cowork-extension/` and a local bundle build script at `scripts/build_cowork_extension.sh`. On this machine, the bundle build is verified; actual in-Cowork install/use remains a proof-of-life workflow, not a guaranteed default path. Treat Dispatch-triggered recording and other mobile workflows as experimental until the plugin-native path is installed and checked end to end.
