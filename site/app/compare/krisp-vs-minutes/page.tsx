import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs Krisp",
  description:
    "Minutes vs Krisp: Krisp's noise cancellation is genuinely on-device, but its meeting notes run through the cloud and transcripts are stored in Krisp Cloud once notes are enabled. Minutes keeps the entire pipeline on your machine. A sourced, fit-based comparison.",
  alternates: {
    canonical: "/compare/krisp-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Making calls sound better (noise cancellation) with meeting notes added on",
    minutes: "On-device conversation memory: private transcripts, structured notes, agent access",
  },
  {
    label: "Capture method",
    competitor: "Botless by default (device-level audio); optional bot mode",
    minutes: "Always botless — records device-side, in-person conversations included",
  },
  {
    label: "Noise cancellation",
    competitor: "Best in category, processed on-device — audio never leaves for this path",
    minutes: "Optional RNNoise-based denoising (local, behind a feature flag) — not the headline feature",
  },
  {
    label: "Where transcription runs",
    competitor: "On-device for English; Krisp's servers for 15 other languages",
    minutes: "On-device always, ~99 languages via Whisper (or Parakeet for European languages)",
  },
  {
    label: "Where AI notes are made",
    competitor: "Cloud — summaries generated via Microsoft Azure services",
    minutes: "Locally structured; LLM summarization only if you explicitly configure one",
  },
  {
    label: "Where transcripts live",
    competitor: "Krisp Cloud (US servers) once meeting notes are enabled — the only non-Enterprise option; on-device storage is Enterprise-gated",
    minutes: "Your own disk, as markdown — for everyone, not a plan tier",
  },
  {
    label: "Compliance posture",
    competitor: "SOC 2 Type II, HIPAA BAA available per its security page (which references a legacy 'Business tier'; pricing lists BAA under Enterprise), published DPA",
    minutes: "No vendor in the loop to trust, breach, or subpoena",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Platforms",
    competitor: "macOS and Windows (no Linux)",
    minutes: "macOS menu bar app + CLI (open source, builds from source elsewhere)",
  },
  {
    label: "Pricing",
    competitor: "Free plan per its help center (2 AI notes/day; the pricing page currently shows a 7-day trial); Core $16/mo ($8 annual); Advanced $30/mo ($15 annual); Enterprise custom",
    minutes: "Open source and free to run yourself",
  },
] as const;

const architecture = {
  caption:
    "Krisp earned its reputation honestly: noise cancellation runs on your device and that audio path never touches its servers. The notes product is a different pipeline — summaries are generated in the cloud, and once you enable meeting notes, transcripts are stored in Krisp Cloud (per Krisp's security page, an explicit opt-in — but the only storage option outside Enterprise).",
  competitor: {
    name: "Krisp",
    leavesDevice: true,
    steps: [
      {
        label: "Capture + denoise",
        detail: "on-device, any app — genuinely local",
        offDevice: false,
      },
      {
        label: "Transcribe",
        detail: "on-device for English; Krisp servers for 15 other languages",
        offDevice: false,
      },
      {
        label: "Generate AI notes",
        detail: "cloud — Microsoft Azure services",
        offDevice: true,
      },
      {
        label: "Store transcripts + recordings",
        detail: "Krisp Cloud, US servers, once notes are enabled (on-device option: Enterprise)",
        offDevice: true,
      },
    ],
    footnote:
      "A hybrid: the audio-quality pipeline is on-device, the memory pipeline is cloud-first. SOC 2 Type II, HIPAA BAAs on business tiers, and a published DPA back the cloud half — but the private-by-architecture configuration is an Enterprise feature, not the default.",
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
        detail: "on-device — sealed local whisper.cpp + pyannote",
        offDevice: false,
      },
      {
        label: "Store",
        detail: "markdown on your disk, 0600 permissions",
        offDevice: false,
      },
    ],
    footnote:
      "The entire pipeline — capture, transcription, diarization, storage — is on-device for every user, free tier included, because there is no other tier. Privacy isn't a plan feature; it's the architecture.",
  },
} as const;

