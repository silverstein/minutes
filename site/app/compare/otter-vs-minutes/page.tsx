import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Otter AI",
  description:
    "A fit-based comparison of Minutes and Otter AI for local-first meeting memory, hosted meeting assistants, and team workflows.",
  alternates: {
    canonical: "/compare/otter-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Hosted meeting assistant for teams, auto-join, and collaborative transcripts",
    minutes: "Local conversation memory, inspectable output, and agent workflows",
  },
  {
    label: "Pricing",
    competitor: "Basic free, Pro $16.99/user/mo, Business $30/user/mo, Enterprise custom",
    minutes: "Open source and free to run yourself",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Meeting bot / auto-join",
    competitor: "Core part of the product",
    minutes: "Not the core product shape",
  },
  {
    label: "MCP support",
    competitor: "Yes, on current pricing page",
    minutes: "Yes, with generated public MCP docs",
  },
  {
    label: "API / webhooks",
    competitor: "Available on higher-end plans and beta/public API paths",
    minutes: "Open-source stack with CLI, SDK, and MCP surfaces",
  },
  {
    label: "Team admin and collaboration",
    competitor: "Stronger today",
    minutes: "Not the main wedge",
  },
  {
    label: "Local-first files",
    competitor: "Not the product center of gravity",
    minutes: "Core part of the product",
  },
] as const;

const sources = [
  { label: "Otter pricing", href: "https://otter.ai/pricing" },
  { label: "Otter apps and integrations", href: "https://otter.ai/apps" },
  { label: "Otter API (beta)", href: "https://help.otter.ai/hc/en-us/articles/4412365535895-Does-Otter-offer-an-open-API" },
  { label: "Otter Zapier integration", href: "https://help.otter.ai/hc/en-us/articles/27616131311127-Zapier-Otter-ai-Integration" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes error reference", href: "https://useminutes.app/docs/errors" },
] as const;

export default function OtterVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Otter"
      competitorLabel="Otter AI"
      markdownHref="/compare/otter-vs-minutes.md"
      heroSummary="Otter and Minutes overlap around meeting capture and AI recall, but they are built around different operating assumptions. Otter is stronger if you want a hosted meeting assistant with auto-join, collaboration, and team workflows. Minutes is stronger if you want local-first meeting memory, inspectable markdown, and agent workflows that live beyond one hosted product."
      quickVerdictCompetitor="you want a hosted meeting assistant that joins calls, captures transcripts for teams, and plugs into a broader admin and integrations story."
      quickVerdictMinutes="you want local processing, open output, and a memory layer that Claude, Codex, and other MCP clients can query across files and tools."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "Otter is better aligned with teams that want a hosted meeting assistant, auto-joining behavior, and a more mature collaboration/admin story.",
        "Otter's integrations, API/webhook direction, and enterprise-focused controls are stronger if your organization is buying a managed SaaS workflow.",
        "For buyers who do not want to think about files, local setup, or tool surfaces, Otter will often feel simpler.",
      ]}
      minutesWins={[
        "Minutes is local-first and open-source, so the output is inspectable markdown you can keep and use outside the product.",
        "Minutes is stronger for developer and agent-heavy workflows because the product spans MCP, CLI, desktop, SDK, and Claude Code plugin surfaces.",
        "Minutes is better if your core problem is long-lived memory and recall across tools, not just hosted meeting capture and summaries.",
      ]}
      workflowSection={[
        "Otter now advertises MCP support on its pricing surface, so this is not a 'hosted tool versus MCP tool' comparison. The real distinction is that Otter's MCP support sits inside a hosted meeting-assistant product with plan-specific limits and team-oriented workflows, while Minutes uses MCP as one surface of a broader local-first memory system.",
        "If your team wants a meeting bot that joins calls and centralizes transcripts in a managed SaaS environment, Otter is often the better fit. If you want durable local artifacts and a workflow your own agents can use across files, CLI, desktop, and MCP, Minutes is the better fit.",
      ]}
      chooseSection={[
        "Pick Otter if your organization wants a hosted meeting assistant with auto-join, team collaboration, and admin controls.",
        "Pick Minutes if you want local ownership, inspectable files, and a developer/operator-friendly memory layer rather than a managed meeting bot.",
        "These products overlap, but they are not trying to be identical. The right choice depends on whether you want a hosted assistant or a local memory layer.",
      ]}
      notRightFitSection={[
        "Minutes is probably not the right first choice if your top priority is a managed team workflow with auto-joining meeting bots, centralized SaaS administration, and enterprise collaboration features.",
        "It is also not the best fit if you do not care about local-first processing, open artifacts, or multi-surface agent workflows. In those cases, Otter may be the better product.",
      ]}
      evaluatedSection={[
        "This page is based on current official product and documentation sources, reviewed on 2026-04-09. It is intentionally a fit-based comparison, not a teardown. Pricing and feature claims can move, so the official sources are linked below.",
        "The Minutes side of the comparison is grounded in the current public agent-facing docs surface and generated MCP reference, not hand-maintained marketing copy.",
      ]}
      sources={sources as any}
    />
  );
}
