import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Hyprnote",
  description:
    "A fit-based comparison of Minutes and Hyprnote (Anarlog) for local-first meeting notes, agent workflows, consent provenance, and inspectable markdown output.",
  alternates: {
    canonical: "/compare/hyprnote-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "A local-first AI notepad for people who take notes during meetings",
    minutes: "Local conversation infrastructure for agent workflows and inspectable output",
  },
  {
    label: "Open source",
    competitor: "Yes, MIT",
    minutes: "Yes, MIT",
  },
  {
    label: "Local-first processing",
    competitor: "Core part of the product",
    minutes: "Core part of the product",
  },
  {
    label: "Product shape",
    competitor: "Notepad app: you write, it listens and enhances your notes",
    minutes: "Memory layer: recordings become structured markdown your agents query",
  },
  {
    label: "Agent surface",
    competitor: "Desktop app first",
    minutes: "Files, 31 MCP tools, CLI, SDK, live transcript reads, Claude Code plugin",
  },
  {
    label: "Consent provenance",
    competitor: "Not a stated focus",
    minutes: "Consent basis stamped into every recording's frontmatter",
  },
  {
    label: "Voice memos and dictation",
    competitor: "Meeting-centered",
    minutes: "iPhone voice memo pipeline, dictation hotkey, daily notes",
  },
  {
    label: "Cross-meeting memory",
    competitor: "Notes organized per meeting",
    minutes: "People, decisions, and commitments tracked across the whole corpus",
  },
] as const;

const sources = [
  { label: "Hyprnote / Anarlog repository", href: "https://github.com/fastrepl/anarlog" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
] as const;

export default function HyprnoteVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Hyprnote"
      competitorLabel="Hyprnote (Anarlog)"
      markdownHref="/compare/hyprnote-vs-minutes.md"
      heroSummary="Hyprnote and Minutes are friendly neighbors: both are open source, local-first, and serious about privacy. The practical difference is the job. Hyprnote is a notepad you write in during meetings, with AI that enhances what you wrote. Minutes is a memory layer: it turns everything you record into structured markdown that Claude, Codex, and any MCP client can query later, with consent provenance in every file."
      quickVerdictCompetitor="you want a polished local notepad for taking and enhancing your own meeting notes, and the app itself is where you want to live."
      quickVerdictMinutes="you want a durable, agent-readable corpus: files on your disk, MCP tools, a CLI, and consent and provenance metadata your tools can rely on."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "Hyprnote's in-meeting note-taking experience is the product. If you think while writing, its enhance-my-notes flow is the better fit.",
        "It has a larger community today and a tight focus on the notepad job.",
        "If you only ever need meeting notes (not voice memos, dictation, or an agent surface), it is the simpler tool.",
      ]}
      minutesWins={[
        "Minutes is built for what happens after the meeting: a corpus of markdown with YAML frontmatter that agents query across months of conversations.",
        "The agent surface is broader: MCP server, CLI, SDK, live transcript reads for mid-meeting coaching, and a Claude Code plugin.",
        "Governance lives in the data: consent basis is stamped into every recording, with sensitive no-capture meetings and agent-enforced sensitivity on the roadmap.",
      ]}
      workflowSection={[
        "Both projects process audio locally, so the privacy floor is similar. The fork in the road is the output contract. Hyprnote's durable artifact is your enhanced notes. Minutes' durable artifact is a structured, diarized transcript plus extracted decisions, action items, and people, written as plain files that outlive any one app.",
        "If your assistant should answer 'what did we decide about pricing in April', the question is whether the record it reads was designed for that. Minutes' files, MCP tools, and knowledge graph are built for exactly that query.",
      ]}
      chooseSection={[
        "Pick Hyprnote if the notepad is the product you want: you write, it listens, your notes get better.",
        "Pick Minutes if the corpus is the product you want: everything recorded becomes agent-readable memory with provenance.",
        "Running both is coherent: they solve adjacent jobs, and neither locks your data away.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if you mainly want to write notes during meetings and have AI clean them up. That is Hyprnote's home turf.",
        "It is also more tool than you need if you have no interest in MCP, CLIs, or giving your AI assistants a memory of your conversations.",
      ]}
      evaluatedSection={[
        "This page is based on public repository and product information, reviewed on 2026-06-10. It is a fit-based comparison between two open-source projects, not a teardown; we genuinely like that Hyprnote exists.",
        "The Minutes side is grounded in the public agent-facing docs surface and generated MCP reference, not hand-maintained marketing copy.",
      ]}
      sources={sources as any}
    />
  );
}
