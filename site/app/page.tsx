import { CopyButton } from "@/components/copy-button";
import { DemoPlayer } from "@/components/demo-player";
import { APPLE_SILICON_DOWNLOAD_PATH } from "@/lib/downloads";
import {
  GITHUB_CONTRIBUTORS,
  GITHUB_FORKS,
  GITHUB_STARS,
  NPM_MONTHLY_DOWNLOADS,
} from "@/lib/proof";
import {
  MINUTES_MCP_TOOL_COUNT,
  MINUTES_RELEASE_VERSION,
  WINDOWS_SETUP_EXE,
} from "@/lib/release";

const featureGrid = [
  {
    label: "For agents",
    title: "Local audio context",
    description:
      `${MINUTES_MCP_TOOL_COUNT} MCP tools, live transcript reads, and structured markdown let Claude, Codex, Gemini CLI, and Cowork work from what was actually said.`,
  },
  {
    label: "For developers",
    title: "Local and inspectable",
    description:
      "whisper.cpp or parakeet.cpp transcription, diarized markdown, YAML frontmatter, and a plain-files workflow that still works with grep and git.",
  },
  {
    label: "For meetings",
    title: "Capture what matters",
    description:
      "One-click recording, streaming transcription, speaker separation, decisions, and action items without shipping your audio to a SaaS vendor.",
  },
  {
    label: "For voice memos",
    title: "Phone to desktop",
    description:
      "Minutes watches for iPhone Voice Memos, transcribes them on your Mac, and makes them available to the same memory layer.",
  },
  {
    label: "For daily work",
    title: "Dictation that stays useful",
    description:
      "Hold the hotkey, speak, release. Minutes sends the text to the clipboard and your daily note without changing tools.",
  },
  {
    label: "For recall",
    title: "Answers from raw output",
    description:
      "Competitors hide the transcript. Minutes keeps timestamps, speakers, and action items visible so the source stays readable.",
  },
] as const;

const capabilityColumns = [
  {
    label: "Capture",
    items: [
      [
        "Local transcription",
        "whisper.cpp with GPU acceleration. Your audio stays on your machine.",
      ],
      [
        "Streaming results",
        "Text appears as you speak, with partial updates every few seconds.",
      ],
      [
        "Speaker diarization",
        "pyannote separates who said what in multi-person meetings.",
      ],
      [
        "Dictation mode",
        "Clipboard + daily note flow for short-form thoughts and commands.",
      ],
    ],
  },
  {
    label: "Intelligence",
    items: [
      [
        "Structured extraction",
        "Action items, decisions, and commitments become queryable markdown.",
      ],
      [
        "Relationship memory",
        "Track people, projects, and unresolved commitments across meetings.",
      ],
      [
        "Cross-meeting search",
        "Search everything or ask your assistant to pull the thread for you.",
      ],
      [
        "Voice memo pipeline",
        "iPhone recordings arrive on Mac and join the same memory graph.",
      ],
    ],
  },
  {
    label: "Integration",
    items: [
      [
        "Desktop app",
        "Tauri menu bar app with recording, dictation hotkey, and meeting prompts.",
      ],
      [
        "Claude-native",
        `${MINUTES_MCP_TOOL_COUNT} MCP tools for Claude Desktop, Cowork, Dispatch, and Claude Code.`,
      ],
      [
        "Any LLM",
        "Use Ollama, OpenAI-compatible gateways, local servers, or skip summarization entirely.",
      ],
      [
        "Markdown is truth",
        "YAML frontmatter, plain files, and a workflow that works outside Minutes.",
      ],
    ],
  },
] as const;

