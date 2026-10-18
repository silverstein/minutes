import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  "title": "Minutes vs Fathom",
  "description": "Fathom offers hosted meeting recordings, summaries, and team integrations. Minutes keeps a local conversation archive for your existing assistant. Both expose MCP; the useful distinction is the workflow and storage behind that connection.",
  "alternates": {
    "canonical": "/compare/fathom-vs-minutes"
  }
};

const comparison = {
  "competitorName": "Fathom",
  "competitorLabel": "Fathom",
  "markdownHref": "/compare/fathom-vs-minutes.md",
  "lastReviewed": "2026-10-07",
  "heroSummary": "Fathom offers hosted meeting recordings, summaries, and team integrations. Minutes keeps a local conversation archive for your existing assistant. Both expose MCP; the useful distinction is the workflow and storage behind that connection.",
  "quickVerdictCompetitor": "you want hosted meeting notes, shared recordings, or CRM workflows.",
  "quickVerdictMinutes": "you want inspectable local records and a file-based archive your preferred assistant can search.",
  "comparisonRows": [
    {
      "label": "Workflow",
      "competitor": "Hosted meeting assistant with recordings and team integrations",
      "minutes": "Local conversation records across meetings, memos, and dictation"
    },
    {
      "label": "Capture",
      "competitor": "Meeting bot or bot-free Mac capture in beta",
      "minutes": "Device capture without a meeting bot; support depends on platform"
    },
    {
      "label": "Record storage",
      "competitor": "Hosted recordings and notes; retained until deleted",
      "minutes": "Markdown and YAML on your disk"
    },
    {
      "label": "AI processing",
      "competitor": "Hosted summaries and assistance",
      "minutes": "Local transcription; optional local or cloud AI"
    },
    {
      "label": "Agent access",
      "competitor": "Public API and first-party MCP server",
      "minutes": "MCP, CLI, SDK, and portable skills over authorized local records"
    },
    {
      "label": "Pricing",
      "competitor": "Free recording and transcription; paid individual and team plans",
      "minutes": "Free MIT software; optional provider usage is separate"
    }
  ],
  "competitorWins": [
    "Fathom offers unlimited recording and transcription on its free tier.",
    "Its paid plans add team and CRM workflows around shared meeting records."
  ],
  "minutesWins": [
    "Minutes writes the primary conversation record as Markdown with YAML metadata.",
    "Its open-source desktop, CLI, SDK, and MCP surfaces work over the same authorized corpus. The desktop app supports macOS and Windows; Linux has a CLI."
  ],
  "workflowSection": [
    "Fathom can provide hosted meeting context through its API and MCP. Minutes exposes local files through its own tools. Test the retrieval and permission model you will actually use rather than treating MCP availability as a quality benchmark.",
    "Minutes captures, transcribes, and stores conversation records locally. Engine availability depends on your platform, build, and installed models. If you choose a cloud summarizer or connect a cloud assistant, authorized meeting context can reach that provider. File sync and backups you configure are separate data boundaries."
  ],
  "chooseSection": [
    "Choose the workflow you will use every day. Try one conversation, inspect the saved record, and ask your assistant to retrieve a specific decision with its source.",
    "Check provider settings, retention, sync, and assistant permissions separately. Local transcription alone does not establish where every later step runs."
  ],
  "notRightFitSection": [
    "Minutes may require more setup if you want a managed team workspace, centralized administration, or ready-made CRM integrations.",
    "A local archive does not establish regulatory compliance. Assess your configuration and obligations before recording sensitive conversations."
  ],
  "evaluatedSection": [
    "Official product, pricing, security, and developer documentation was reviewed on October 7, 2026. This is a maintainer-written comparison, not a hands-on accuracy or reliability benchmark.",
    "The Minutes side reflects its published documentation and source. Optional cloud AI and user-configured sync are separate from local capture and transcription."
  ],
  "sources": [
    {
      "label": "Fathom plans and capture",
      "href": "https://www.fathom.ai/pricing"
    },
    {
      "label": "Fathom MCP",
      "href": "https://developers.fathom.ai/mcp-docs"
    },
    {
      "label": "Fathom recording storage",
      "href": "https://help.fathom.video/en/articles/296448"
    },
    {
      "label": "Minutes security and data flow",
      "href": "https://useminutes.app/security"
    },
    {
      "label": "Minutes proof and limitations",
      "href": "https://useminutes.app/proof"
    }
  ]
};

export default function FathomVsMinutesPage() {
  return <ComparePage {...comparison} />;
}
