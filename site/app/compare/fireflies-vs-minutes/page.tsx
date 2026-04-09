import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Fireflies",
  description:
    "A fit-based comparison of Minutes and Fireflies for local-first meeting memory, hosted meeting assistants, integrations, and team workflows.",
  alternates: {
    canonical: "/compare/fireflies-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Hosted meeting assistant, team workflows, integrations, and admin",
    minutes: "Local conversation memory, inspectable output, and agent workflows",
  },
  {
    label: "Pricing",
    competitor: "Free, Pro $18/mo, Business $29/mo, Enterprise/custom tiers",
    minutes: "Open source and free to run yourself",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "MCP support",
    competitor: "Yes, including meeting-data MCP and docs MCP surfaces",
    minutes: "Yes, with generated public MCP docs",
  },
  {
    label: "Integrations and workflow automation",
    competitor: "Stronger today",
    minutes: "Not the main wedge",
  },
  {
    label: "Hosted meeting assistant",
    competitor: "Core part of the product",
    minutes: "Not the core product shape",
  },
  {
    label: "CLI workflow",
    competitor: "Not the main product surface",
    minutes: "First-class surface",
  },
  {
    label: "Inspectable files",
    competitor: "Less central to the product",
    minutes: "Structured markdown is a core artifact",
  },
] as const;

const sources = [
  { label: "Fireflies pricing", href: "https://fireflies.ai/pricing" },
  { label: "Fireflies pricing guide", href: "https://guide.fireflies.ai/articles/3734844560-learn-about-the-fireflies-pricing-plans" },
  { label: "Fireflies MCP server", href: "https://fireflies.ai/blog/fireflies-mcp-server" },
  { label: "Fireflies MCP tools overview", href: "https://docs.fireflies.ai/mcp-tools/overview" },
  { label: "Fireflies docs MCP server", href: "https://docs.fireflies.ai/getting-started/docs-mcp-server" },
  { label: "Fireflies apps and integrations", href: "https://fireflies.ai/apps" },
  { label: "Fireflies custom integrations", href: "https://guide.fireflies.ai/articles/1945662079-custom-integrations-supported" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes error reference", href: "https://useminutes.app/docs/errors" },
] as const;

export default function FirefliesVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Fireflies"
      competitorLabel="Fireflies"
      heroSummary="Fireflies and Minutes overlap around meeting capture and AI recall, but they are optimized for different jobs. Fireflies is stronger if you want a hosted meeting assistant with integrations, team workflows, and centralized admin. Minutes is stronger if you want local-first meeting memory, inspectable markdown, and agent workflows that live beyond one hosted product."
      quickVerdictCompetitor="you want a hosted meeting assistant with stronger integrations, team workflows, admin controls, and a SaaS collaboration story."
      quickVerdictMinutes="you want local processing, open output, and a memory layer that Claude, Codex, and other MCP clients can query across files and tools."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "Fireflies is better aligned with teams that want a hosted meeting assistant plus a wider integrations and workflow-automation story.",
        "Fireflies has stronger business-oriented collaboration, analytics, and admin posture if your organization is buying a managed SaaS workflow.",
        "For buyers who want centralized transcripts and meeting workflows without caring about local files or CLI surfaces, Fireflies will usually feel simpler.",
      ]}
      minutesWins={[
        "Minutes is local-first and open-source, so the durable output is inspectable markdown you can keep and use outside the product.",
        "Minutes is stronger for developer and agent-heavy workflows because the product spans MCP, CLI, desktop, SDK, and Claude Code plugin surfaces.",
        "Minutes is better if your core problem is long-lived memory and recall across tools rather than primarily hosted transcript and workflow automation.",
      ]}
      workflowSection={[
        "Fireflies now has real MCP surfaces, including an MCP server for meeting data and a docs MCP server. So the comparison is not 'Fireflies is SaaS and Minutes is MCP.' The more honest distinction is what the system is optimized around. Fireflies is centered on hosted meeting-assistant workflows, integrations, and team operations. Minutes uses MCP as one surface of a broader local-first memory system.",
        "If your team wants a managed meeting assistant that plugs into the rest of your SaaS stack, Fireflies is often the better fit. If you want durable local artifacts and a workflow your own assistants can use across files, desktop, CLI, plugin, and MCP, Minutes is the better fit.",
      ]}
      chooseSection={[
        "Pick Fireflies if your organization wants a hosted meeting assistant with stronger integration depth, admin controls, and centralized team workflows.",
        "Pick Minutes if you want local ownership, inspectable files, and a developer/operator-friendly memory layer rather than a managed workflow hub.",
        "These products overlap, but they are not trying to be identical. The right choice depends on whether you want a hosted assistant for teams or a local memory layer for people and agents.",
      ]}
      notRightFitSection={[
        "Minutes is probably not the right first choice if your top priority is a managed team workflow with broad SaaS integrations, admin controls, and hosted collaboration around meeting data.",
        "It is also not the best fit if you do not care about local-first processing, open artifacts, or multi-surface agent workflows. In those cases, Fireflies may be the better product.",
      ]}
      evaluatedSection={[
        "This page is based on current official product and documentation sources, reviewed on 2026-04-09. It is intentionally a fit-based comparison, not a teardown. Pricing and feature claims can move, so the official sources are linked below.",
        "The Minutes side of the comparison is grounded in the current public agent-facing docs surface and generated MCP reference, not hand-maintained marketing copy.",
      ]}
      sources={sources as any}
    />
  );
}