// Competitor cells reflect public docs as of June 2026. Refresh quarterly.
const comparisons = [
  ["Local transcription", "No (cloud)", "No (cloud)", "Yes", "Yes"],
  ["Open source", "No", "No", "MIT", "MIT"],
  ["Free", "Freemium", "Freemium", "Free", "Free"],
  ["Agent surface", "Hosted MCP", "Hosted integrations", "Local app", `Files + ${MINUTES_MCP_TOOL_COUNT} MCP tools`],
  ["Cross-meeting intelligence", "Cloud chat", "Cloud chat", "No", "Local graph"],
  ["Consent provenance", "No", "No", "No", "In every file"],
  ["Dictation mode", "No", "No", "No", "Yes"],
  ["Voice memos", "No", "No", "No", "iPhone pipeline"],
  ["People memory", "No", "No", "No", "Yes"],
  ["Data ownership", "Their servers", "Their servers", "Local", "Local"],
  ["Data format", "Cloud DB", "Cloud DB", "Local files", "Markdown + YAML"],
  ["Agent-agnostic", "No", "No", "Partially", "Yes"],
] as const;

function SectionLabel({ n, label }: { n: string; label: string }) {
  return (
    <div className="mb-8 flex items-center gap-3">
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {n}
      </span>
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
        {label}
      </span>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

function TranscriptCard() {
  return (
    <div className="overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] text-left shadow-[var(--shadow-panel)]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border)] px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Transcript Output
          </p>
          <p className="mt-1 font-mono text-[12px] text-[var(--text-secondary)]">
            2026-04-08-strategy-sync.md
          </p>
        </div>
        <div className="flex flex-wrap gap-2">
          <span className="rounded-full bg-[var(--accent-soft)] px-2 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--accent)]">
            2 speakers
          </span>
          <span className="rounded-full bg-[var(--bg-hover)] px-2 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            42 min
          </span>
          <span className="rounded-full bg-[var(--bg-hover)] px-2 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            3 actions
          </span>
        </div>
      </div>

      <div className="space-y-6 px-5 py-5 font-mono text-[12px] leading-6 text-[var(--text)] sm:px-6">
        <div className="transcript-grid">
          <span className="text-[var(--text-tertiary)]">09:02</span>
          <span className="text-[var(--accent)]">mat</span>
          <span>
            We should switch consultants to monthly billing instead of annual.
          </span>

          <span className="text-[var(--text-tertiary)]">09:04</span>
          <span className="text-[var(--accent)]">dana</span>
          <span>
            Test it on the next three signups first and compare retention.
          </span>

          <span className="text-[var(--text-tertiary)]">09:11</span>
          <span className="text-[var(--accent)]">mat</span>
          <span>
            Minutes, capture that as a pricing experiment and link it to Q2
            planning.
          </span>
        </div>

        <div className="border-t border-[color:var(--border)] pt-5">
          <p className="mb-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
            Action Items
          </p>
          <div className="space-y-2 text-[var(--text)]">
            <div>
              <span className="mr-2 text-[var(--accent)]">☐</span>
              Test monthly billing with the next three consultant signups
            </div>
            <div>
              <span className="mr-2 text-[var(--accent)]">☐</span>
              Compare retention and payback against annual billing
            </div>
            <div>
              <span className="mr-2 text-[var(--accent)]">☐</span>
              Review experiment results in next week&apos;s pricing sync
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

const dictationEntries = [
  ["08:14", "Remind me to send the Q3 numbers to the board before Friday."],
  ["11:02", "Onboarding idea: defer the model download until the first recording."],
  ["15:47", "Dana owns the pricing experiment — review the results next week."],
] as const;

function DictationCard() {
  return (
    <div className="overflow-hidden rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] text-left shadow-[var(--shadow-panel)]">
      <div className="flex flex-col gap-3 border-b border-[color:var(--border)] px-5 py-4 sm:flex-row sm:items-center sm:justify-between">
        <div>
          <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
            Dictation
          </p>
          <p className="mt-1 font-mono text-[12px] text-[var(--text-secondary)]">
            daily-note.md · 2026-07-10
          </p>
        </div>
        <div className="flex items-center gap-2 font-mono text-[11px] text-[var(--text-secondary)]">
          <kbd className="rounded-[4px] border border-[color:var(--border-mid)] bg-[var(--bg-hover)] px-2 py-1 text-[var(--text)]">
            ⌥
          </kbd>
          <kbd className="rounded-[4px] border border-[color:var(--border-mid)] bg-[var(--bg-hover)] px-2 py-1 text-[var(--text)]">
            Space
          </kbd>
          <span>hold to talk</span>
        </div>
      </div>

      <div className="space-y-3 px-5 py-5 font-mono text-[12px] leading-6 text-[var(--text)] sm:px-6">
        {dictationEntries.map(([time, text]) => (
          <div key={time} className="flex gap-4">
            <span className="shrink-0 text-[var(--text-tertiary)]">{time}</span>
            <span>{text}</span>
          </div>
        ))}
      </div>

      <div className="border-t border-[color:var(--border)] px-5 py-3 font-mono text-[11px] leading-5 text-[var(--text-tertiary)] sm:px-6">
        Transcribed on-device · appended locally · searchable by your AI later
      </div>
    </div>
  );
}

type HomeFlowStep = { label: string; detail: string; offDevice?: boolean };

const cloudFlow: HomeFlowStep[] = [
  { label: "Capture from your mic", detail: "device audio, on your Mac" },
  { label: "Transcribe", detail: "cloud providers", offDevice: true },
  { label: "Enhance notes", detail: "hosted AI", offDevice: true },
  { label: "Store transcripts + notes", detail: "their servers", offDevice: true },
];

const minutesFlow: HomeFlowStep[] = [
  { label: "Capture from your mic", detail: "device audio, on your Mac" },
  { label: "Transcribe", detail: "on-device — whisper.cpp / parakeet.cpp" },
  { label: "Store transcripts + notes", detail: "your disk — markdown in ~/meetings" },
];

function HomeFlowCard({
  name,
  leavesDevice,
  steps,
  footnote,
}: {
  name: string;
  leavesDevice: boolean;
  steps: HomeFlowStep[];
  footnote: string;
}) {
  return (
    <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 text-left shadow-[var(--shadow-panel)]">
      <div className="flex items-center justify-between gap-3">
        <p className="font-mono text-[13px] font-medium text-[var(--text)]">{name}</p>
        <span
          className={`rounded-full px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.14em] ${
            leavesDevice
              ? "bg-[var(--bg-hover)] text-[var(--text-secondary)]"
              : "bg-[var(--accent-soft)] text-[var(--accent)]"
          }`}
        >
          {leavesDevice ? "Leaves your device" : "Stays on device"}
        </span>
      </div>
      <ol className="mt-5">
        {steps.map((step, i) => (
          <li key={step.label}>
            <div
              className={`rounded-[6px] px-4 py-3 ${
                step.offDevice
                  ? "border border-dashed border-[color:var(--border-mid)] bg-transparent"
                  : "border border-[color:var(--border)] bg-[var(--bg)]"
              }`}
            >
              <div className="flex items-center justify-between gap-3">
                <span className="font-mono text-[13px] text-[var(--text)]">
                  {step.label}
                </span>
                <span
                  className={`shrink-0 font-mono text-[10px] uppercase tracking-[0.12em] ${
                    step.offDevice
                      ? "text-[var(--text-tertiary)]"
                      : "text-[var(--accent)]"
                  }`}
                >
                  {step.offDevice ? "☁ cloud" : "on-device"}
                </span>
              </div>
              <p className="mt-1 font-mono text-[11px] leading-5 text-[var(--text-secondary)]">
                {step.detail}
              </p>
            </div>
            {i < steps.length - 1 ? (
              <div
                className="flex justify-center py-1.5 text-[15px] text-[var(--text-tertiary)]"
                aria-hidden="true"
              >
                ↓
              </div>
            ) : null}
          </li>
        ))}
      </ol>
      <p className="mt-5 border-t border-[color:var(--border)] pt-4 text-[13px] leading-7 text-[var(--text-secondary)]">
        {footnote}
      </p>
    </div>
  );
}

export default function Home() {
  return (
    <div className="mx-auto max-w-[840px] px-6 pb-16 sm:px-8">
      <nav className="sticky top-0 z-40 flex flex-wrap items-center justify-between gap-3 border-b border-[color:var(--border)] bg-[var(--bg)] py-4 backdrop-blur-sm">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex flex-wrap justify-end gap-x-6 gap-y-2 text-sm text-[var(--text-secondary)] max-sm:gap-x-4 max-sm:text-xs">
          <a href="https://github.com/silverstein/minutes" className="hover:text-[var(--accent)]">
            GitHub
          </a>
          <a href="#install" className="hover:text-[var(--accent)]">
            Install
          </a>
          <a href="#dictation" className="hover:text-[var(--accent)]">
            Dictation
          </a>
          <a href="#local" className="hover:text-[var(--accent)]">
            On-device
          </a>
          <a href="#pipeline" className="hover:text-[var(--accent)]">
            Pipeline
          </a>
          <a href="/proof" className="hover:text-[var(--accent)]">
            Proof
          </a>
          <a href="/writing" className="hover:text-[var(--accent)]">
            Writing
          </a>
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            For agents
          </a>
          <a href="/llms.txt" className="hover:text-[var(--accent)]">
            llms.txt
          </a>
        </div>
      </nav>

      <section className="pb-16 pt-16 text-center sm:pb-20 sm:pt-24">
        <p className="mb-5 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Open source · Local first · MIT
        </p>
        <h1 className="mx-auto max-w-[760px] font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Your AI remembers every conversation —
          <br />
          <span className="italic text-[var(--accent)]">and no one can take it from you.</span>
        </h1>
        <p className="mx-auto mt-5 max-w-[640px] text-[16px] leading-7 text-[var(--text-secondary)] sm:text-[17px]">
          Meetings, voice memos, dictation — Minutes transcribes them locally,
          writes structured markdown to your own disk, and lets every AI you use
          (Claude, Codex, Gemini, anything MCP) read the same folder of truth.
          Nothing is uploaded. When a cloud memory app gets acquired or
          subpoenaed, your recordings aren&apos;t theirs to hand over — they
          never left your machine.
        </p>
        <div className="mx-auto mt-7 flex max-w-[640px] flex-wrap justify-center gap-2">
          {[
            "On-device transcription",
            "Nothing uploaded",
            "Open source · MIT",
            "Consent in every file",
          ].map((claim) => (
            <span
              key={claim}
              className="rounded-full border border-[color:var(--border-mid)] px-3 py-1 font-mono text-[11px] tracking-[0.02em] text-[var(--text-secondary)]"
            >
              {claim}
            </span>
          ))}
        </div>
        <p className="mx-auto mt-6 max-w-[720px] font-mono text-[12px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
          {GITHUB_STARS} GitHub stars • {GITHUB_FORKS} forks •{" "}
          {GITHUB_CONTRIBUTORS} contributors • {NPM_MONTHLY_DOWNLOADS} npm
          installs/mo
        </p>

        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <a
            href="#install"
            className="inline-flex items-center gap-2 rounded-[5px] bg-[var(--accent)] px-6 py-2.5 font-mono text-[11px] font-medium uppercase tracking-[0.1em] text-black hover:bg-[var(--accent-hover)]"
          >
            Get started
            <svg
              width="14"
              height="14"
              viewBox="0 0 16 16"
              fill="none"
              className="mt-px"
            >
              <path
                d="M6 3l5 5-5 5"
                stroke="currentColor"
                strokeWidth="1.5"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </a>
          <a
            href="https://github.com/silverstein/minutes"
            className="inline-flex items-center gap-2 rounded-[5px] border border-[color:var(--border-mid)] px-6 py-2.5 font-mono text-[11px] uppercase tracking-[0.1em] text-[var(--text-secondary)] hover:border-[color:var(--accent)] hover:text-[var(--accent)]"
          >
            View on GitHub
          </a>
          <a
            href="/proof"
            className="inline-flex items-center gap-2 rounded-[5px] border border-[color:var(--border-mid)] px-6 py-2.5 font-mono text-[11px] uppercase tracking-[0.1em] text-[var(--text-secondary)] hover:border-[color:var(--accent)] hover:text-[var(--accent)]"
          >
            See proof
          </a>
        </div>

        <p className="mt-5 font-mono text-[12px] text-[var(--text-secondary)]">
          Local, open source, free forever.
        </p>

        <div className="mt-12">
          <DemoPlayer />
        </div>

        <div className="mt-12">
          <TranscriptCard />
        </div>

        <p className="mx-auto mt-4 max-w-[560px] text-[14px] leading-6 text-[var(--text-secondary)]">
          Minutes keeps the raw transcript visible. The structure is the
          interface: timestamps, speakers, action items, and decisions stay
          readable even before an assistant touches them.
        </p>

        <p className="mx-auto mt-12 max-w-[620px] rounded-[5px] border border-[color:var(--border)] bg-[var(--bg-elevated)] px-4 py-3 font-mono text-[12px] leading-5 text-[var(--text-secondary)]">
          <span className="text-[var(--accent)]">v{MINUTES_RELEASE_VERSION}</span>{" "}
          closes the macOS desktop window-lifecycle crash, detects Zoom calls on vanity meeting URLs, and makes app settings persist reliably.{" "}
          <a
            href={`https://github.com/silverstein/minutes/releases/tag/v${MINUTES_RELEASE_VERSION}`}
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
          >
            Release notes
          </a>
          {" · "}
          <a
            href="https://github.com/silverstein/minutes/releases.atom"
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
          >
            Feed
          </a>
        </p>

        <div
          id="install"
          className="mt-12 flex flex-wrap justify-center gap-3"
        >
          <a
            href={APPLE_SILICON_DOWNLOAD_PATH}
            className="inline-flex items-center gap-2 rounded-[5px] border border-[color:var(--border)] bg-[var(--bg-elevated)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.1em] text-[var(--text)] shadow-[var(--shadow-panel)] hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            Mac (Apple Silicon)
          </a>
          <a
            href={WINDOWS_SETUP_EXE}
            className="inline-flex items-center gap-2 rounded-[5px] border border-[color:var(--border)] bg-[var(--bg-elevated)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.1em] text-[var(--text)] shadow-[var(--shadow-panel)] hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <svg
              width="14"
              height="14"
              viewBox="0 0 24 24"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              strokeLinecap="round"
              strokeLinejoin="round"
            >
              <path d="M21 15v4a2 2 0 01-2 2H5a2 2 0 01-2-2v-4" />
              <polyline points="7 10 12 15 17 10" />
              <line x1="12" y1="15" x2="12" y2="3" />
            </svg>
            Windows
          </a>
        </div>

        <p className="mt-4 text-[13px] text-[var(--text-secondary)]">
          Download, install, done. First launch downloads a speech model. Run
          <span className="mx-1 font-mono text-[var(--text)]">minutes setup --parakeet</span>
          for the multilingual Parakeet backend, or
          <span className="mx-1 font-mono text-[var(--text)]">minutes setup --demo</span>
          to try the pipeline on five bundled fixture meetings.
        </p>

        <div className="mt-8 flex flex-wrap justify-center gap-3">
          <CopyButton
            label="Homebrew (desktop)"
            cmd="brew install --cask silverstein/tap/minutes"
          />
          <CopyButton
            label="Homebrew (CLI)"
            cmd="brew tap silverstein/tap && brew install minutes"
          />
          <CopyButton label="MCP server" cmd="npx minutes-mcp" />
        </div>

        <p className="mt-3 text-[12px] text-[var(--text-secondary)]">
          Newer Homebrew distrusts third-party taps by default; if brew warns
          about silverstein/tap, run{" "}
          <span className="font-mono text-[var(--text)]">brew trust silverstein/tap</span>{" "}
          once.
        </p>

        <div className="mt-8 rounded-[5px] border border-[color:var(--border)] bg-[var(--bg-elevated)] px-4 py-3 text-[13px] leading-6 text-[var(--text-secondary)] sm:hidden">
          Reading on your phone? Minutes installs on Mac and Windows.{" "}
          <a
            href="https://github.com/silverstein/minutes"
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2"
          >
            Star the repo
          </a>{" "}
          so it is waiting when you are back at your desk.
        </div>

        <div className="mt-10 border-t border-[color:var(--border)] pt-8">
          <p className="mb-4 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--text-secondary)]">
            Works with any MCP client
          </p>
          <div className="flex flex-wrap items-center justify-center gap-4 text-sm text-[var(--text-secondary)]">
            <span>Claude Code</span>
            <span className="text-[var(--text-tertiary)]">/</span>
            <span>Codex</span>
            <span className="text-[var(--text-tertiary)]">/</span>
            <span>Gemini CLI</span>
            <span className="text-[var(--text-tertiary)]">/</span>
            <span>Claude Desktop</span>
            <span className="text-[var(--text-tertiary)]">/</span>
            <span>Cowork</span>
          </div>
        </div>
      </section>

      <section id="dictation" className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="01" label="Dictation" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          The fastest way in is your voice.
        </h2>
        <p className="mt-5 max-w-[660px] text-[15px] leading-7 text-[var(--text-secondary)]">
          Hold the hotkey, speak, release. The text lands at your cursor and in
          your daily note — no app to open, no tool to switch. It&apos;s the
          habit most people start with, and it runs on the same local engine as
          every meeting and memo.
        </p>
        <p className="mt-4 max-w-[660px] text-[15px] leading-7 text-[var(--text-secondary)]">
          Other dictation tools hand you text and forget it. Minutes keeps every
          word — transcribed on your machine, part of the same owned memory your
          AI can search later. Your voice never touches a server.
        </p>
        <div className="mt-8">
          <DictationCard />
        </div>
      </section>

      <section id="local" className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="02" label="On-device" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          Your conversation never leaves your machine.
        </h2>
        <p className="mt-5 max-w-[660px] text-[15px] leading-7 text-[var(--text-secondary)]">
          Every other tool sends your audio somewhere to be understood — a cloud
          transcriber, a hosted AI, a server that keeps the result. Minutes
          doesn&apos;t. Transcription runs on your Mac and the record is markdown
          on your own disk. There is no server to trust, breach, or subpoena.
        </p>
        <div className="mt-8 grid gap-5 lg:grid-cols-2">
          <HomeFlowCard
            name="Cloud notetakers"
            leavesDevice
            steps={cloudFlow}
            footnote="Even the privacy-conscious ones capture locally, then stream your audio to the cloud to transcribe and store the result on their servers."
          />
          <HomeFlowCard
            name="Minutes"
            leavesDevice={false}
            steps={minutesFlow}
            footnote="Audio, transcript, and notes never leave your machine. You hold the only copy — nothing to upload, sell, or lose in an acquisition."
          />
        </div>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="03" label="Proof" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          See it work before you believe it.
        </h2>
        <div className="mt-8 grid gap-4 md:grid-cols-3">
          <a
            href="/for-agents#try"
            className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              Try it in two minutes
            </p>
            <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
              One command installs five sample meetings and connects them to
              your AI. Ask Claude what was decided, without recording anything.
            </p>
          </a>
          <a
            href="/proof"
            className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              How we test it
            </p>
            <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
              1,148 automated tests and a public eval harness, with the limits
              stated plainly. Read the receipts before you trust it.
            </p>
          </a>
          <a
            href="https://github.com/silverstein/minutes/tree/main/examples"
            className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)] transition hover:border-[color:var(--border-mid)] hover:bg-[var(--bg-hover)]"
          >
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              Build on the files
            </p>
            <p className="mt-3 text-[14px] leading-7 text-[var(--text-secondary)]">
              Working Mem0 and Graphiti examples show how other tools read the
              same markdown. Your data, everyone&apos;s integration.
            </p>
          </a>
        </div>
      </section>

      <section id="pipeline" className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="04" label="Pipeline" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          How it works
        </h2>
        <pre className="mt-6 overflow-x-auto rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 font-mono text-[13px] leading-7 text-[var(--text-secondary)] shadow-[var(--shadow-panel)]">
{`Audio -> Transcribe -> Diarize -> Summarize -> Markdown -> Relationship Graph
       (local)      (local)    (your LLM)  (decisions,   (people, commitments,
      whisper.cpp   pyannote   Claude /     action items) topics, scores)
                                Ollama`}
        </pre>
        <p className="mt-5 max-w-[660px] text-[15px] leading-7 text-[var(--text-secondary)]">
          Transcription is local via whisper.cpp or parakeet.cpp. Parakeet is
          multilingual by default with native VAD. Live transcription falls
          back cleanly through Apple Speech, Parakeet, and Whisper.
          Summarization is optional — Claude can do it conversationally when
          you ask, using your existing subscription. No API keys are required
          to get useful output.
        </p>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="05" label="Audience" />
        <h2 className="max-w-[620px] font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          Capture it anywhere. Find it everywhere.
        </h2>
        <p className="mt-3 font-mono text-[12px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
          From meetings to memos to agents
        </p>
        <div className="mt-8 grid gap-px bg-[var(--border)] sm:grid-cols-2 lg:grid-cols-3">
          {featureGrid.map((item) => (
            <div key={item.title} className="bg-[var(--bg)] px-6 py-6">
              <p className="font-mono text-[10px] uppercase tracking-[0.18em] text-[var(--accent)]">
                {item.label}
              </p>
              <h3 className="mt-3 font-serif text-[20px] leading-6 text-[var(--text)]">
                {item.title}
              </h3>
              <p className="mt-3 text-[14px] leading-6 text-[var(--text-secondary)]">
                {item.description}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="06" label="Features" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          What you get
        </h2>
        <div className="mt-10 grid gap-10 lg:grid-cols-3">
          {capabilityColumns.map((column) => (
            <div key={column.label}>
              <p className="mb-5 font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
                {column.label}
              </p>
              <div className="space-y-4">
                {column.items.map(([title, description]) => (
                  <div key={title} className="flex gap-3 text-sm">
                    <span className="mt-0.5 font-mono text-[12px] text-[var(--accent)]">
                      &gt;
                    </span>
                    <p className="leading-6 text-[var(--text-secondary)]">
                      <strong className="font-medium text-[var(--text)]">
                        {title}.
                      </strong>{" "}
                      {description}
                    </p>
                  </div>
                ))}
              </div>
            </div>
          ))}
        </div>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="07" label="Comparison" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          How it compares
        </h2>
        <div className="mt-8 overflow-x-auto rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] shadow-[var(--shadow-panel)]">
          <table className="w-full min-w-[620px] border-collapse text-[13px]">
            <thead>
              <tr className="bg-[var(--bg-hover)]">
                <th className="p-3 text-left font-mono text-[10px] uppercase tracking-[0.16em] text-[var(--text-secondary)]" />
                <th className="p-3 text-left font-mono text-[10px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Granola
                </th>
                <th className="p-3 text-left font-mono text-[10px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Otter.ai
                </th>
                <th className="p-3 text-left font-mono text-[10px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Hyprnote
                </th>
                <th className="p-3 text-left font-mono text-[10px] uppercase tracking-[0.16em] text-[var(--accent)]">
                  minutes
                </th>
              </tr>
            </thead>
            <tbody>
              {comparisons.map(([feature, ...values]) => (
                <tr key={feature} className="hover:bg-[var(--bg-hover)]">
                  <td className="border-b border-[color:var(--border)] p-3 font-medium text-[var(--text)]">
                    {feature}
                  </td>
                  {values.map((value, index) => {
                    const isMinutes = index === 3;
                    const isNo = value === "No";
                    return (
                      <td
                        key={`${feature}-${index}-${value}`}
                        className={`border-b border-[color:var(--border)] p-3 ${
                          isMinutes
                            ? "font-semibold text-[var(--text)]"
                            : isNo
                              ? "text-[var(--text-tertiary)]"
                              : "text-[var(--text-secondary)]"
                        }`}
                      >
                        {isNo ? "—" : value}
                      </td>
                    );
                  })}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="08" label="Governance" />
        <h2 className="font-serif text-[30px] leading-tight tracking-[-0.035em] text-[var(--text)] sm:text-[32px]">
          Built in, not retrofitted.
        </h2>
        <p className="mt-5 max-w-[660px] text-[15px] leading-7 text-[var(--text-secondary)]">
          If you take notes on client conversations for a living — legal,
          clinical, financial — a cloud recorder isn&apos;t a preference, it&apos;s
          a compliance problem. Minutes keeps both the audio and the record on
          your own machine, and puts governance in the record itself: every file
          states the consent it was captured under, because its primary reader is
          now an agent.
        </p>
        <div className="mt-8 grid gap-4 md:grid-cols-3">
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              Shipped
            </p>
            <p className="mt-3 text-[14px] leading-6 text-[var(--text-secondary)]">
              Every recording stamps its consent basis into the file&apos;s
              frontmatter. Sensitive meetings capture no audio but keep
              structured notes, and Require mode blocks every desktop and CLI
              entry point until consent is confirmed.
            </p>
          </div>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              Next
            </p>
            <p className="mt-3 text-[14px] leading-6 text-[var(--text-secondary)]">
              Retention rules the corpus enforces on its own audio, and
              enforcement of the sensitivity contract across every agent
              surface, not just the debrief path.
            </p>
          </div>
          <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-5 shadow-[var(--shadow-panel)]">
            <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
              The point
            </p>
            <p className="mt-3 text-[14px] leading-6 text-[var(--text-secondary)]">
              Sensitivity metadata your agents are required to respect: a
              restricted meeting never appears in search, graph queries, or
              anything an agent assembles.
            </p>
          </div>
        </div>
        <p className="mt-6 text-[13px] leading-6 text-[var(--text-secondary)]">
          A disclosure aid, not legal advice; make sure everyone present has
          agreed where required. The design is public:{" "}
          <a
            href="/writing/governance-built-in-not-retrofitted"
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
          >
            why we built it this way
          </a>
          {" and the "}
          <a
            href="https://github.com/silverstein/minutes/blob/main/docs/plans/consent-layer-phase2-2026-06-10.md"
            className="text-[var(--text)] underline decoration-[color:var(--border-mid)] underline-offset-2 hover:text-[var(--accent)]"
          >
            phase 2 plan
          </a>
          .
        </p>
      </section>

      <footer className="border-t border-[color:var(--border)] py-14 text-center text-[13px] text-[var(--text-secondary)]">
        <p>minutes is MIT licensed and free forever.</p>
        <p className="mt-1">
          Built by{" "}
          <a
            href="https://github.com/silverstein"
            className="text-[var(--text)] hover:text-[var(--accent)]"
          >
            Mat Silverstein
          </a>
          , founder of{" "}
          <a
            href="https://x1wealth.com"
            className="text-[var(--text)] hover:text-[var(--accent)]"
          >
            X1 Wealth
          </a>
        </p>
        <p className="mt-3">
          <a href="/for-agents" className="hover:text-[var(--accent)]">
            For agents
          </a>
          {" · "}
          <a href="/docs/mcp/tools" className="hover:text-[var(--accent)]">
            MCP docs
          </a>
          {" · "}
          <a href="/docs/errors" className="hover:text-[var(--accent)]">
            Errors
          </a>
        </p>
        <p className="mt-1">
          <a href="/compare" className="hover:text-[var(--accent)]">
            Compare
          </a>
          {" · "}
          <a
            href="https://github.com/silverstein/minutes"
            className="hover:text-[var(--accent)]"
          >
            GitHub
          </a>
          {" · "}
          <a href="/llms.txt" className="hover:text-[var(--accent)]">
            llms.txt
          </a>
          {" · "}
          <a
            href="https://github.com/silverstein/minutes/blob/main/CONTRIBUTING.md"
            className="hover:text-[var(--accent)]"
          >
            Contribute
          </a>
        </p>
      </footer>
    </div>
  );
}
