import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  "title": "Minutes vs Krisp",
  "description": "Krisp combines noise cancellation with meeting notes. Minutes focuses on keeping conversations as local files your assistant can search. Krisp also has on-device processing, so this comparison is about the features and data settings you choose, rather than a blanket local-versus-cloud distinction.",
  "alternates": {
    "canonical": "/compare/krisp-vs-minutes"
  }
};

const comparison = {
  "competitorName": "Krisp",
  "competitorLabel": "Krisp",
  "markdownHref": "/compare/krisp-vs-minutes.md",
  "lastReviewed": "2026-10-07",
  "heroSummary": "Krisp combines noise cancellation with meeting notes. Minutes focuses on keeping conversations as local files your assistant can search. Krisp also has on-device processing, so this comparison is about the features and data settings you choose, rather than a blanket local-versus-cloud distinction.",
  "quickVerdictCompetitor": "noise cancellation and a meeting-assistant workflow are your main priorities.",
  "quickVerdictMinutes": "you want inspectable local records and a file-based archive your preferred assistant can search.",
  "comparisonRows": [
    {
      "label": "Workflow",
      "competitor": "Noise cancellation and AI meeting notes",
      "minutes": "Local conversation archive and agent recall"
    },
    {
      "label": "Transcription",
      "competitor": "On-device speech-to-text described in its security documentation",
      "minutes": "On-device engines available for your platform and build"
    },
    {
      "label": "Summaries",
      "competitor": "Cloud AI through Microsoft Azure",
      "minutes": "Optional local or cloud AI, configured separately"
    },
    {
      "label": "Record storage",
      "competitor": "Cloud storage with Meeting Notes enabled and consent; Enterprise offers private on-device options",
      "minutes": "Markdown and YAML on your disk"
    },
    {
      "label": "Noise cancellation",
      "competitor": "Core product capability",
      "minutes": "Does not replace a system-wide noise-cancellation tool"
    },
    {
      "label": "Pricing",
      "competitor": "Core $16 and Advanced $30 per user/month when billed monthly; annual discounts",
      "minutes": "Free MIT software; optional provider usage is separate"
    }
  ],
  "competitorWins": [
    "Krisp provides noise cancellation alongside meeting notes.",
    "Its Enterprise plans include private transcription and recording options. Check the plan and settings rather than assuming every mode has the same data flow."
  ],
  "minutesWins": [
    "Minutes writes the primary conversation record as Markdown with YAML metadata.",
    "Its open-source desktop, CLI, SDK, and MCP surfaces work over the same authorized corpus. The desktop app supports macOS and Windows; Linux has a CLI."
  ],
  "workflowSection": [
    "You can use a noise-cancellation tool alongside Minutes. Minutes stores the resulting conversation locally; a cloud assistant or summarizer remains a separate choice. Compare each enabled processing stage, including storage, before using either product for sensitive work.",
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
      "label": "Krisp meeting-assistant security",
      "href": "https://krisp.ai/security-for-ai-meeting-assistant/"
    },
    {
      "label": "Krisp pricing and private options",
      "href": "https://krisp.ai/pricing/"
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

export default function KrispVsMinutesPage() {
  return <ComparePage {...comparison} />;
}
