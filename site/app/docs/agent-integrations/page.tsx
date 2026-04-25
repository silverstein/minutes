import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Adding agent integrations — Minutes",
  description:
    "How to decide whether a new AI tool belongs in Minutes as files, MCP, portable skills, an agent command, or an OpenAI-compatible model backend.",
  alternates: {
    canonical: "/docs/agent-integrations",
  },
};

const surfaces = [
  ["Raw files", "The agent can read ~/meetings/ directly.", "Cursor, any local coding agent"],
  ["MCP server", "The host supports MCP tools, resources, and prompts.", "Claude Desktop, Codex, Gemini CLI"],
  ["Portable skills", "The host discovers Agent Skills-style .agents/skills folders.", "Codex, Gemini CLI, Pi"],
  ["Host-specific skills", "The host needs a different generated shape.", "Claude Code plugin, OpenCode commands"],
  ["agent_command backend", "Minutes should call the agent CLI for summaries.", "claude, codex, opencode, pi"],
  [
    "OpenAI-compatible backend",
    "Minutes should call a model API directly, not an agent loop.",
    "OpenRouter, Vercel AI Gateway, Cloudflare AI Gateway, llama.cpp",
  ],
  ["Routing eval", "The agent has a non-interactive prompt mode worth benchmarking.", "routing:agents -- --agent codex"],
] as const;

const modelBackends = [
  ["Ollama", "Local model runtime", "Already supported directly; can also be reached through its OpenAI-compatible endpoint."],
  ["llama.cpp", "Local model runtime", "Use an OpenAI-compatible base URL. Do not add it as an agent option."],
  ["vLLM / LM Studio / LocalAI", "Local or self-hosted runtime", "Use the same OpenAI-compatible path when available."],
  ["OpenRouter", "Cloud model router", "Preset candidate for one-key access to many providers."],
  ["Vercel AI Gateway", "Cloud model gateway", "Preset candidate for teams already using Vercel routing, billing, or AI SDK workflows."],
  ["Cloudflare AI Gateway", "Cloud model gateway", "Preset candidate for observability, rate limits, caching, retries, and Cloudflare routing."],
] as const;

const experimentalRuntimes = [
  ["Goose", "Open-source on-machine agent with CLI, API, MCP extensions, and custom OpenAI-compatible provider support."],
  ["Hermes Agent", "Persistent personal agent with gateway, memory, skills, browser control, OpenRouter, custom APIs, and local vLLM support."],
  ["OpenClaw", "Local-first personal automation gateway with messaging channels, tools, and daemon-style routing."],
  ["Aider", "Open-source terminal pair programmer with broad model support, including OpenRouter."],
  ["OpenHands", "Open-source agent platform and SDK with local and sandboxed deployment modes."],
] as const;

const checklist = [
  "Identify whether the host supports file reads, MCP, .agents skill discovery, host-specific skill discovery, and non-interactive CLI prompts.",
  "Reuse .agents/skills when the host already discovers it. Do not add a duplicate generated tree just for symmetry.",
  "Add a host-specific skill compiler target only when the host cannot consume the portable skill pack.",
  "For summarization backends, update summarize.rs, desktop settings, docs/CONFIG.md, and targeted invocation tests.",
  "For model providers, prefer one generic OpenAI-compatible backend with presets over one top-level engine per gateway.",
  "For routing evals, update tooling/skills/compiler/agent-routing.ts and run the routing-agent smoke where auth allows.",
  "Update README, /for-agents, product-surfaces.json, manifest.json, llms.txt, and any provider-specific docs.",
  "Run Rust, skill-tooling, llms, and site gates before shipping.",
] as const;

const current = [
  ["Claude Code", "Host-specific plugin surface plus MCP."],
  ["OpenCode", "Host-specific .opencode skills and commands, plus MCP when configured."],
  ["Codex", "Portable .agents skills plus MCP."],
  ["Gemini CLI", "Portable .agents skills plus MCP."],
  ["Pi coding agent", "Portable .agents skills plus opt-in agent_command = \"pi\" summarization. No .pi/skills export."],
  ["Cursor and editors", "Raw meeting files and MCP where the host supports it."],
] as const;

