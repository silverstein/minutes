import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs superwhisper",
  description:
    "Both transcribe on your device. superwhisper is a polished dictation tool; Minutes is an open-source memory layer for meetings, memos, and agents.",
  alternates: {
    canonical: "/compare/superwhisper-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Polished voice-to-text dictation into any app, on Mac, Windows, iOS, and Android",
    minutes: "On-device conversation memory: meetings, voice memos, and dictation your agents can query",
  },
  {
    label: "Core job",
    competitor: "Speak, get clean formatted text where you're typing",
    minutes: "Capture conversations, transcribe and diarize them, keep a searchable markdown record",
  },
  {
    label: "Where transcription runs",
    competitor: "On-device by default; optional cloud models (recommended on Intel Macs)",
    minutes: "On-device transcription; optional AI providers are configured separately",
  },
  {
    label: "AI formatting / summarization",
    competitor: "Predefined and custom modes, using local or cloud AI models",
    minutes: "Optional and explicit: Claude via MCP or a local LLM you configure; nothing calls a cloud unless you set it up",
  },
  {
    label: "Durable output",
    competitor: "Text inserted into your active app, plus searchable transcription history",
    minutes: "Markdown files with YAML frontmatter, action items, and decisions, on your own disk",
  },
  {
    label: "Meetings and speakers",
    competitor: "Meeting recording and file transcription",
    minutes: "Diarized speakers, confidence-aware attribution, action items, and a meeting lifecycle",
  },
  {
    label: "Agent / MCP surface",
    competitor: "CLI for transcription history and piping transcripts into coding agents",
    minutes: "MCP server (34 tools), CLI, SDK, and a Claude Code plugin over your local files",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Platforms",
    competitor: "macOS, Windows, iOS, Android",
    minutes: "macOS and Windows desktop apps; Linux CLI",
  },
  {
    label: "Pricing",
    competitor: "Free tier; Pro subscription (yearly discount), lifetime and enterprise options",
    minutes: "Open source and free to run yourself",
  },
] as const;

const sources = [
  { label: "superwhisper website and pricing", href: "https://superwhisper.com" },
  { label: "superwhisper transcription-history CLI", href: "https://superwhisper.com/cli" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Minutes MCP reference", href: "https://useminutes.app/docs/mcp/tools" },
  { label: "Minutes on GitHub", href: "https://github.com/silverstein/minutes" },
] as const;

export default function SuperwhisperVsMinutesPage() {
  return (
    <ComparePage
      competitorName="superwhisper"
      competitorLabel="superwhisper"
      markdownHref="/compare/superwhisper-vs-minutes.md"
      lastReviewed="2026-07-11"
      heroSummary="superwhisper and Minutes agree on the thing this category usually gets wrong: your voice should be transcribed on your device, not in someone's cloud. The difference is the job. superwhisper is a polished dictation tool — speak, and clean text lands in whatever app you're typing in. Minutes treats dictation as one input to a bigger system: an open-source conversation memory that records meetings, diarizes speakers, and writes markdown files your AI agents can query. Different jobs, with real overlap."
      quickVerdictCompetitor="you want the most refined dedicated dictation experience — custom per-app modes, 100+ languages, iOS and Windows support — and you're happy paying a subscription for a closed-source tool."
      quickVerdictMinutes="dictation is one mode of a bigger need — recording meetings, keeping voice memos, and building a private, searchable memory of your conversations that Claude and other agents can use — and you want it open source and free."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "The dictation experience itself is more polished: predefined and custom modes format your speech differently per app (email vs Slack vs prose), and that focus shows.",
        "Platform reach is wider today: macOS, Windows, iOS, and Android. Minutes has desktop apps for macOS and Windows, plus a Linux CLI.",
        "Its main workflow is dictation into other apps, with transcription history available through its CLI.",
      ]}
      minutesWins={[
        "Meetings and memos become diarized, searchable Markdown with action items and decisions. The files stay on your disk and remain available independently of the app.",
        "It's open source (MIT) and free. You can read the capture, transcription, and storage code instead of trusting a privacy page.",
        "Your agents can use it: Claude, Codex, and any MCP client query your conversation history through 34 MCP tools, a CLI, an SDK, and a Claude Code plugin.",
      ]}
      workflowSection={[
        "Both tools support dictation and access to previous transcripts. superwhisper offers app-specific formatting and a CLI for its history. Minutes keeps meeting and memo records as Markdown, with speaker labels, action items, decisions, and policy-aware retrieval through its CLI and MCP tools.",
        "Try each tool against a few conversations you will need again. Check whether its stored output, source links, and access controls fit the way you want an assistant to use that history.",
      ]}
      chooseSection={[
        "Pick superwhisper for dedicated voice input across Mac, Windows, iOS, and Android, with formatting modes and transcript-history access.",
        "Pick Minutes if you want one local pipeline for meetings, voice memos, and dictation, with a durable markdown record your agents can query — and you'd rather run open source than subscribe to closed source.",
        "Using both can make sense if you prefer superwhisper for text input and Minutes for meeting capture, inspectable files, and agent access.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if you want best-in-class dictation UX on iOS or Windows today, or if per-app text formatting modes are the feature you'd actually use daily. superwhisper is better at that.",
        "It's also not the fit if you find markdown files, a CLI, and agent workflows to be complexity you don't want. A single-purpose dictation app is legitimately simpler.",
      ]}
      evaluatedSection={[
        "Release spot check, October 7, 2026: reviewed the linked official product pages for current workflow, provider, platform, and pricing claims. This is not a new hands-on benchmark. Minutes captures, transcribes, and stores conversation records locally. Engine availability depends on your platform, build, and installed models. If you choose a cloud summarizer or connect a cloud assistant, authorized meeting context can reach that provider. File sync and backups you configure are separate data boundaries.",
        "The October 7 source check also covers superwhisper's transcription-history CLI and current mobile platforms. The CLI was not installed or tested here; its documented capabilities are linked below.",
        "The Minutes side is grounded in its public agent-facing docs, generated MCP reference, and open-source repository. Where a claim depends on current pricing or feature scope, the official source is linked.",
      ]}
      sources={sources as any}
    />
  );
}
