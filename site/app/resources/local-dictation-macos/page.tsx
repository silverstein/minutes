import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Local dictation on macOS: the complete guide",
  description:
    "Every way to dictate on a Mac without sending your voice to a cloud: built-in macOS dictation, superwhisper, MacWhisper, and Minutes — what each is for, and how to pick.",
  alternates: {
    canonical: "/resources/local-dictation-macos",
  },
};

const options = [
  {
    name: "Built-in macOS dictation",
    bestFor: "Zero-install, occasional use",
    detail:
      "Press the shortcut, talk, done. Apple processes many languages on-device on Apple Silicon Macs, with automatic punctuation. No custom vocabulary and no formatting modes — but it's free, already installed, and fine for a quick sentence.",
  },
  {
    name: "superwhisper",
    bestFor: "Heavy daily dictation with per-app formatting",
    detail:
      "The most polished dedicated dictation tool on the Mac: local Whisper-family models by default, custom modes that reformat your speech per app (email vs Slack vs prose), 100+ languages. Closed source; free tier with a Pro subscription and lifetime option.",
  },
  {
    name: "MacWhisper",
    bestFor: "File transcription first, dictation included",
    detail:
      "Primarily the best drag-and-drop file transcriber on macOS, with a system-wide dictation feature included in the direct-download version (the App Store build lacks dictation). If you mostly transcribe recordings and only sometimes dictate, the direct version covers both. Closed source; free tier, one-time Pro purchase.",
  },
  {
    name: "Minutes",
    bestFor: "Dictation as part of a conversation-memory system",
    detail:
      "Open source (MIT) and free. Dictation is one of four capture modes: speak, and the text is typed at your cursor (or lands in your clipboard via the CLI), with a timestamped copy in your daily note — alongside meeting recording, voice memos, and live transcription, all on-device, all searchable by your AI agents via MCP. Full disclosure: Minutes is our tool.",
  },
] as const;

const sources = [
  { label: "Apple: use voice dictation on Mac", href: "https://support.apple.com/guide/mac-help/use-dictation-mh40584/mac" },
  { label: "superwhisper", href: "https://superwhisper.com" },
  { label: "MacWhisper", href: "https://goodsnooze.gumroad.com/l/macwhisper" },
  { label: "Minutes on GitHub", href: "https://github.com/silverstein/minutes" },
  { label: "Minutes vs superwhisper — full comparison", href: "/compare/superwhisper-vs-minutes" },
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

export default function LocalDictationMacosPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/local-dictation-macos.md" className="hover:text-[var(--accent)]">
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
          Local dictation on macOS: the complete guide
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Dictation is the most personal audio there is — half-formed thoughts, names, drafts you
          would never say out loud in a meeting. It&rsquo;s also where cloud processing is least
          necessary: modern Macs transcribe speech locally faster than you can talk. Here is
          every serious way to dictate on a Mac with the audio staying on the machine, and how
          to pick between them.
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
            Occasional sentence? Use{" "}
            <span className="font-medium text-[var(--text)]">built-in macOS dictation</span> —
            it&rsquo;s already there. Dictating all day into different apps?{" "}
            <span className="font-medium text-[var(--text)]">superwhisper</span> is the most
            polished dedicated tool. Want dictation plus a private, searchable record of
            everything you capture — meetings, memos, and dictations — that your AI agents can
            query? That&rsquo;s <span className="font-medium text-[var(--text)]">Minutes</span>,
            free and open source.
          </p>
          <p>
            One tool to be careful with if &ldquo;local&rdquo; is your requirement: several
            popular dictation apps (Wispr Flow being the best known) process your speech in the
            cloud. Fast and polished — but a different privacy contract than everything on this
            page.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="The Local Options" />
        <div className="grid gap-4">
          {options.map((opt) => (
            <div
              key={opt.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <h2 className="font-serif text-[22px] text-[var(--text)]">{opt.name}</h2>
              <p className="mt-2 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
                {opt.bestFor}
              </p>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">
                {opt.detail}
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Setting Up Minutes Dictation" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            If you go the Minutes route: install, run{" "}
            <code className="rounded-[3px] bg-[var(--bg-hover)] px-1.5 py-0.5 font-mono text-[13px]">
              minutes setup --model tiny
            </code>{" "}
            once to download a local Whisper model, and bind the dictation hotkey in the menu bar
            app. Speak; the text is inserted where your cursor is (the CLI mode lands it in your
            clipboard instead), and a timestamped copy is appended to your daily note in{" "}
            <code className="rounded-[3px] bg-[var(--bg-hover)] px-1.5 py-0.5 font-mono text-[13px]">
              ~/meetings
            </code>{" "}
            — which means every idea you&rsquo;ve ever dictated is greppable, and your agents can
            answer &ldquo;what was that idea I had last Tuesday?&rdquo; That daily-note trail is
            the practical difference from pure dictation tools: text you dictate into other apps
            vanishes into those apps; text that also lands in your own files compounds.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="How To Choose" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Volume decides it. A few sentences a week: the built-in dictation is genuinely
            enough. Hours of daily dictation where per-app formatting saves real time:
            superwhisper earns its subscription. Mostly transcribing files, occasionally
            dictating: MacWhisper covers both with one one-time purchase. And if dictation is one
            piece of a bigger habit — capturing meetings, memos, and ideas into a private archive
            your assistant can search — Minutes does all four modes, free, with the code open for
            inspection.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="https://github.com/silverstein/minutes"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            Get Minutes
          </a>
          <a
            href="/compare/superwhisper-vs-minutes"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Minutes vs superwhisper
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
