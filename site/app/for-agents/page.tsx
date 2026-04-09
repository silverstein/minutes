import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "minutes for agents",
  description:
    "Agent-facing setup and reference entry point for Minutes: llms.txt, MCP install, host surfaces, and canonical links.",
  alternates: { canonical: "/for-agents" },
};

const surfaces = [
  {
    name: "Claude Code",
    bestFor: "Plugin skills plus MCP-powered meeting memory",
    path: "Install the Minutes plugin or point Claude Code at `npx minutes-mcp`.",
  },
  {
    name: "Codex",
    bestFor: "MCP access to recordings, search, insights, and relationship memory",
    path: "Use the standard MCP server via `npx minutes-mcp`.",
  },
  {
    name: "Claude Desktop",
    bestFor: "Interactive MCP App dashboard and natural-language meeting recall",
    path: "Use the Minutes MCP server or marketplace bundle.",
  },
  {
    name: "Gemini CLI",
    bestFor: "MCP-based meeting memory in a terminal workflow",
    path: "Use the standard MCP server via `npx minutes-mcp`.",
  },
] as const;

export default function ForAgentsPage() {
  return (
    <div className="mx-auto max-w-[840px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/llms.txt" className="hover:text-[var(--accent)]">
            llms.txt
          </a>
          <a href="https://github.com/silverstein/minutes" className="hover:text-[var(--accent)]">
            GitHub
          </a>
        </div>
      </div>

      <section className="max-w-[720px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Agent Entry Point
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Minutes for agents
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Minutes is local conversation memory for AI systems. It records meetings and voice memos,
          transcribes them on-device, stores structured markdown, and exposes recall through MCP so
          Claude, Codex, Gemini, and other clients can query what happened instead of guessing.
        </p>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Start Here
        </p>
        <div className="mt-4 space-y-3 font-mono text-[13px] leading-7 text-[var(--text)]">
          <div>
            <span className="text-[var(--text-secondary)]">MCP install:</span> <code>npx minutes-mcp</code>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">Agent index:</span>{" "}
            <a href="/llms.txt" className="text-[var(--accent)] hover:underline">
              /llms.txt
            </a>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">MCP reference:</span>{" "}
            <a href="/docs/mcp/tools" className="text-[var(--accent)] hover:underline">
              /docs/mcp/tools
            </a>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">Markdown mirror:</span>{" "}
            <a href="/docs/mcp/tools.md" className="text-[var(--accent)] hover:underline">
              /docs/mcp/tools.md
            </a>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">Full agent index:</span>{" "}
            <a href="/llms-full.txt" className="text-[var(--accent)] hover:underline">
              /llms-full.txt
            </a>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">Error reference:</span>{" "}
            <a href="/docs/errors" className="text-[var(--accent)] hover:underline">
              /docs/errors
            </a>
          </div>
          <div>
            <span className="text-[var(--text-secondary)]">Privacy:</span>{" "}
            <a href="/privacy.html" className="text-[var(--accent)] hover:underline">
              /privacy.html
            </a>
          </div>
        </div>
      </section>

      <section className="mt-14">
        <div className="mb-6 flex items-center gap-3">
          <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
            Surfaces
          </span>
          <div className="h-px flex-1 bg-[var(--border)]" />
        </div>
        <div className="grid gap-4 sm:grid-cols-2">
          {surfaces.map((surface) => (
            <div
              key={surface.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {surface.name}
              </p>
              <p className="mt-3 text-[15px] font-medium text-[var(--text)]">{surface.bestFor}</p>
              <p className="mt-2 text-[14px] leading-7 text-[var(--text-secondary)]">
                {surface.path}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <div className="mb-6 flex items-center gap-3">
          <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
            What Exists
          </span>
          <div className="h-px flex-1 bg-[var(--border)]" />
        </div>
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            The stable agent-facing contract today is the MCP server plus the generated{" "}
            <a href="/llms.txt" className="text-[var(--accent)] hover:underline">
              llms.txt
            </a>
            ,{" "}
            <a href="/llms-full.txt" className="text-[var(--accent)] hover:underline">
              llms-full.txt
            </a>
            , and{" "}
            <a href="/docs/mcp/tools" className="text-[var(--accent)] hover:underline">
              /docs/mcp/tools
            </a>
            ,{" "}
            <a href="/docs/errors" className="text-[var(--accent)] hover:underline">
              /docs/errors
            </a>
            . The wider public docs center is still being built, but the MCP surface now has a
            generated canonical reference.
          </p>
          <p>
            Minutes is strongest when you want local processing, inspectable markdown output, and
            durable memory your assistant can query later. It is weaker if you are looking for a full
            hosted video-meeting platform or a cloud note-taking bot that joins calls for you.
          </p>
        </div>
      </section>
    </div>
  );
}
