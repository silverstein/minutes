import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Fathom",
  description:
    "Minutes vs Fathom: Fathom offers a generous free tier and now bot-free capture, but every recording is processed and stored in its US cloud. Minutes keeps transcription and storage on your own device. A sourced, fit-based comparison.",
  alternates: {
    canonical: "/compare/fathom-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Free, polished cloud meeting summaries with CRM workflows",
    minutes: "On-device conversation memory and agent workflows over inspectable files",
  },
  {
    label: "Capture method",
    competitor: "Bot, or bot-free via desktop app (beta), per meeting — Zoom, Meet, Teams, Slack Huddles",
    minutes: "Always botless — records device-side, works for in-person conversations too",
  },
  {
    label: "Where audio is processed",
    competitor: "Fathom's cloud; AI via Anthropic, OpenAI, and Google",
    minutes: "On your device (whisper.cpp or parakeet.cpp); audio is never uploaded",
  },
  {
    label: "Where recordings live",
    competitor: "Fathom's US servers, retained indefinitely by default (auto-delete rules: Business+)",
    minutes: "Your own disk, as markdown files you can delete with rm",
  },
  {
    label: "Data residency",
    competitor: "US only",
    minutes: "Moot — data never leaves your machine",
  },
  {
    label: "Model training on your data",
    competitor: "LLM subprocessors contractually barred; Fathom trains its own models on de-identified data (opt-out)",
    minutes: "No vendor exists to train on anything",
  },
  {
    label: "Compliance posture",
    competitor: "SOC 2 Type II; HIPAA with a published blanket BAA (pricing lists HIPAA BAA under Enterprise)",
    minutes: "No vendor in the loop to trust, breach, or subpoena",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "API / MCP",
    competitor: "Yes — public API and a first-party MCP server over its cloud",
    minutes: "Yes — MCP (31 tools), CLI, and SDK over local files",
  },
  {
    label: "Pricing",
    competitor: "Free (unlimited recording); Premium $20, Team $19, Business $34 per user/mo billed monthly ($16/$15/$25 annually); Enterprise custom",
    minutes: "Open source and free to run yourself",
  },
] as const;

const architecture = {
  caption:
    "Fathom's bot-free mode removes the visible participant from your call — real credit for that. It does not change where your conversation goes: capture is uploaded to Fathom's US cloud for transcription, summarization via Anthropic/OpenAI/Google, and indefinite default storage.",
  competitor: {
    name: "Fathom",
    leavesDevice: true,
    steps: [
      {
        label: "Capture",
        detail: "bot in the call, or bot-free via desktop app (beta)",
        offDevice: false,
      },
      {
        label: "Transcribe + summarize",
        detail: "Fathom's cloud; AI via Anthropic / OpenAI / Google",
        offDevice: true,
      },
      {
        label: "Store recordings + transcripts",
        detail: "Fathom's servers, US only, indefinite by default",
        offDevice: true,
      },
    ],
    footnote:
      "SOC 2 Type II, a published blanket HIPAA BAA, and no-training contracts with its LLM subprocessors — a serious cloud posture. But it is a cloud posture: your meetings live on Fathom's US servers, and Fathom improves its own models on de-identified customer data unless you opt out.",
  },
  minutes: {
    name: "Minutes",
    leavesDevice: false,
    steps: [
      {
        label: "Capture",
        detail: "device audio — no bot, works offline and in person",
        offDevice: false,
      },
      {
        label: "Transcribe + diarize",
        detail: "on-device — whisper.cpp / parakeet.cpp + pyannote",
        offDevice: false,
      },
      {
        label: "Store",
        detail: "markdown on your disk, 0600 permissions",
        offDevice: false,
      },
    ],
    footnote:
      "Nothing is uploaded by default — the only network traffic is one-time model downloads, plus transcript text if you explicitly configure an LLM summarizer (local via Ollama, or a provider you choose). There is no retention policy to configure because there is no vendor copy to retain — the only copy is yours.",
  },
} as const;

