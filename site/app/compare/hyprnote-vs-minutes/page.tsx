import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Anarlog (formerly Hyprnote)",
  description: "Compare two open-source meeting apps: Anarlog's editable local notepad and Minutes' Markdown conversation memory for your existing AI.",
  alternates: { canonical: "/compare/hyprnote-vs-minutes" },
};

export default function AnarlogVsMinutesPage() {
  return <ComparePage
    competitorName="Anarlog" competitorLabel="Anarlog (formerly Hyprnote)"
    markdownHref="/compare/hyprnote-vs-minutes.md" lastReviewed="2026-09-05"
    heroSummary={"Anarlog, formerly Hyprnote, and Minutes both support local meeting workflows and external AI tools. Anarlog centers on an editable notepad backed by SQLite. Minutes centers on conversation records stored as Markdown with source and consent metadata. Choose by the workflow you will actually use."}
    quickVerdictCompetitor={"you want to write and edit meeting notes in a local app, with local or hosted AI and optional sync."}
    quickVerdictMinutes={"you want meetings, voice memos, and dictation in a file-based corpus that your preferred assistant can search."}
    competitorWins={["Anarlog makes the editable meeting notepad its main interface.", "It offers Markdown export, local AI choices, and opt-in sync and sharing."]}
    minutesWins={["Minutes writes the primary conversation record as inspectable Markdown and YAML.", "Its capture, voice memo, and dictation workflows feed the same authorized meeting corpus."]}
    workflowSection={["Both projects now expose a CLI and MCP. Having MCP alone is not a reason to prefer Minutes. Evaluate setup, retrieval quality, and what your assistant is allowed to read.", "In Minutes, try the sample pricing reversal through your own assistant, request source files, then repeat with a real conversation. A sample success does not establish recording reliability."]}
    chooseSection={["Choose Anarlog for the editable notepad workflow. Choose Minutes for a Markdown-first conversation archive.", "With either product, verify transcription and AI provider settings separately. Optional cloud AI can receive meeting context."]}
    notRightFitSection={["Minutes may add unnecessary setup if you only want polished notes inside one app.", "Check platform-specific capture support before committing to either tool."]}
    evaluatedSection={["Reviewed September 5, 2026 using the official repositories and documentation. This is a maintainer-written comparison, not a hands-on benchmark.", "Anarlog is the maintained open-source project formerly called Hyprnote. Char is a separate current product from its team."]}
    comparisonRows={[{"label": "Best fit", "competitor": "An editable meeting notepad", "minutes": "A conversation corpus for your existing AI"}, {"label": "License", "competitor": "MIT community app; commercial enterprise components", "minutes": "MIT"}, {"label": "Storage", "competitor": "Local SQLite, recordings and attachments; Markdown export", "minutes": "Markdown with YAML frontmatter as the primary record"}, {"label": "AI processing", "competitor": "Local or hosted transcription and intelligence providers", "minutes": "Local transcription; optional local or cloud AI"}, {"label": "Agent access", "competitor": "Local CLI, MCP server, and agent skills", "minutes": "MCP, CLI, SDK, and portable agent skills"}]}
    sources={[{"label": "Anarlog repository and licensing", "href": "https://github.com/fastrepl/anarlog"}, {"label": "Anarlog documentation", "href": "https://docs.anarlog.so"}, {"label": "Minutes agent workflow", "href": "https://useminutes.app/for-agents"}, {"label": "Minutes proof and limitations", "href": "https://useminutes.app/proof"}]}
  />;
}