const sources = [
  { label: "Krisp (product)", href: "https://krisp.ai/" },
  { label: "Krisp AI Meeting Assistant", href: "https://krisp.ai/ai-meeting-assistant/" },
  { label: "Krisp pricing", href: "https://krisp.ai/pricing/" },
  {
    label: "Krisp security for AI Meeting Assistant",
    href: "https://krisp.ai/security-for-ai-meeting-assistant/",
  },
  { label: "Krisp security", href: "https://krisp.ai/security/" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes security & privacy architecture", href: "https://useminutes.app/security" },
] as const;

export default function KrispVsMinutesPage() {
  return (
    <ComparePage
      competitorName="Krisp"
      competitorLabel="Krisp"
      markdownHref="/compare/krisp-vs-minutes.md"
      lastReviewed="2026-07-11"
      heroSummary="Krisp is the rare cloud-era company with real on-device credentials: its noise cancellation processes audio locally and never sends it anywhere, which is why it became the default answer to 'my calls sound bad.' But Krisp's meeting-notes product is a different pipeline — summaries are generated through Microsoft Azure, transcripts and recordings are stored in Krisp Cloud once you enable notes, and the fully on-device storage configuration is gated to Enterprise. Minutes runs the entire pipeline on your machine for everyone. If you need better-sounding calls, buy Krisp. If you need a private record of your conversations, the architectures diverge exactly where it matters."
      quickVerdictCompetitor="your primary problem is call audio quality — noise, echo, accents — and AI notes are a convenient add-on you're comfortable having processed and stored in Krisp's cloud."
      quickVerdictMinutes="your primary problem is owning a private record of your conversations — transcripts that never leave your machine, structured notes, and agent access — and you want that as the default, not an Enterprise upgrade."
      architecture={architecture as any}
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "Noise cancellation is the best in the category, genuinely on-device, and works as a virtual microphone across every app — if calls sounding professional is the job, nothing else here competes.",
        "Accent conversion and real-time voice AI are differentiated capabilities no notetaker (including Minutes) offers.",
        "Windows support today, plus an enterprise trust stack: SOC 2 Type II, HIPAA BAAs, PCI-DSS, published DPA.",
      ]}
      minutesWins={[
        "The private configuration is the only configuration: on-device transcription and local storage for every user, free — where Krisp gates on-device transcript/recording storage to Enterprise (and its on-device transcription itself covers English only).",
        "It's a real memory layer: diarized speakers, action items and decisions in YAML, months of meetings greppable and queryable by your agents via MCP, CLI, SDK, and the Claude Code plugin.",
        "Open source (MIT): the claim that audio has no network path is verifiable in the Rust source, not a security-page promise.",
      ]}
      workflowSection={[
        "The two tools barely overlap in daily use. Krisp sits between your microphone and your meeting app, improving audio in real time; its notes are a byproduct of calls. Minutes sits underneath your conversations — calls, in-person meetings, voice memos, dictation — and its entire output is the record: markdown files that accumulate into a searchable corpus your assistant can reason over.",
        "They also compose without conflict: Krisp can clean your microphone signal while Minutes captures and transcribes the conversation locally. People who care about both audio quality and data ownership sometimes run exactly that stack.",
      ]}
      chooseSection={[
        "Pick Krisp if the pain is acoustic: noisy environments, echo, accent friction on calls. That's its core competency and it is genuinely excellent at it.",
        "Pick Minutes if the pain is memory and privacy: you want every conversation transcribed, structured, and owned — with nothing in any vendor's cloud, on any plan.",
        "If you're evaluating Krisp specifically for its AI notes, apply the architecture test: ask where the transcript is stored once notes are on, and what plan tier makes that storage private. Then compare that answer to a tool where the private answer is the only answer.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if your actual problem is call audio quality — Minutes doesn't make you sound better on calls; Krisp does, brilliantly.",
        "It's also not the fit if you need Windows today or want accent conversion and real-time voice features; those are Krisp capabilities without a Minutes equivalent.",
      ]}
      evaluatedSection={[
        "This is a fit-based comparison, not a teardown, reviewed on 2026-07-11 against Krisp's official product, pricing, and security documentation, linked below. Krisp's hybrid architecture — on-device noise cancellation and English transcription, server-side transcription for other languages, Azure-generated summaries, Krisp Cloud default storage, and Enterprise-gated on-device storage — is drawn from Krisp's own security and product pages.",
        "The Minutes side is grounded in its public docs and open-source repository. Where a claim depends on current pricing or plan gating, the official source is linked.",
      ]}
      sources={sources as any}
    />
  );
}
