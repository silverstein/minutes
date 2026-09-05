import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";

export const metadata: Metadata = {
  title: "Security and privacy: local capture, explicit AI sharing",
  description:
    "Capture, transcription, and storage run locally. Learn what stays on your device and what a connected cloud AI provider can receive.",
  alternates: {
    canonical: "/security",
  },
};

const pipeline = [
  {
    step: "Capture",
    detail: "Mic (cpal) and system audio (native macOS capture in the desktop app, or a loopback device), recorded on your machine",
  },
  {
    step: "Transcribe",
    detail: "sealed local whisper.cpp, running on your CPU/GPU",
  },
  {
    step: "Diarize",
    detail: "pyannote ONNX models, local; speaker labels never computed in a cloud",
  },
  {
    step: "Store",
    detail: "Markdown + YAML frontmatter on your own disk, 0600 owner-only permissions",
  },
] as const;

const guarantees = [
  {
    title: "Local recording and transcription",
    body: "The recording and transcription pipeline processes audio on your device. Optional AI assistants and summarizers are a separate boundary: a cloud provider receives the context you authorize.",
  },
  {
    title: "Files you own outright",
    body: "The durable record is plain markdown in ~/meetings on your disk, written with 0600 permissions. Grep it, back it up, delete it; no export button between you and your data.",
  },
  {
    title: "No account, no vendor database",
    body: "Minutes does not require an account to capture and store conversations. Your device security, backups, file-sync services, and chosen AI providers still matter.",
  },
  {
    title: "Open source, MIT",
    body: "Every claim on this page is verifiable in the repository; capture, transcription, and storage are readable Rust, not a trust-center PDF.",
  },
] as const;

const sources = [
  { label: "Minutes on GitHub (MIT)", href: "https://github.com/silverstein/minutes" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Is Otter.ai HIPAA compliant?", href: "/resources/is-otter-ai-hipaa-compliant" },
  { label: "Compare Minutes", href: "/compare" },
] as const;


export default function SecurityPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/security.md" className="hover:text-[var(--accent)]">
            page.md
          </a>
          <a href="/compare" className="hover:text-[var(--accent)]">
            compare
          </a>
          <a href="/docs" className="hover:text-[var(--accent)]">
            docs
          </a>
        </div>
      </div>

      <section className="max-w-[800px]">
        <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-[var(--accent)]">
          Security &amp; Privacy
        </p>
        <h1 className="mt-4 font-serif text-[40px] leading-[0.98] tracking-[-0.045em] text-[var(--text)] sm:text-[58px]">
          Your records stay local. Your AI connections are your choice.
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Minutes captures, transcribes, and stores conversations on your own machine.
          If you connect a cloud AI assistant or summarizer, the meeting context you
          authorize is sent to that provider. Use a local model or skip summarization
          for an on-device workflow. Local storage does not remove your responsibility
          for access controls, consent, backups, or retention.
        </p>
      </section>

      <section className="mt-14">
        <SectionLabel label="The Pipeline" />
        <div className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
          <div className="flex items-center justify-between gap-3">
            <p className="font-mono text-[13px] font-medium text-[var(--text)]">
              Every step, on your device
            </p>
            <span className="rounded-full bg-[var(--accent-soft)] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.14em] text-[var(--accent)]">
              Stays on device
            </span>
          </div>
          <ol className="mt-5">
            {pipeline.map((item, i) => (
              <li key={item.step}>
                <div className="rounded-[6px] border border-[color:var(--border)] bg-[var(--bg)] px-4 py-3">
                  <div className="flex items-center justify-between gap-3">
                    <span className="font-mono text-[13px] text-[var(--text)]">{item.step}</span>
                    <span className="shrink-0 font-mono text-[10px] uppercase tracking-[0.12em] text-[var(--accent)]">
                      on-device
                    </span>
                  </div>
                  <p className="mt-1 font-mono text-[11px] leading-5 text-[var(--text-secondary)]">
                    {item.detail}
                  </p>
                </div>
                {i < pipeline.length - 1 ? (
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
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="What That Buys You" />
        <div className="grid gap-5 lg:grid-cols-2">
          {guarantees.map((g) => (
            <div
              key={g.title}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <h3 className="font-serif text-[20px] text-[var(--text)]">{g.title}</h3>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{g.body}</p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What Does Touch The Network" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Setup and updates can download models and software. Connected AI
            assistants and summarizers may send authorized transcript context to
            their model provider. A local model keeps that processing on-device.
            Review each assistant and provider configuration before sharing a meeting.
          </p>
          <p>
            Files copied into a synced folder follow that service&apos;s settings.
            Your browser also uses website analytics on this site, separately from
            the desktop recording pipeline. Website download and setup events contain
            an action category and page path, not meeting content or clipboard text.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="For Regulated Work" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Local processing is a deployment choice, not a compliance certification.
            Review recording consent, device access, encryption, retention, backup
            destinations, and any cloud AI provider with the people responsible
            for your organization&apos;s requirements.
          </p>
          <p>
            Do not infer that local files are immune from disclosure obligations
            or that connecting a cloud assistant creates no third-party access.
            See the <a href="/docs/agent-integrations" className="text-[var(--accent)] hover:underline">agent integration guide</a> for the data path.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Verify It Yourself" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Minutes is MIT-licensed and the entire pipeline is readable Rust. Audio capture lives
            in <code className="rounded-[3px] bg-[var(--bg-hover)] px-1.5 py-0.5 font-mono text-[13px]">crates/core/src/capture.rs</code>,
            transcription in{" "}
            <code className="rounded-[3px] bg-[var(--bg-hover)] px-1.5 py-0.5 font-mono text-[13px]">crates/core/src/transcribe.rs</code>,
            and output permissions where files are written. Don&rsquo;t take an SEO page&rsquo;s
            word for an architecture claim; read the code, or have your security team do it.
            That&rsquo;s the point of shipping it open.
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
            Read the source
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Compare architectures
          </a>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Related" />
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
