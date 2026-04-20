import { CopyButton } from "@/components/copy-button";
import { APPLE_SILICON_DOWNLOAD_PATH } from "@/lib/downloads";
import { WINDOWS_SETUP_EXE } from "@/lib/release";

const featureGrid = [
  {
    label: "For agents",
    title: "Memory with structure",
    description:
      "26 MCP tools and structured markdown let Claude, Codex, Gemini CLI, and Cowork query what happened instead of guessing.",
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
        "26 MCP tools for Claude Desktop, Cowork, Dispatch, and Claude Code.",
      ],
      [
        "Any LLM",
        "Use Ollama, OpenAI, or skip summarization entirely if markdown is enough.",
      ],
      [
        "Markdown is truth",
        "YAML frontmatter, plain files, and a workflow that works outside Minutes.",
      ],
    ],
  },
] as const;

const comparisons = [
  ["Local transcription", "No", "No", "Yes", "Yes"],
  ["Open source", "No", "No", "Yes", "MIT"],
  ["Free", "$18/mo", "Freemium", "Free", "Free"],
  ["AI agent integration", "No", "No", "No", "26 MCP tools"],
  ["Cross-meeting intelligence", "No", "No", "No", "Yes"],
  ["Dictation mode", "No", "No", "No", "Yes"],
  ["Voice memos", "No", "No", "No", "iPhone pipeline"],
  ["People memory", "No", "No", "No", "Yes"],
  ["Data ownership", "Their servers", "Their servers", "Local", "Local"],
  ["Data format", "Cloud DB", "Cloud DB", "Markdown", "Markdown + YAML"],
  ["Agent-agnostic", "No", "No", "No", "Yes"],
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

export default function Home() {
  return (
    <div className="mx-auto max-w-[840px] px-6 pb-16 sm:px-8">
      <nav className="sticky top-0 z-40 flex items-center justify-between border-b border-[color:var(--border)] bg-[var(--bg)] py-4 backdrop-blur-sm">
        <a
          href="/"
          className="font-mono text-[15px] font-medium text-[var(--text)]"
        >
          minutes
        </a>
        <div className="flex gap-6 text-sm text-[var(--text-secondary)] max-sm:gap-4 max-sm:text-xs">
          <a href="https://github.com/silverstein/minutes" className="hover:text-[var(--accent)]">
            GitHub
          </a>
          <a href="#install" className="hover:text-[var(--accent)]">
            Install
          </a>
          <a href="#pipeline" className="hover:text-[var(--accent)]">
            Pipeline
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
          Open-source. MCP-native.
        </p>
        <p className="mx-auto mb-5 max-w-[720px] font-mono text-[12px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
          v0.13.2 • 26 MCP tools • 45 CLI commands • 620 tests
        </p>
        <h1 className="mx-auto max-w-[720px] font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Every meeting, memo, and voice note,
          <br />
          <span className="italic text-[var(--accent)]">structured and searchable.</span>
        </h1>
        <p className="mx-auto mt-5 max-w-[600px] text-[16px] leading-7 text-[var(--text-secondary)] sm:text-[17px]">
          Cloud meeting tools rent your own conversations back to you. Minutes
          captures meetings and voice memos locally, writes them as structured
          markdown to your disk, and lets every AI you use read the same folder.
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
        </div>

        <p className="mt-5 font-mono text-[12px] text-[var(--text-secondary)]">
          Local, open source, free forever.
        </p>

        <p className="mx-auto mt-4 max-w-[620px] text-[14px] leading-6 text-[var(--text-secondary)]">
          v0.12.0 makes Parakeet multilingual by default. New installs use
          <span className="mx-1 font-mono text-[var(--text)]">tdt-600m</span>
          instead of the old English-only
          <span className="mx-1 font-mono text-[var(--text)]">tdt-ctc-110m</span>,
          with native VAD, watcher batching, and opt-in fp16 and vocabulary boosts.
        </p>

        <div className="mt-12">
          <TranscriptCard />
        </div>

        <p className="mx-auto mt-4 max-w-[560px] text-[14px] leading-6 text-[var(--text-secondary)]">
          Minutes keeps the raw transcript visible. The structure is the
          interface: timestamps, speakers, action items, and decisions stay
          readable even before an assistant touches them.
        </p>

        <div
          id="install"
          className="mt-14 flex flex-wrap justify-center gap-3"
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
          Download, install, done. First launch downloads a speech model. Existing
          Parakeet users should run
          <span className="mx-1 font-mono text-[var(--text)]">minutes setup --parakeet</span>
          to fetch the new multilingual default, or pin the old 110m model in config.
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

      <section id="pipeline" className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="01" label="Pipeline" />
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
          Transcription is local via whisper.cpp or parakeet.cpp. In v0.12.0,
          Parakeet is multilingual by default and uses native VAD to avoid the
          old fixed 45-second slices. Summarization is optional. Claude can do
          it conversationally when you ask, using your existing subscription.
          No API keys are required to get useful output.
        </p>
      </section>

      <section className="border-t border-[color:var(--border)] py-16">
        <SectionLabel n="02" label="Audience" />
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
        <SectionLabel n="03" label="Features" />
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
        <SectionLabel n="04" label="Comparison" />
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
                  Meetily
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
