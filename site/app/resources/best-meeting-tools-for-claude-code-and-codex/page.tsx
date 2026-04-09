import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Best meeting tools for Claude Code and Codex",
  description:
    "A fit-based guide to the best meeting tools when your real workflow is Claude Code, Codex, MCP, local memory, and durable recall.",
  alternates: {
    canonical: "/resources/best-meeting-tools-for-claude-code-and-codex",
  },
};

const tools = [
  {
    name: "Minutes",
    bestFor: "Local-first agent memory",
    summary:
      "Best when you want local processing, inspectable markdown, and a meeting memory layer that works across MCP, CLI, desktop, SDK, and Claude Code plugin workflows.",
  },
  {
    name: "Granola AI",
    bestFor: "Polished AI notepad",
    summary:
      "Best when you want a polished AI note-taking experience centered on reading, editing, and sharing enhanced notes inside a hosted product.",
  },
  {
    name: "Fireflies",
    bestFor: "Hosted team assistant workflows",
    summary:
      "Best when you want a hosted meeting assistant with integrations, admin controls, and broader team workflow automation.",
  },
  {
    name: "Otter AI",
    bestFor: "Hosted mainstream meeting assistant",
    summary:
      "Best when you want a hosted meeting assistant with centralized transcripts, collaboration, and team-oriented admin and integration surfaces.",
  },
] as const;

const criteria = [
  "How good is the MCP / agent workflow, not just whether MCP exists?",
  "Do you own the output as inspectable files or mostly as data inside one app?",
  "Is the product built for developers/operators, or for general hosted note-taking?",
  "How strong are team collaboration, admin controls, and integrations?",
  "Is the product optimized for local memory and recall, or for hosted assistant workflows?",
] as const;

const shortlist = [
  {
    category: "Best for local-first agent memory",
    winner: "Minutes",
    why: "It is the most purpose-built for durable local memory, inspectable markdown, and agent workflows across MCP, CLI, desktop, SDK, and plugin surfaces.",
  },
  {
    category: "Best for polished AI note-taking",
    winner: "Granola AI",
    why: "It is stronger if your priority is a refined hosted note-taking experience inside one product.",
  },
  {
    category: "Best for hosted team meeting workflows",
    winner: "Fireflies",
    why: "It has the stronger team/integration/admin posture if the job is a managed hosted assistant for an organization.",
  },
  {
    category: "Best for hosted mainstream meeting assistant workflows",
    winner: "Otter AI",
    why: "It is a stronger fit if you want a hosted assistant with centralized transcripts and a broad collaboration story.",
  },
] as const;

const sources = [
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes error reference", href: "https://useminutes.app/docs/errors" },
  { label: "Granola pricing", href: "https://www.granola.ai/pricing/" },
  { label: "Granola MCP", href: "https://help.granola.ai/article/granola-mcp" },
  { label: "Otter pricing", href: "https://otter.ai/pricing" },
  { label: "Otter apps", href: "https://otter.ai/apps" },
  { label: "Fireflies pricing", href: "https://fireflies.ai/pricing" },
  { label: "Fireflies MCP", href: "https://docs.fireflies.ai/mcp-tools/overview" },
  { label: "Fireflies apps", href: "https://fireflies.ai/apps" },
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

export default function BestMeetingToolsPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP docs
          </a>
        </div>
      </div>

      <section className="max-w-[800px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Resource
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Best meeting tools for Claude Code and Codex
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          If your real workflow runs through Claude Code, Codex, MCP, and durable meeting memory,
          the best tool is not automatically the one with the prettiest summary UI. The real split
          is simpler than that: do you want a hosted assistant for teams, a polished AI notepad, or
          a local-first memory layer your agents can actually use across tools?
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-04-09
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Category-creation guide
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick answer
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            If you want the best local-first meeting memory layer for Claude Code, Codex, and other
            MCP workflows, <span className="font-medium text-[var(--text)]">Minutes</span> is the
            strongest fit in this group.
          </p>
          <p>
            If you want a more polished hosted AI notepad,{" "}
            <span className="font-medium text-[var(--text)]">Granola AI</span> is often the better
            fit. If you want a hosted team assistant with deeper SaaS workflow posture,{" "}
            <span className="font-medium text-[var(--text)]">Fireflies</span> or{" "}
            <span className="font-medium text-[var(--text)]">Otter AI</span> may be the better
            fit.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="How to evaluate this category" />
        <ul className="space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          {criteria.map((criterion) => (
            <li key={criterion}>{criterion}</li>
          ))}
        </ul>
      </section>

      <section className="mt-14">
        <SectionLabel label="Shortlist" />
        <div className="grid gap-4">
          {shortlist.map((item) => (
            <div
              key={item.category}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {item.category}
              </p>
              <p className="mt-3 text-[18px] font-medium text-[var(--text)]">{item.winner}</p>
              <p className="mt-2 text-[15px] leading-8 text-[var(--text-secondary)]">{item.why}</p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Included tools" />
        <div className="grid gap-4 md:grid-cols-2">
          {tools.map((tool) => (
            <div
              key={tool.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {tool.bestFor}
              </p>
              <h2 className="mt-3 text-[18px] font-medium text-[var(--text)]">{tool.name}</h2>
              <p className="mt-2 text-[15px] leading-8 text-[var(--text-secondary)]">
                {tool.summary}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="How to choose" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Pick <span className="font-medium text-[var(--text)]">Minutes</span> if the real
            output you want is local memory your assistants can query later, not just a transcript
            or summary inside one hosted app.
          </p>
          <p>
            Pick <span className="font-medium text-[var(--text)]">Granola AI</span> if you want
            the best polished AI notepad experience. Pick{" "}
            <span className="font-medium text-[var(--text)]">Fireflies</span> or{" "}
            <span className="font-medium text-[var(--text)]">Otter AI</span> if you want a hosted
            assistant with stronger team and integration posture.
          </p>
          <p>
            The category gets much clearer once you stop asking “which app summarizes meetings
            best?” and start asking “what workflow am I actually trying to support?”
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="When Minutes is not the right fit" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Minutes is not the best fit if you want a managed hosted meeting assistant for a team,
            with centralized admin, deeper SaaS integrations, and a workflow built around one
            polished product experience.
          </p>
          <p>
            It is strongest when you care about local ownership, open artifacts, agent readability,
            and multi-surface workflows. If that is not the job, one of the hosted products on this
            page may be the better choice.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/for-agents"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            See agent setup
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Compare pages
          </a>
          <a
            href="/docs/mcp/tools"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            MCP docs
          </a>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Sources" />
        <ul className="space-y-2 text-[14px] leading-7 text-[var(--text-secondary)]">
          {sources.map((source) => (
            <li key={source.href}>
              <a href={source.href} className="text-[var(--accent)] hover:underline">
                {source.label}
              </a>
            </li>
          ))}
        </ul>
      </section>

      <PublicFooter />
    </div>
  );
}