const sources = [
  { label: "Fathom (product)", href: "https://fathom.ai/" },
  { label: "Fathom pricing", href: "https://fathom.ai/pricing" },
  { label: "Fathom: Is Fathom secure?", href: "https://help.fathom.video/en/articles/296512" },
  { label: "Fathom HIPAA / BAA", href: "https://help.fathom.video/en/articles/5291265" },
  { label: "Fathom blanket BAA", href: "https://www.fathom.ai/baa" },
  { label: "Fathom storage & retention", href: "https://help.fathom.video/en/articles/296448" },
  { label: "Fathom MCP docs", href: "https://developers.fathom.ai/mcp-docs" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes security & privacy architecture", href: "/security" },
] as const;

export default function FathomVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Fathom"
      competitorLabel="Fathom"
      markdownHref="/compare/fathom-vs-minutes.md"
      lastReviewed="2026-07-11"
      heroSummary="Fathom is the strongest free offer in cloud meeting notes — unlimited recording at $0, polished summaries, CRM sync, and now a bot-free capture option (in beta). Minutes draws the line somewhere Fathom doesn't: recording, transcription, and storage all happen on your own machine, and your audio is never uploaded. Fathom processes and stores every meeting in its US cloud, with AI running through Anthropic, OpenAI, and Google; Minutes transcribes on-device and writes markdown to your own disk. Which trade you should make depends on whether 'free and polished' or 'never uploaded' is your constraint."
      quickVerdictCompetitor="you want excellent free meeting summaries with CRM workflows and you're comfortable with your recordings living in a US cloud, processed by major AI providers under no-training contracts."
      quickVerdictMinutes="your conversations must never leave your machine — for compliance, confidentiality, or principle — and you want inspectable markdown your own agents query locally, not another cloud account."
      architecture={architecture as any}
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "The free tier is genuinely exceptional: unlimited recordings, transcription, and storage at $0 — nobody else in the category, including us, matches that offer.",
        "Sales and CRM workflows are real product depth: HubSpot/Salesforce field sync, coaching metrics, AI scorecards, deal views.",
        "Good agent-ecosystem citizenship: a public API and a first-party MCP server, plus bot-free capture (beta) that addresses the most-hated part of cloud notetakers.",
      ]}
      minutesWins={[
        "Your audio never leaves your machine — Fathom's bot-free mode still uploads your meeting to its cloud; Minutes' capture-to-storage pipeline has no upload step at all (see our security page for the complete list of what does touch the network).",
        "No indefinite vendor retention: Fathom keeps recordings by default until deleted, with auto-delete rules gated to Business+. Minutes' only copy is the one on your disk.",
        "Open source (MIT) and free forever — the privacy claims are verifiable Rust, not a trust page; and it captures in-person conversations and voice memos, not just video calls.",
      ]}
      workflowSection={[
        "Both products now speak MCP, so the agent question is again about where the data lives. Fathom's MCP reads its cloud — capable, but your agent is querying Fathom's copy of your meetings. Minutes' MCP reads markdown on your own disk, alongside a CLI, SDK, and Claude Code plugin — your agent queries your copy, offline if need be.",
        "Fathom is built around scheduled video calls. Minutes treats calls as one case of a broader thing — conversations — which is why device-side capture matters: hallway conversations, in-person meetings, and voice memos never had a meeting link for a bot to join.",
      ]}
      chooseSection={[
        "Pick Fathom if the free tier's value is the point and cloud storage is acceptable: it is the best free cloud notetaker, and its compliance posture (SOC 2, published BAA) is serious for a cloud product.",
        "Pick Minutes if the constraint is architectural: no vendor copy, no US-residency question, no retention policy to audit — because no upload exists. That's the only posture a cloud product can't offer at any tier.",
        "Teams sometimes run both: Fathom for sales calls that feed the CRM, Minutes for everything that should never be in a vendor cloud.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if you want maximum polish for zero dollars and zero setup on video calls — Fathom's free tier is unbeatable on that axis.",
        "It's also not the fit if your team lives in HubSpot or Salesforce and the notetaker's job is feeding the CRM: Fathom's sales tooling has no Minutes equivalent.",
      ]}
      evaluatedSection={[
        "This is a fit-based comparison, not a teardown, reviewed on 2026-07-11 against Fathom's official product, pricing, help-center, and developer documentation, linked below. Fathom's data flow — bot or bot-free capture, cloud processing, US-only storage with indefinite default retention, Anthropic/OpenAI/Google as AI subprocessors, and de-identified internal model training with opt-out — is drawn from Fathom's own security documentation.",
        "The Minutes side is grounded in its public docs and open-source repository. Where a claim depends on current pricing or plan gating, the official source is linked; Fathom's HIPAA BAA plan gating is stated ambiguously across its own pages (a published blanket BAA exists, while pricing lists 'HIPAA BAA' under Enterprise), and we've represented both.",
      ]}
      sources={sources as any}
    />
  );
}
