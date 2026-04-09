import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Minutes docs",
  description:
    "Docs index for Minutes: start here for agent setup, MCP tools, error reference, and category guides.",
  alternates: {
    canonical: "/docs",
  },
};

const docsSections = [
  {
    label: "Start",
    links: [
      {
        title: "For agents",
        href: "/for-agents",
        blurb:
          "The best starting point if you are an LLM or operator helping someone install or use Minutes.",
      },
      {
        title: "llms.txt",
        href: "/llms.txt",
        blurb:
          "Concise machine-readable agent index for retrieval and integration.",
      },
      {
        title: "llms-full.txt",
        href: "/llms-full.txt",
        blurb:
          "Long-form machine-readable agent reference with the canonical public surfaces.",
      },
    ],
  },
  {
    label: "Reference",
    links: [
      {
        title: "MCP tools",
        href: "/docs/mcp/tools",
        blurb:
          "Generated reference for MCP tools, resources, prompt templates, and stable anchors.",
      },
      {
        title: "MCP tools markdown",
        href: "/docs/mcp/tools.md",
        blurb:
          "Parallel markdown mirror of the MCP tools reference for agent retrieval.",
      },
      {
        title: "Error reference",
        href: "/docs/errors",
        blurb:
          "Generated reference for stable Minutes core errors and platform-specific variants.",
      },
    ],
  },
  {
    label: "Guides and compare",
    links: [
      {
        title: "Compare hub",
        href: "/compare",
        blurb:
          "Fit-based comparisons between Minutes and other products such as Granola, Otter, and Fireflies.",
      },
      {
        title: "Best meeting tools for Claude Code and Codex",
        href: "/resources/best-meeting-tools-for-claude-code-and-codex",
        blurb:
          "Category-creation guide for users whose real workflow is Claude Code, Codex, MCP, and durable meeting memory.",
      },
    ],
  },
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

export default function DocsIndexPage() {
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
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
        </div>
      </div>

      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Docs
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Minutes docs
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          This is the clean map of the public Minutes docs surface. Start with{" "}
          <a href="/for-agents" className="text-[var(--accent)] hover:underline">
            For agents
          </a>{" "}
          if you are helping someone install or use Minutes. Use the generated references for the
          MCP and error surfaces. Use the compare and resource pages when the real question is fit,
          not setup.
        </p>
      </section>

      <section className="mt-12 space-y-12">
        {docsSections.map((section) => (
          <div key={section.label}>
            <SectionLabel label={section.label} />
            <div className="grid gap-4">
              {section.links.map((link) => (
                <a
                  key={link.href}
                  href={link.href}
                  className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
                >
                  <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                    {section.label}
                  </p>
                  <h2 className="mt-3 text-[18px] font-medium text-[var(--text)]">{link.title}</h2>
                  <p className="mt-2 text-[15px] leading-8 text-[var(--text-secondary)]">
                    {link.blurb}
                  </p>
                </a>
              ))}
            </div>
          </div>
        ))}
      </section>
    </div>
  );
}
