import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Granola AI",
  description:
    "Minutes vs Granola AI: both skip the meeting bot and capture locally, but Granola transcribes and stores in its US cloud while Minutes keeps every step on your device. A sourced, fit-based comparison.",
  alternates: {
    canonical: "/compare/granola-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Polished, collaborative AI notepad and team-friendly meeting notes",
    minutes: "On-device conversation memory and agent workflows over inspectable files",
  },
  {
    label: "Where audio is transcribed",
    competitor: "Cloud providers (Deepgram, AssemblyAI)",
    minutes: "On your device (whisper.cpp or parakeet.cpp)",
  },
  {
    label: "Where transcripts and notes live",
    competitor: "Granola's servers (AWS, US only)",
    minutes: "Your own disk, as markdown files",
  },
  {
    label: "Audio retention",
    competitor: "Streamed to the cloud, then deleted after transcription",
    minutes: "Never uploaded; kept locally only if you choose to",
  },
  {
    label: "EU data residency",
    competitor: "Not available yet",
    minutes: "Moot — data never leaves your machine",
  },
  {
    label: "Compliance posture",
    competitor: "SOC 2 Type 2, GDPR DPA on request, no third-party model training",
    minutes: "No vendor in the loop to trust, breach, or subpoena",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Pricing",
    competitor: "Basic free, Business $14/user/mo, Enterprise $35+/user/mo",
    minutes: "Open source and free to run yourself",
  },
  {
    label: "MCP support",
    competitor: "Yes, over a hosted cloud notes product",
    minutes: "Yes, over local files, a CLI, and generated public docs",
  },
  {
    label: "Team sharing and collaboration",
    competitor: "Stronger today",
    minutes: "Not the main wedge",
  },
] as const;

const architecture = {
  caption:
    "Both apps capture audio locally and neither drops a bot into your call — that's real common ground. The difference is what happens next: where your conversation goes to be transcribed, enhanced, and stored.",
  competitor: {
    name: "Granola",
    leavesDevice: true,
    steps: [
      {
        label: "Capture from your mic",
        detail: "device audio, on your Mac",
        offDevice: false,
      },
      {
        label: "Transcribe",
        detail: "cloud providers — Deepgram / AssemblyAI",
        offDevice: true,
      },
      {
        label: "Enhance notes",
        detail: "cloud AI — OpenAI / Anthropic",
        offDevice: true,
      },
      {
        label: "Store transcripts + notes",
        detail: "Granola servers — AWS, US only",
        offDevice: true,
      },
    ],
    footnote:
      "Audio is streamed to third-party transcription and deleted afterward — genuinely privacy-conscious for a cloud tool (bot-free, SOC 2 Type 2, no model training). But your transcripts and notes live in Granola's US cloud, and there's no EU data residency yet.",
  },
  minutes: {
    name: "Minutes",
    leavesDevice: false,
    steps: [
      {
        label: "Capture from your mic",
        detail: "device audio, on your Mac",
        offDevice: false,
      },
      {
        label: "Transcribe",
        detail: "on-device — whisper.cpp / parakeet.cpp",
        offDevice: false,
      },
      {
        label: "Store transcripts + notes",
        detail: "your disk — markdown in ~/meetings",
        offDevice: false,
      },
    ],
    footnote:
      "Nothing is uploaded. Audio, transcript, and notes never leave your machine — there is no vendor cloud to trust, breach, or subpoena. That's the difference between “we delete your audio” and “we never had it.”",
  },
} as const;

const sources = [
  { label: "Granola security", href: "https://www.granola.ai/security" },
  {
    label: "Granola security, privacy & data FAQs",
    href: "https://docs.granola.ai/help-center/consent-security-privacy/security-privacy-data-faqs",
  },
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
      lastReviewed="2026-07-10"
      heroSummary="Granola and Minutes both skip the meeting bot and capture audio locally — but they draw the privacy line in different places. Granola sends that audio to cloud transcription and AI services, then stores your transcripts and notes on its own US servers; it's a polished, collaborative product built around that hosted cloud. Minutes transcribes on your device and writes markdown to your own disk, so nothing leaves your machine. It's the difference between “we delete your audio” and “we never had it.”"
      quickVerdictCompetitor="you want a polished, collaborative AI notepad and you're comfortable with cloud transcription and hosted storage on US servers — backed by SOC 2, audio auto-deletion, and no third-party model training."
      quickVerdictMinutes="your conversations must never leave your machine — for compliance, client confidentiality, or principle — and you want inspectable files your own agents can read, not a hosted app."
      architecture={architecture as any}
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "The hosted product is more polished and collaborative: sharing, team workspaces, and integrations are more mature than anything file-native Minutes offers today.",
        "Granola's cloud is genuinely privacy-conscious for a cloud tool — bot-free capture, audio deleted after transcription, SOC 2 Type 2, and no third-party model training.",
        "For non-technical teams that live inside one app and want enhanced notes shared widely, Granola will simply feel simpler than files, a CLI, and MCP surfaces.",
      ]}
      minutesWins={[
        "Nothing leaves your machine. Transcription runs on-device and the record is markdown on your own disk — the only architecture that satisfies “no client audio in anyone's cloud,” not just “audio deleted after.”",
        "No US-only data-residency problem, no vendor to breach or subpoena, no DPA to negotiate — because there is no third party in the loop at all.",
        "The durable output is inspectable files any agent can read: Claude, Codex, and other MCP clients query your meetings as local memory across CLI, desktop, SDK, and the Claude Code plugin.",
      ]}
      workflowSection={[
        "Both now have MCP, so this is no longer 'Granola for humans, Minutes for agents.' The architectural distinction is where the data the MCP serves actually lives. Granola's MCP reads a hosted notes product on Granola's servers; Minutes' MCP reads local files you own, alongside a CLI, a desktop app, live transcript reads, a public MCP reference, and a Claude Code plugin.",
        "If the question is 'can my assistant see some meeting notes?', both qualify. If it's 'can my assistant use my meetings as durable local memory that never leaves my control?', only one architecture answers yes.",
      ]}
      chooseSection={[
        "Pick Granola if you want the more polished, collaborative hosted product and you're comfortable with cloud transcription and US-based storage.",
        "Pick Minutes if local ownership is non-negotiable — regulated work, client confidentiality, or simply not wanting your conversations on someone else's servers — and you want files your agents can use.",
        "These are real, different trade-offs, not fake alternatives: one optimizes for hosted polish and collaboration, the other for on-device ownership and agent-native files.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if your priority is a hosted, collaborative note-taking app for a team that wants to live in one polished product and share enhanced notes broadly. Granola is better at that today.",
        "It is also not the fit if you don't care about local processing, inspectable files, or agent workflows, and you'd rather trade on-device control for collaboration and ease. That's a legitimate choice, and Granola may be the better product for it.",
      ]}
      evaluatedSection={[
        "This is a fit-based comparison, not a teardown, reviewed on 2026-07-10 against Granola's official product, security, and help-center documentation, linked below. Granola's data flow — local capture, cloud transcription via Deepgram/AssemblyAI, AI enhancement via OpenAI/Anthropic, and storage on AWS in the US — is drawn from Granola's own security and privacy FAQ.",
        "The Minutes side is grounded in its public agent-facing docs and generated MCP reference. Where a claim depends on current pricing, storage region, or MCP scope, the official source is linked.",
      ]}
      sources={sources as any}
    />
  );
}
