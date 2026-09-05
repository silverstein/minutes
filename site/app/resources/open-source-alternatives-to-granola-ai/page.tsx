import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { resourceArticleSchema } from "@/lib/schema";
import { RelatedResources } from "@/components/related-resources";

export const metadata: Metadata = {
  title: "Open-source alternatives to Granola AI",
  description:
    "A fit-based guide to open-source alternatives to Granola AI for users who care about local processing, inspectable output, and agent-friendly workflows.",
  alternates: {
    canonical: "/resources/open-source-alternatives-to-granola-ai",
  },
};

const alternatives = [
  {
    name: "Minutes",
    bestFor: "Open-source meeting memory for agents",
    summary:
      "Best if you want local conversation memory, inspectable markdown, MCP, CLI, desktop, and plugin workflows.",
  },
  {
    name: "Anarlog (formerly Hyprnote)",
    bestFor: "Open-source private AI notepad",
    summary:
      "An editable local notepad with SQLite storage, Markdown export, CLI and MCP. Local AI is a provider choice; community code is MIT and enterprise components are commercial.",
  },
  {
    name: "Meetily",
    bestFor: "Open-source local meeting assistant",
    summary:
      "Best if you want a privacy-first open-source meeting assistant centered on local transcription and summaries.",
  },
  {
    name: "Omi",
    bestFor: "Conversation memory across desktop, phone, and wearables",
    summary: "An MIT project with apps and wearable hardware. The desktop quick start uses Omi's cloud backend; evaluate its data path separately from its open-source license.",
  },
  {
    name: "Vexa",
    bestFor: "Self-hosted meeting capture infrastructure",
    summary: "An Apache-2.0 meeting-bot API for Meet, Teams, and Zoom, with transcripts and MCP. Suitable when you want to operate capture services; it is a different setup from a personal desktop notepad.",
  },
] as const;

const sources = [
  { label: "Omi repository and setup", href: "https://github.com/BasedHardware/omi" },
  { label: "Vexa repository and license", href: "https://github.com/Vexa-ai/vexa" },
  { label: "Granola pricing", href: "https://www.granola.ai/pricing/" },
  { label: "Granola MCP", href: "https://help.granola.ai/article/granola-mcp" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Anarlog (formerly Hyprnote) open source", href: "https://github.com/fastrepl/anarlog/blob/main/LICENSING.md" },
  { label: "Anarlog (formerly Hyprnote) GitHub", href: "https://github.com/fastrepl/anarlog" },
  { label: "Anarlog (formerly Hyprnote) docs", href: "https://docs.anarlog.so" },
  { label: "Meetily website", href: "https://meetily.ai/" },
  { label: "Meetily GitHub", href: "https://github.com/Zackriya-Solutions/meetily" },
] as const;


const LAST_REVIEWED = "2026-09-05";

export default function OpenSourceAlternativesToGranolaPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(resourceArticleSchema({ metadata, path: "/resources/open-source-alternatives-to-granola-ai", lastReviewed: LAST_REVIEWED, sources })) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/open-source-alternatives-to-granola-ai.md" className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            for agents
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
          Open-source alternatives to Granola AI
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          If you like the idea of Granola AI but want something open-source, don’t assume the best
          option is the one that looks most similar. In practice, the better alternative is often
          the one that gives you more control, more inspectable output, or a workflow that actually
          fits the way you already use assistants and tools.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: {LAST_REVIEWED}
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Fit-based resource
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick answer
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Consider <span className="font-medium text-[var(--text)]">Minutes</span> when you want meetings stored as Markdown that your existing AI can search. This guide is written by the Minutes maintainer; compare the setup and limitations before choosing.
          </p>
          <p>
            If you want an open-source private AI notepad that feels closer to the “AI notepad”
            concept, <span className="font-medium text-[var(--text)]">Anarlog (formerly Hyprnote)</span>{" "}
            is a closer conceptual match. If you want a privacy-first open-source meeting assistant
            centered on local transcription and summary workflows,{" "}
            <span className="font-medium text-[var(--text)]">Meetily</span> is also worth a look.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Shortlist" />
        <div className="grid gap-4 md:grid-cols-3">
          {alternatives.map((tool) => (
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
            Pick <span className="font-medium text-[var(--text)]">Minutes</span> if the main job is
            local-first memory your agents can query later through MCP, CLI, desktop, and plugin
            workflows.
          </p>
          <p>
            Pick <span className="font-medium text-[var(--text)]">Anarlog (formerly Hyprnote)</span> if you
            want a more classic “private AI notepad” shape but still care about open-source and
            local-first principles.
          </p>
          <p>
            Pick <span className="font-medium text-[var(--text)]">Meetily</span> if you want a
            privacy-first open-source meeting assistant focused on local transcription and summary
            workflows.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="When an open-source alternative is not the right fit" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            If your top priority is the polished hosted collaboration experience, managed sharing,
            and in-app workflow of Granola AI, none of these open-source tools will be a perfect
            drop-in replacement.
          </p>
          <p>
            Open-source alternatives are strongest when you care about control, inspectability,
            local processing, and adapting the workflow to your own stack. If what you really want
            is the polished hosted Granola experience, it is better to say that plainly.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="How we evaluated" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            This page is based on current official product and documentation sources, reviewed on
            2026-04-09. It is intentionally fit-based rather than exhaustive. The point is not to
            list every open-source note tool on the internet; it is to shortlist the most credible
            alternatives for someone who is specifically considering Granola AI.
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
            href="/compare/granola-vs-minutes"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Minutes vs Granola
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

      <RelatedResources slug="open-source-alternatives-to-granola-ai" />

      <PublicFooter />
    </div>
  );
}
