import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "The best local speech-to-text apps (2026)",
  description:
    "A fit-based guide to on-device transcription: MacWhisper, superwhisper, Buzz, Vibe, whisper.cpp, and Minutes — matched to the job you're actually hiring for, with honest disclosure of which one is ours.",
  alternates: {
    canonical: "/resources/best-local-speech-to-text",
  },
};

const tools = [
  {
    name: "Minutes",
    bestFor: "Meetings, voice memos, and conversation memory for AI agents",
    detail:
      "Open source (MIT), free. Records and transcribes on-device (whisper.cpp or parakeet.cpp), diarizes speakers, and writes markdown with action items that Claude and other MCP clients can query. macOS menu bar app + CLI.",
    license: "Open source, free",
  },
  {
    name: "MacWhisper",
    bestFor: "Polished Mac GUI for transcribing audio and video files",
    detail:
      "The most refined drag-and-drop Whisper experience on macOS: batch file transcription, system-audio capture, subtitle export. Free with smaller models; Pro is a one-time purchase. Closed source.",
    license: "Freemium, one-time Pro",
  },
  {
    name: "superwhisper",
    bestFor: "Dictation into any app",
    detail:
      "Speak and get clean, per-app formatted text wherever you're typing. Local models by default, with optional cloud models. macOS, Windows, and iOS. Closed source, subscription with a lifetime option.",
    license: "Freemium, subscription",
  },
  {
    name: "Buzz",
    bestFor: "Free open-source transcription on Windows, Mac, and Linux",
    detail:
      "Cross-platform Whisper GUI that runs fully offline: import files, transcribe, translate, export. Not fancy, reliably maintained, genuinely free.",
    license: "Open source, free",
  },
  {
    name: "Vibe",
    bestFor: "Free open-source batch transcription with a modern UI",
    detail:
      "Cross-platform (Tauri-based) offline transcription supporting 90+ languages, batch processing, and multiple export formats. A strong zero-cost default for file transcription.",
    license: "Open source, free",
  },
  {
    name: "whisper.cpp",
    bestFor: "Developers, scripting, and embedding",
    detail:
      "The C/C++ engine most tools on this page are built on. CLI-first, runs everywhere, no UI. If you're comfortable in a terminal, it's the most flexible option there is — several apps here (including ours) are interfaces to it.",
    license: "Open source, free",
  },
] as const;

const sources = [
  { label: "Minutes on GitHub", href: "https://github.com/silverstein/minutes" },
  { label: "MacWhisper", href: "https://goodsnooze.gumroad.com/l/macwhisper" },
  { label: "superwhisper", href: "https://superwhisper.com" },
  { label: "Buzz on GitHub", href: "https://github.com/chidiwilliams/buzz" },
  { label: "Vibe on GitHub", href: "https://github.com/thewh1teagle/vibe" },
  { label: "whisper.cpp on GitHub", href: "https://github.com/ggml-org/whisper.cpp" },
  {
    label: "whisper.cpp vs parakeet.cpp — our engine comparison",
    href: "/writing/whisper-cpp-vs-parakeet-cpp",
  },
] as const;

function SectionLabel({ label }: { label: string }) {
  return (
    <div className="mb-6 flex items-center gap-3">
      <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
        {label}
      </span>
      <div className="h-px flex-1 bg-[var(--border)]" />
    </div>
  );
}

export default function BestLocalSpeechToTextPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/best-local-speech-to-text.md" className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/security" className="hover:text-[var(--accent)]">
            security
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
        </div>
      </div>

      <section className="max-w-[800px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Resource
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          The best local speech-to-text apps
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          &ldquo;Local&rdquo; is the whole point here: every tool on this page transcribes on
          your own hardware, so your audio never touches a vendor&rsquo;s server. But
          &ldquo;best&rdquo; depends entirely on the job — dictating an email, transcribing a
          folder of interviews, and remembering every meeting you&rsquo;ve had are three
          different problems. Full disclosure up front: Minutes is our tool. We&rsquo;ll tell
          you exactly when it&rsquo;s the wrong pick.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-07-11
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Fit-based resource
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick answer
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            For <span className="font-medium text-[var(--text)]">transcribing files with a nice Mac GUI</span>,
            get MacWhisper. For <span className="font-medium text-[var(--text)]">dictation</span>,
            get superwhisper. For{" "}
            <span className="font-medium text-[var(--text)]">free cross-platform transcription</span>,
            get Buzz or Vibe. For{" "}
            <span className="font-medium text-[var(--text)]">scripting and embedding</span>, use
            whisper.cpp directly.
          </p>
          <p>
            For <span className="font-medium text-[var(--text)]">meetings and conversation memory</span> —
            diarized speakers, action items, and an archive your AI agents can search — that&rsquo;s
            the job Minutes exists for, and the one none of the file-transcriber tools attempt.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="The Tools, By Job" />
        <div className="grid gap-4">
          {tools.map((tool) => (
            <div
              key={tool.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h2 className="font-serif text-[22px] text-[var(--text)]">{tool.name}</h2>
                <span className="rounded-full bg-[var(--bg-hover)] px-3 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
                  {tool.license}
                </span>
              </div>
              <p className="mt-2 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {tool.bestFor}
              </p>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">
                {tool.detail}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="How To Actually Choose" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Ask what happens to the transcript after it exists. If the answer is &ldquo;I read it
            once and file it,&rdquo; any file transcriber above will serve you well — pick by
            platform and budget. If the answer is &ldquo;I paste it somewhere else,&rdquo; you
            want dictation, which is superwhisper&rsquo;s specialty (and a mode Minutes includes).
          </p>
          <p>
            If the answer is &ldquo;I want to ask questions about it later&rdquo; — what did we
            decide, who said what, what&rsquo;s still open — you need structure the moment of
            transcription: speaker labels, timestamps, action items, and files an assistant can
            search. That&rsquo;s the memory-layer job, and it&rsquo;s where Minutes is the only
            tool on this list actually built for it. It&rsquo;s also overkill if you just want
            subtitles for a video file — use MacWhisper or Vibe for that and keep your life
            simple.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/security"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            Why on-device matters
          </a>
          <a
            href="/writing/whisper-cpp-vs-parakeet-cpp"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Engine deep-dive
          </a>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Sources" />
        <ul className="space-y-2 text-[14px] leading-7 text-[var(--text-secondary)]">
          {sources.map((source) => (
            <li key={source.href}>
              <a href={source.href} className="text-[var(--accent)] hover:underline">
                {source.label}
              </a>
            </li>
          ))}
        </ul>
      </section>

      <PublicFooter />
    </div>
  );
}
