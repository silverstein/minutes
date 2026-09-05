import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { resourceArticleSchema } from "@/lib/schema";
import { RelatedResources } from "@/components/related-resources";

export const metadata: Metadata = {
  title: "Best MCP meeting memory tools",
  description:
    "A fit-based guide to the best MCP meeting memory tools for local-first workflows, hosted team assistants, and agent-readable meeting recall.",
  alternates: {
    canonical: "/resources/best-mcp-meeting-memory-tools",
  },
};

const shortlist = [
  {
    name: "Minutes",
    bestFor: "Local-first MCP meeting memory",
    summary:
      "Best when you want local processing, inspectable markdown, and a meeting memory layer that works across MCP, CLI, desktop, SDK, and Claude Code plugin workflows.",
  },
  {
    name: "Granola AI",
    bestFor: "Polished AI notepad with MCP",
    summary:
      "Best when you want a refined AI note-taking experience and also want some MCP access into a hosted product.",
  },
  {
    name: "Fireflies.ai",
    bestFor: "Hosted team workflow with MCP",
    summary:
      "Best when you want a hosted meeting assistant, broader integrations, and MCP access in a team-centered SaaS workflow.",
  },
  {
    name: "Otter AI",
    bestFor: "Hosted mainstream meeting assistant with MCP",
    summary:
      "Best when you want a hosted meeting assistant with centralized transcripts, collaboration, and broader team/admin posture.",
  },
  {
    name: "Anarlog",
    bestFor: "A local notepad with CLI and MCP",
    summary: "An editable meeting app backed by local SQLite, with Markdown export and a local CLI/MCP surface. Select local or hosted AI providers separately.",
  },
  {
    name: "Vexa",
    bestFor: "Self-hosted capture infrastructure",
    summary: "A meeting-bot API with live transcripts and MCP for people operating their own capture stack. Evaluate the operational setup before choosing it over a desktop app.",
  },
] as const;

const criteria = [
  "This is a maintainer-written fit guide, not a hands-on benchmark. Anarlog and Vexa were checked against their repositories on September 5, 2026; confirm hosted product plans in the linked docs.",
  "Is the MCP support real and current, not just vague integration language?",
  "Does the MCP layer expose durable meeting memory or just a narrow hosted slice?",
  "Can the output be inspected and reused outside one app?",
  "Is the product optimized for local memory workflows or hosted team workflows?",
] as const;

const sources = [
  { label: "Anarlog CLI and MCP", href: "https://github.com/fastrepl/anarlog" },
  { label: "Vexa MCP and capture", href: "https://github.com/Vexa-ai/vexa" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Granola MCP", href: "https://help.granola.ai/article/granola-mcp" },
  { label: "Fireflies MCP server", href: "https://fireflies.ai/blog/fireflies-mcp-server" },
  { label: "Fireflies MCP docs", href: "https://docs.fireflies.ai/mcp-tools/overview" },
  { label: "Otter pricing", href: "https://otter.ai/pricing" },
] as const;


const LAST_REVIEWED = "2026-09-05";

export default function BestMcpMeetingMemoryToolsPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(resourceArticleSchema({ metadata, path: "/resources/best-mcp-meeting-memory-tools", lastReviewed: LAST_REVIEWED, sources })) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/best-mcp-meeting-memory-tools.md" className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
          </a>
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP docs
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
        </div>
      </div>

      <section className="max-w-[800px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Resource
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Best MCP meeting memory tools
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          The useful question is not just “which meeting tool has MCP?” It’s “which tool gives my
          assistants durable meeting memory in a form they can actually use?” That is a smaller,
          more specific category, and the tools in it are not all good at the same thing.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: {LAST_REVIEWED}
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
            Consider <span className="font-medium text-[var(--text)]">Minutes</span> for local meeting records stored as Markdown and searched through your preferred MCP client. Compare Anarlog too if you want an editable local notepad.
          </p>
          <p>
            If you want a more polished hosted AI note-taking product with MCP access,{" "}
            <span className="font-medium text-[var(--text)]">Granola AI</span> is a stronger fit.
            If you want a hosted team assistant with broader integration and admin posture,{" "}
            <span className="font-medium text-[var(--text)]">Fireflies.ai</span> or{" "}
            <span className="font-medium text-[var(--text)]">Otter AI</span> may be the better
            choice.
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
        <div className="grid gap-4 md:grid-cols-2">
          {shortlist.map((tool) => (
            <div
              key={tool.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {tool.bestFor}
              </p>
              <h3 className="mt-3 text-[18px] font-medium text-[var(--text)]">{tool.name}</h3>
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
            Pick <span className="font-medium text-[var(--text)]">Minutes</span> if your assistants
            need local, inspectable meeting memory they can query later across MCP, CLI, desktop,
            and plugin workflows.
          </p>
          <p>
            Pick <span className="font-medium text-[var(--text)]">Granola AI</span> if you want a
            polished hosted AI notepad with MCP access into that product. Pick{" "}
            <span className="font-medium text-[var(--text)]">Fireflies.ai</span> or{" "}
            <span className="font-medium text-[var(--text)]">Otter AI</span> if you want a hosted
            meeting assistant with stronger team and admin posture.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="When Minutes is not the right fit" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Minutes is not the best fit if the real goal is a managed hosted assistant for a team,
            with centralized admin, broader SaaS integrations, and a workflow built around one
            polished product experience.
          </p>
          <p>
            It fits workflows that need local ownership, open artifacts, and agent
            readability. If that is not the job, one of the hosted tools in this category may be a
            better fit.
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
            href="/docs/mcp/tools"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            MCP docs
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Compare pages
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

      <RelatedResources slug="best-mcp-meeting-memory-tools" />

      <PublicFooter />
    </div>
  );
}
