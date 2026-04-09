import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Granola AI",
  description:
    "A fit-based comparison of Minutes and Granola AI for local-first meeting memory, MCP workflows, collaboration, and team note-taking.",
  alternates: {
    canonical: "/compare/granola-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Polished AI notepad, sharing, and team-friendly meeting notes",
    minutes: "Local conversation memory, agent workflows, and inspectable output",
  },
  {
    label: "Pricing",
    competitor: "Basic free, Business $14/user/mo, Enterprise $35+/user/mo",
    minutes: "Open source and free to run yourself",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Local-first processing",
    competitor: "Not its core product story",
    minutes: "Core part of the product",
  },
  {
    label: "MCP support",
    competitor: "Yes, with plan and data-scope caveats",
    minutes: "Yes, with generated public MCP docs",
  },
  {
    label: "CLI workflow",
    competitor: "Not the main product shape",
    minutes: "First-class surface",
  },
  {
    label: "Team sharing and collaboration",
    competitor: "Stronger today",
    minutes: "Not the main wedge",
  },
  {
    label: "Inspectable files",
    competitor: "Less central to the product",
    minutes: "Structured markdown is a core artifact",
  },
] as const;

const sources = [
  { label: "Granola pricing", href: "https://www.granola.ai/pricing/" },
  { label: "Granola integrations", href: "https://docs.granola.ai/article/integrations-with-granola" },
  { label: "Granola MCP", href: "https://help.granola.ai/article/granola-mcp" },
  { label: "Granola AI-enhanced notes", href: "https://docs.granola.ai/help-center/taking-notes/ai-enhanced-notes" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes error reference", href: "https://useminutes.app/docs/errors" },
] as const;

export default function GranolaVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Granola"
      competitorLabel="Granola AI"
      markdownHref="/compare/granola-vs-minutes.md"
      heroSummary="Granola and Minutes are both good, but they solve different problems. Granola is a better fit if you want a polished AI note-taking product with stronger collaboration and integration ergonomics. Minutes is a better fit if you want local conversation memory, inspectable markdown, and a workflow your agents can use across MCP, CLI, desktop, and plugin surfaces."
      quickVerdictCompetitor="your top priority is a polished AI notepad, collaboration, and a product built around reading, editing, and sharing notes inside the app."
      quickVerdictMinutes="your top priority is local processing, inspectable files, and a memory layer that Claude, Codex, and other MCP clients can query later."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "The standalone note-taking experience looks more polished and better optimized for users who mainly want to live inside one app.",
        "Granola's collaboration and integration story is more mature if your job is sharing notes across a team or pushing them into the rest of your stack.",
        "For many non-technical users, Granola will feel simpler because the product is centered on hosted note workflows rather than files, CLIs, and multiple surfaces.",
      ]}
      minutesWins={[
        "Minutes is local-first and file-native. The durable output is structured markdown you can inspect, sync, grep, and use outside the app.",
        "Minutes is stronger if your real goal is giving Claude, Codex, or another MCP client durable memory across meetings instead of just a better note view.",
        "Minutes has a broader operator/developer surface: MCP server, desktop app, CLI, SDK, and Claude Code plugin rather than one primary app experience.",
      ]}
      workflowSection={[
        "Granola now has official MCP support, so the comparison is no longer 'Granola for humans, Minutes for MCP.' The more honest distinction is what the MCP layer is serving. Granola's MCP offering is attached to a hosted AI notes product and has plan and data-scope caveats. Minutes is built around a broader operator and developer workflow: local processing, inspectable markdown, a public MCP reference, a CLI, a desktop app, and a Claude Code plugin.",
        "If your question is 'can my assistant access some meeting notes?', both can be relevant. If your question is 'can my assistant use my meetings as durable local memory across tools and workflows?', that is where Minutes is more purpose-built.",
      ]}
      chooseSection={[
        "Pick Granola if your team wants the better all-in-one note-taking product, collaboration story, and hosted UX.",
        "Pick Minutes if you care more about local ownership, file-native output, and agent workflows that extend beyond one note-taking app.",
        "The important thing is that these are not fake alternatives. They overlap, but they are optimized for different jobs.",
      ]}
      notRightFitSection={[
        "Minutes is probably not the right first choice if your highest priority is a hosted, collaborative note-taking product for teams that want to stay inside one polished app and share enhanced meeting notes broadly.",
        "It is also not the best fit if you do not care about local files, inspectable output, MCP workflows, or developer/operator control. In that case, Granola may simply be the better product for the job.",
      ]}
      evaluatedSection={[
        "This page is based on current official product and documentation sources, reviewed on 2026-04-09. It is intentionally a fit-based comparison, not a teardown. Where a claim depends on current pricing or current MCP scope, the official source is linked below.",
        "The Minutes side of the comparison is grounded in the current public agent-facing docs surface and generated MCP reference, not hand-maintained marketing copy.",
      ]}
      sources={sources as any}
    />
  );
}