function SectionLabel({ label }: { label: string }) {
  return (
    <div className="mb-6 flex items-center gap-3">
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {label}
      </span>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

export default function AgentIntegrationsPage() {
  return (
    <div className="mx-auto max-w-[920px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP tools
          </a>
        </div>
      </div>

      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Agent integrations
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Add the smallest useful agent surface.
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Minutes is deliberately not one integration per agent. Most hosts can
          use the same local markdown, MCP server, or portable skill pack. Add a
          custom surface only when the host contract actually requires one.
        </p>
      </section>

      <section className="mt-12 grid gap-6 md:grid-cols-2">
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
          <p className="font-mono text-[12px] text-[var(--text)]">Agent hosts</p>
          <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
            Hosts run their own agent loop, prompt wrapper, tools, and memory.
            Add them to <span className="font-mono text-[var(--text)]">agent_command</span>{" "}
            only after a non-interactive stdout contract is proven.
          </p>
        </div>
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
          <p className="font-mono text-[12px] text-[var(--text)]">Model providers</p>
          <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
            Providers expose inference APIs. OpenRouter, Vercel AI Gateway,
            Cloudflare AI Gateway, and llama.cpp belong behind a generic
            OpenAI-compatible backend, not in the agent list.
          </p>
        </div>
      </section>

      <section className="mt-12">
        <SectionLabel label="Choose the surface" />
        <div className="overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
          <table className="w-full border-collapse text-[13px]">
            <thead>
              <tr className="border-b border-[color:var(--border)]">
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Surface
                </th>
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Use when
                </th>
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Examples
                </th>
              </tr>
            </thead>
            <tbody>
              {surfaces.map(([surface, useWhen, examples]) => (
                <tr key={surface} className="border-b border-[color:var(--border)] last:border-0">
                  <td className="px-4 py-3 font-mono text-[var(--text)]">{surface}</td>
                  <td className="px-4 py-3 leading-6 text-[var(--text-secondary)]">{useWhen}</td>
                  <td className="px-4 py-3 leading-6 text-[var(--text-secondary)]">{examples}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="mt-12">
        <SectionLabel label="Model backends" />
        <div className="overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)]">
          <table className="w-full border-collapse text-[13px]">
            <thead>
              <tr className="border-b border-[color:var(--border)]">
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Backend
                </th>
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Class
                </th>
                <th className="px-4 py-3 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  Posture
                </th>
              </tr>
            </thead>
            <tbody>
              {modelBackends.map(([backend, className, posture]) => (
                <tr key={backend} className="border-b border-[color:var(--border)] last:border-0">
                  <td className="px-4 py-3 font-mono text-[var(--text)]">{backend}</td>
                  <td className="px-4 py-3 leading-6 text-[var(--text-secondary)]">{className}</td>
                  <td className="px-4 py-3 leading-6 text-[var(--text-secondary)]">{posture}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="mt-12 grid gap-6 md:grid-cols-[1.1fr_0.9fr]">
        <div>
          <SectionLabel label="Checklist" />
          <ol className="space-y-3 text-[14px] leading-7 text-[var(--text-secondary)]">
            {checklist.map((item, index) => (
              <li key={item} className="flex gap-3">
                <span className="shrink-0 font-mono text-[var(--text-tertiary)]">
                  {index + 1}.
                </span>
                <span>{item}</span>
              </li>
            ))}
          </ol>
        </div>
        <div>
          <SectionLabel label="Current map" />
          <div className="space-y-3">
            {current.map(([name, detail]) => (
              <div
                key={name}
                className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-4"
              >
                <p className="font-mono text-[12px] text-[var(--text)]">{name}</p>
                <p className="mt-2 text-[13px] leading-6 text-[var(--text-secondary)]">
                  {detail}
                </p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="mt-12">
        <SectionLabel label="Experimental runtimes" />
        <div className="grid gap-3 sm:grid-cols-2">
          {experimentalRuntimes.map(([name, detail]) => (
            <div
              key={name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-4"
            >
              <p className="font-mono text-[12px] text-[var(--text)]">{name}</p>
              <p className="mt-2 text-[13px] leading-6 text-[var(--text-secondary)]">
                {detail}
              </p>
            </div>
          ))}
        </div>
        <p className="mt-4 text-[13px] leading-6 text-[var(--text-tertiary)]">
          These should stay in docs or recipes until a spike verifies read-only
          behavior, non-interactive execution, stable stdout, and permission
          boundaries.
        </p>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5">
        <SectionLabel label="Repo checklist" />
        <p className="text-[14px] leading-7 text-[var(--text-secondary)]">
          The full contributor checklist lives in{" "}
          <a
            href="https://github.com/silverstein/minutes/blob/main/docs/AGENT-INTEGRATIONS.md"
            className="text-[var(--accent)] hover:underline"
          >
            docs/AGENT-INTEGRATIONS.md
          </a>
          . It names the exact files to update for compiler hosts,
          summarization backends, routing evals, and generated public agent docs.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
