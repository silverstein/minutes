# Adding agent integrations

Minutes has several agent surfaces. Do not add a new agent by copying an existing
integration wholesale. Pick the smallest surface that matches what the host
actually supports.

## Surfaces

| Surface | Use when | Examples |
|---|---|---|
| Raw files | The agent can read `~/meetings/` directly. | Cursor, any local coding agent |
| MCP server | The host supports MCP tools/resources/prompts. | Claude Desktop, Codex, Gemini CLI |
| Portable skills | The host discovers Agent Skills-style `.agents/skills` folders. | Codex, Gemini CLI, Pi |
| Host-specific skills | The host needs a different generated shape. | Claude Code plugin, OpenCode commands |
| `agent_command` backend | Minutes should call the agent CLI for summaries. | `claude`, `codex`, `opencode`, `pi` |
| OpenAI-compatible model backend | Minutes should call a model API directly, not an agent CLI. | OpenRouter, Vercel AI Gateway, Cloudflare AI Gateway, llama.cpp, vLLM |
| Routing eval | The agent has a non-interactive prompt mode worth benchmarking. | `npm --prefix tooling/skills run routing:agents -- --agent codex` |

## Agent hosts vs model providers

Do not treat every AI brand as an agent integration. Minutes has two separate
contracts:

- Agent hosts run their own agent loop, tool policy, memory, and prompt wrapper.
  They belong in `agent_command` only when Minutes can safely invoke them
  non-interactively and capture stdout.
- Model providers expose inference APIs. They belong behind a direct
  summarization backend, ideally a generic OpenAI-compatible backend with
  provider presets.

OpenCode is an agent host. It can use many providers internally, but that only
covers the `agent_command = "opencode"` path. It does not replace Minutes
supporting direct model backends for users who want summaries without an
external coding-agent loop.

## Model backend candidates

Prefer one generic OpenAI-compatible backend over one engine per provider. That
keeps settings small and lets advanced users bring their own gateway or local
runtime.

| Backend | Classification | Notes |
|---|---|---|
| Ollama | Local model runtime | Already supported directly as `engine = "ollama"`; can also be reached through its OpenAI-compatible endpoint. |
| llama.cpp / llama-cpp-python | Local model runtime | Support through an OpenAI-compatible `base_url`; do not add as an agent option. |
| vLLM / LM Studio / LocalAI | Local or self-hosted runtime | Support through the same OpenAI-compatible path when available. |
| OpenRouter | Cloud model router | Good preset for one-key access to many providers; transcripts leave the machine. |
| Vercel AI Gateway | Cloud model gateway | Good preset for hosted apps and teams already using Vercel; transcripts leave the machine. |
| Cloudflare AI Gateway | Cloud model gateway | Good preset for observability, rate limits, caching, retries, and Cloudflare-managed routing; transcripts leave the machine unless the upstream is local/private. |

Recommended config shape for future direct backends:

```toml
[summarization]
engine = "openai-compatible"
model = "anthropic/claude-sonnet-4.6"
base_url = "https://gateway.example.com/v1"
api_key_env = "AI_GATEWAY_API_KEY"
```

## Checklist

1. Identify the host contract.
   - Can it read files?
   - Does it support MCP?
   - Does it auto-discover `.agents/skills`?
   - Does it require a host-specific skill tree?
   - Does it have a non-interactive CLI mode?

2. If the host can reuse `.agents/skills`, do not generate a duplicate tree.
   Duplicate skill names can create collisions and make the agent less reliable.

3. If the host needs a generated skill surface, update:
   - `tooling/skills/schema.ts`
   - `tooling/skills/hosts/`
   - `tooling/skills/compiler/render.ts`
   - `tooling/skills/compiler/compile.ts`
   - `tooling/skills/compiler/check.ts`
   - `tooling/skills/compiler/golden.ts`
   - generated outputs under the host-specific tree

4. If the host should be callable from Minutes summarization, update:
   - `crates/core/src/summarize.rs`
   - targeted `prepare_agent_invocation_*` tests
   - `tauri/src-tauri/src/commands.rs`
   - `tauri/src/index.html`
   - `docs/CONFIG.md`

5. If adding a direct model backend rather than an agent host, update:
   - `crates/core/src/summarize.rs`
   - `SummarizationConfig` in `crates/core/src/config.rs`
   - desktop settings and validation in `tauri/src-tauri/src/commands.rs`
   - `tauri/src/index.html`
   - `docs/CONFIG.md`
   - provider-specific docs only when there are real caveats

6. If the host should participate in routing evals, update:
   - `tooling/skills/compiler/agent-routing.ts`
   - `tooling/skills/compiler/agent-routing.test.ts` if parsing or unavailable handling changes

7. Update public and agent-facing docs:
   - `README.md`
   - `site/app/for-agents/page.tsx`
   - `site/lib/product-surfaces.json`
   - `manifest.json`
   - `docs/CONFIG.md`
   - `docs/<agent>.md` when the host has provider-specific caveats
   - run `node scripts/generate_llms_txt.mjs`

8. Run the relevant gates:
   - `cargo fmt`
   - targeted Rust tests for the invocation path
   - `cargo check -p minutes-app`
   - `npm --prefix tooling/skills run build`
   - `npm --prefix tooling/skills run compile:dry`
   - `npm --prefix tooling/skills run check`
   - `npm --prefix tooling/skills run test`
   - `npm --prefix site run check:llms`
   - `npm --prefix site run build` when site pages changed

## Current agent classes

- Claude Code: host-specific plugin surface plus MCP.
- OpenCode: host-specific `.opencode/skills` and `.opencode/commands`, plus MCP when configured.
- Codex: portable `.agents/skills` plus MCP.
- Gemini CLI: portable `.agents/skills` plus MCP.
- Pi coding agent: portable `.agents/skills` plus opt-in `agent_command = "pi"` summarization. No separate `.pi/skills` tree.
- Cursor and other editors: raw meeting files and MCP where the host supports it.

## Experimental runtime watchlist

These are real enough to mention but should not become first-class UI options
until a small spike proves the command contract:

| Runtime | Why it matters | Integration posture |
|---|---|---|
| Goose | Open-source on-machine agent with CLI, API, MCP extensions, and custom OpenAI-compatible provider support. | Spike as `agent_command` only if `goose` has a clean non-interactive run mode for transcript summaries. |
| Hermes Agent | Persistent personal agent with gateway, memory, skills, browser control, OpenRouter, custom APIs, and local vLLM support. | Treat as an experimental agent host and portable-skills target; verify non-interactive stdout behavior first. |
| OpenClaw | Local-first personal automation gateway with many messaging channels, tools, and daemon-style routing. | Prefer webhook/notification recipes first; only add `agent_command` after a security review and command-contract spike. |
| Aider | Open-source terminal pair programmer with broad model support, including OpenRouter. | Coding-focused; verify read-only, non-editing summarization behavior before exposing. |
| OpenHands | Open-source agent platform and SDK with local and sandboxed deployment modes. | Advanced/platform integration, not a simple dropdown peer to Codex or OpenCode. |

When in doubt, prefer the raw file or MCP path first. Add a custom host surface
only when the agent cannot consume the existing portable one.
