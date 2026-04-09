import type { Metadata } from "next";

export const metadata: Metadata = {
  title: "Compare Minutes",
  description:
    "Fit-based comparisons for Minutes against other meeting-note and meeting-memory products.",
  alternates: {
    canonical: "/compare",
  },
};

const pages = [
  {
    title: "Minutes vs Granola AI",
    href: "/compare/granola-vs-minutes",
    blurb:
      "Best if you want the honest tradeoff between a polished hosted AI notepad and a local-first memory layer for agents.",
  },
  {
    title: "Minutes vs Otter AI",
    href: "/compare/otter-vs-minutes",
    blurb:
      "Best if you are choosing between a hosted meeting assistant for teams and a file-native memory workflow for Claude, Codex, and MCP clients.",
  },
] as const;

export default function CompareHubPage() {
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
            MCP docs
          </a>
        </div>
      </div>

      <section className="max-w-[760px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Compare
        </p>
        <h1 className="mt-4 font-serif text-[42px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[56px]">
          Compare Minutes
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          These comparison pages are fit-based, not teardown posts. The goal is to help the right
          user pick the right product and make the Minutes wedge clear without pretending every tool
          should do the same job.
        </p>
      </section>

      <section className="mt-12 grid gap-4">
        {pages.map((page) => (
          <a
            key={page.href}
            href={page.href}
            className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              Comparison
            </p>
            <h2 className="mt-3 text-[18px] font-medium text-[var(--text)]">{page.title}</h2>
            <p className="mt-2 text-[15px] leading-8 text-[var(--text-secondary)]">{page.blurb}</p>
          </a>
        ))}
      </section>
    </div>
  );
}
