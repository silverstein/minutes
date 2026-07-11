import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs MacWhisper",
  description:
    "Minutes vs MacWhisper: both transcribe locally on your Mac. MacWhisper is the best file-transcription GUI; Minutes is an open-source conversation memory layer for meetings, memos, and agents. A sourced, fit-based comparison.",
  alternates: {
    canonical: "/compare/macwhisper-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Transcribing audio/video files on a Mac with a polished GUI",
    minutes: "On-device conversation memory: meetings, memos, and dictation your agents can query",
  },
  {
    label: "Core job",
    competitor: "File in, transcript out — batch jobs, subtitles, podcasts, YouTube URLs",
    minutes: "Capture conversations, diarize them, keep a searchable structured record",
  },
  {
    label: "Where transcription runs",
    competitor: "On-device (Whisper, Parakeet, and other local models)",
    minutes: "On-device (whisper.cpp or parakeet.cpp)",
  },
  {
    label: "Optional cloud AI",
    competitor: "BYO API keys for summaries/chat (OpenAI, Anthropic, others) — or fully local via Ollama/LM Studio",
    minutes: "Optional and explicit: Claude via MCP or a local LLM you configure",
  },
  {
    label: "Durable output",
    competitor: "Exports: txt, srt, vtt, md, pdf, docx — per file",
    minutes: "Markdown archive with YAML frontmatter, action items, decisions — per conversation, organized over time",
  },
  {
    label: "Speaker handling",
    competitor: "Automatic speaker recognition (Pro)",
    minutes: "Diarization plus confidence-aware attribution that learns real names",
  },
  {
    label: "Agent / MCP surface",
    competitor: "CLI control and workflow automations; no MCP server we could find",
    minutes: "MCP server (31 tools), CLI, SDK, Claude Code plugin over your local files",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Platforms",
    competitor: "macOS (14+) and iOS; no Windows or Linux",
    minutes: "macOS menu bar app + CLI (open source, builds from source elsewhere)",
  },
  {
    label: "Pricing",
    competitor: "Free tier; Pro €64 one-time direct (App Store channel is subscription-based)",
    minutes: "Open source and free to run yourself",
  },
] as const;

const sources = [
  { label: "MacWhisper (official site)", href: "https://www.macwhisper.com/" },
  {
    label: "Whisper Transcription on the App Store",
    href: "https://apps.apple.com/us/app/whisper-transcription/id1668083311",
  },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes on GitHub", href: "https://github.com/silverstein/minutes" },
  {
    label: "whisper.cpp vs parakeet.cpp — our engine comparison",
    href: "/writing/whisper-cpp-vs-parakeet-cpp",
  },
] as const;

export default function MacwhisperVsMinutesPage() {
  return (
    <ComparePage
      competitorName="MacWhisper"
      competitorLabel="MacWhisper"
      markdownHref="/compare/macwhisper-vs-minutes.md"
      lastReviewed="2026-07-11"
      heroSummary="MacWhisper and Minutes are on the same side of the line that matters most: transcription runs locally on your Mac, and both can run Whisper or Parakeet models. The difference is the shape of the job. MacWhisper is the best drag-and-drop file transcriber on macOS — files in, transcripts and subtitles out. Minutes is a conversation memory layer — meetings and memos in, a growing structured archive out, one your AI agents can query. Respect where it's due; this is a comparison between two local-first tools, and many people legitimately want the other one."
      quickVerdictCompetitor="your job is transcribing files — interviews, podcasts, videos, YouTube links — and you want the most polished Mac GUI for it, with subtitle export and a one-time price."
      quickVerdictMinutes="your job is remembering conversations — recording meetings and memos into a private, diarized, searchable archive that Claude and other agents can use — and you want it open source and free."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "File-transcription ergonomics are unmatched: drag-and-drop batches, YouTube and podcast URLs, per-speaker files, filler-word removal, and a real subtitle workflow (.srt/.vtt with inline video preview and auto-translation).",
        "One-time pricing (€64 direct, lifetime updates) is a genuinely fair deal, and the free tier already covers basic recording and file transcription in 100 languages.",
        "An iOS companion app exists; Minutes is desktop-first today.",
      ]}
      minutesWins={[
        "It builds an archive, not just outputs: every meeting, memo, and dictation becomes structured markdown — attendees, action items, decisions in YAML — organized and greppable over months, not a folder of one-off exports.",
        "It's agent-native: 31 MCP tools, a CLI, an SDK, and a Claude Code plugin let your assistant search meetings, track commitments, and build person profiles from your local files. MacWhisper automates workflows; it doesn't give agents a memory.",
        "It's open source (MIT) and free — the entire pipeline is auditable Rust, which matters if 'local' is a compliance requirement rather than a preference.",
      ]}
      workflowSection={[
        "The overlap is real: both record meetings, both transcribe locally, both can use Whisper or Parakeet engines. The divergence is what happens after transcription. MacWhisper's output is a document you export and move somewhere; its center of gravity is the file. Minutes' output is an entry in a corpus — ~/meetings accumulates, search spans months, and MCP tools answer questions like 'what did we decide about pricing in April' across everything.",
        "A fair test: open your transcription tool's output folder. If it's a pile of exports you rarely revisit, either tool works and MacWhisper is more polished. If you wish that pile were a queryable memory, that wish is the entire reason Minutes exists.",
      ]}
      chooseSection={[
        "Pick MacWhisper for file work: interviews, podcast episodes, subtitle jobs, transcribing someone else's videos. It is the best tool on macOS for exactly that, and the one-time price is honest.",
        "Pick Minutes for conversation memory: your own meetings and ideas, captured continuously, structured automatically, and readable by your agents without anything leaving the machine.",
        "Plenty of people should own both — they're neighbors, not rivals: one optimizes the transcript, the other optimizes the archive.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if your work is transcribing files you receive — it's built around capturing live conversations, not batch-processing media libraries or producing subtitles.",
        "It's also not the fit if you want an iOS-first experience or a polished GUI for one-off transcription jobs; MacWhisper is simply better at those today.",
      ]}
      evaluatedSection={[
        "This is a fit-based comparison between two local-first tools, reviewed on 2026-07-11 against MacWhisper's official site and App Store listing, linked below. MacWhisper's local-by-default transcription, Whisper/Parakeet engine support, Pro feature list, and €64 one-time direct pricing (with a separate subscription-based App Store channel) are drawn from its own pages.",
        "The Minutes side is grounded in its public docs and open-source repository. Both tools' privacy claims are architecture-level and, in Minutes' case, verifiable in source.",
      ]}
      sources={sources as any}
    />
  );
}
