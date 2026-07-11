import type { Metadata } from "next";
import { ComparePage } from "@/components/compare-page";

export const metadata: Metadata = {
  title: "Minutes vs superwhisper",
  description:
    "Minutes vs superwhisper: both transcribe on your device, but superwhisper is a polished dictation tool while Minutes is an open-source conversation memory layer for meetings, memos, and agents. A sourced, fit-based comparison.",
  alternates: {
    canonical: "/compare/superwhisper-vs-minutes",
  },
};

const comparisonRows = [
  {
    label: "Best for",
    competitor: "Polished voice-to-text dictation into any app, on Mac, Windows, and iOS",
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
    minutes: "On-device always (whisper.cpp or parakeet.cpp) — there is no cloud path",
  },
  {
    label: "AI formatting / summarization",
    competitor: "Predefined and custom modes, using local or cloud AI models",
    minutes: "Optional and explicit: Claude via MCP or a local LLM you configure; nothing calls a cloud unless you set it up",
  },
  {
    label: "Durable output",
    competitor: "Text inserted into the app you're using",
    minutes: "Markdown files with YAML frontmatter, action items, and decisions, on your own disk",
  },
  {
    label: "Meetings and speakers",
    competitor: "Meeting recording and file transcription",
    minutes: "Diarized speakers, confidence-aware attribution, action items, and a meeting lifecycle",
  },
  {
    label: "Agent / MCP surface",
    competitor: "None that we could find — built for humans typing with their voice",
    minutes: "MCP server (31 tools), CLI, SDK, and a Claude Code plugin over your local files",
  },
  {
    label: "Open source",
    competitor: "No",
    minutes: "Yes, MIT",
  },
  {
    label: "Platforms",
    competitor: "macOS, Windows, iOS",
    minutes: "macOS menu bar app + CLI (open source, builds from source elsewhere)",
  },
  {
    label: "Pricing",
    competitor: "Free tier; Pro subscription (yearly discount), lifetime and enterprise options",
    minutes: "Open source and free to run yourself",
  },
] as const;

const sources = [
  { label: "superwhisper website and pricing", href: "https://superwhisper.com" },
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
      heroSummary="superwhisper and Minutes agree on the thing this category usually gets wrong: your voice should be transcribed on your device, not in someone's cloud. The difference is the job. superwhisper is a polished dictation tool — speak, and clean text lands in whatever app you're typing in. Minutes treats dictation as one input to a bigger system: an open-source conversation memory that records meetings, diarizes speakers, and writes markdown files your AI agents can query. Different jobs, with honest overlap."
      quickVerdictCompetitor="you want the most refined dedicated dictation experience — custom per-app modes, 100+ languages, iOS and Windows support — and you're happy paying a subscription for a closed-source tool."
      quickVerdictMinutes="dictation is one mode of a bigger need — recording meetings, keeping voice memos, and building a private, searchable memory of your conversations that Claude and other agents can use — and you want it open source and free."
      comparisonRows={comparisonRows as any}
      competitorWins={[
        "The dictation experience itself is more polished: predefined and custom modes format your speech differently per app (email vs Slack vs prose), and that focus shows.",
        "Platform reach is wider today — macOS, Windows, and iOS — where Minutes is macOS-first.",
        "If you never record meetings and never want a durable transcript archive, a dedicated dictation tool is the simpler purchase.",
      ]}
      minutesWins={[
        "It's a memory layer, not just an input method: meetings and memos become diarized, searchable markdown with action items and decisions — a record you own, not text that vanishes into whatever app you pasted it into.",
        "It's open source (MIT) and free. You can read the capture, transcription, and storage code instead of trusting a privacy page.",
        "Your agents can use it: Claude, Codex, and any MCP client query your conversation history through 31 MCP tools, a CLI, an SDK, and a Claude Code plugin.",
      ]}
      workflowSection={[
        "The overlap is real: Minutes has a dictation mode too — speak, and the text lands in your clipboard and a daily note. But the two tools point in different directions from there. superwhisper optimizes the moment of typing: its modes reshape your words for the app you're in, and the output's job is to be pasted. Minutes optimizes what happens after the conversation: every meeting, memo, and dictation becomes a timestamped markdown file that search, the CLI, and MCP tools can reach.",
        "A useful test: a month from now, will you want to ask an assistant 'what did I say about this?' If no, you want a dictation tool. If yes, you want a memory layer — dictation included.",
      ]}
      chooseSection={[
        "Pick superwhisper if your entire need is voice-to-text into other apps and you want the most polished version of that, across Mac, Windows, and iPhone.",
        "Pick Minutes if you want one local pipeline for meetings, voice memos, and dictation, with a durable markdown record your agents can query — and you'd rather run open source than subscribe to closed source.",
        "Running both is coherent, but most people discover the memory layer makes the standalone dictation tool redundant — or vice versa, if they never record conversations at all.",
      ]}
      notRightFitSection={[
        "Minutes is not the right first choice if you want best-in-class dictation UX on iOS or Windows today, or if per-app text formatting modes are the feature you'd actually use daily. superwhisper is better at that.",
        "It's also not the fit if you find markdown files, a CLI, and agent workflows to be complexity you don't want. A single-purpose dictation app is legitimately simpler.",
      ]}
      evaluatedSection={[
        "This is a fit-based comparison, not a teardown, reviewed on 2026-07-11 against superwhisper's public website and pricing, linked below. superwhisper's local-by-default transcription with optional cloud models, its mode system, platform list, and pricing tiers are drawn from its own site.",
        "The Minutes side is grounded in its public agent-facing docs, generated MCP reference, and open-source repository. Where a claim depends on current pricing or feature scope, the official source is linked.",
      ]}
      sources={sources as any}
    />
  );
}
