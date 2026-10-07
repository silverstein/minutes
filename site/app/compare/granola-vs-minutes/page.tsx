import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  "title": "Minutes vs Granola AI",
  "description": "Granola combines bot-free capture with a hosted AI notepad and shared notes. Minutes writes conversation records to local Markdown files that your existing assistant can search. The choice depends on where you want the record to live and how you want to use it.",
  "alternates": {
    "canonical": "/compare/granola-vs-minutes"
  }
};

const comparison = {
  "competitorName": "Granola",
  "competitorLabel": "Granola AI",
  "markdownHref": "/compare/granola-vs-minutes.md",
  "lastReviewed": "2026-10-07",
  "heroSummary": "Granola combines bot-free capture with a hosted AI notepad and shared notes. Minutes writes conversation records to local Markdown files that your existing assistant can search. The choice depends on where you want the record to live and how you want to use it.",
  "quickVerdictCompetitor": "you want a hosted notepad with shared notes and workspace features.",
  "quickVerdictMinutes": "you want inspectable local records and a file-based archive your preferred assistant can search.",
  "comparisonRows": [
    {
      "label": "Workflow",
      "competitor": "Hosted AI notepad and team workspaces",
      "minutes": "Meetings, memos, and dictation in a local conversation archive"
    },
    {
      "label": "Transcription",
      "competitor": "Cloud transcription providers",
      "minutes": "On-device engines available for your platform and build"
    },
    {
      "label": "Record storage",
      "competitor": "Notes and transcripts hosted on AWS in the US",
      "minutes": "Markdown and YAML on your disk"
    },
    {
      "label": "AI enhancement",
      "competitor": "Cloud AI providers",
      "minutes": "Optional local or cloud AI, configured separately"
    },
    {
      "label": "Agent access",
      "competitor": "Hosted MCP; scope depends on plan and workspace permissions",
      "minutes": "MCP, CLI, SDK, and portable skills over authorized local records"
    },
    {
      "label": "Pricing",
      "competitor": "Basic free; Business $14 and Enterprise $35 per user/month",
      "minutes": "Free MIT software; optional provider usage is separate"
    }
  ],
  "competitorWins": [
    "Granola combines note editing and sharing in a hosted workspace.",
    "Its MCP integration supports assistants without requiring a local meeting archive."
  ],
  "minutesWins": [
    "Minutes writes the primary conversation record as Markdown with YAML metadata.",
    "Its open-source desktop, CLI, SDK, and MCP surfaces work over the same authorized corpus. The desktop app supports macOS and Windows; Linux has a CLI."
  ],
  "workflowSection": [
    "Both products have MCP support. Compare which records the connection can read and which permissions apply. Granola serves hosted notes; Minutes serves your authorized local corpus.",
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
      "label": "Granola security",
      "href": "https://www.granola.ai/security"
    },
    {
      "label": "Granola pricing",
      "href": "https://www.granola.ai/pricing"
    },
    {
      "label": "Granola MCP permissions",
      "href": "https://docs.granola.ai/help-center/sharing/integrations/mcp"
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

export default function GranolaVsMinutesPage() {
  return <ComparePage {...comparison} />;
}
