import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Security — audio that never leaves your device",
  description:
    "Minutes' security model is architectural, not contractual: capture, transcription, diarization, and storage all run on your own machine. No vendor cloud to trust, breach, or subpoena — and it's open source, so you can verify every claim in code.",
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
    detail: "whisper.cpp or parakeet.cpp, running on your CPU/GPU",
  },
  {
    step: "Diarize",
    detail: "pyannote ONNX models, local — speaker labels never computed in a cloud",
  },
  {
    step: "Store",
    detail: "Markdown + YAML frontmatter on your own disk, 0600 owner-only permissions",
  },
] as const;

const guarantees = [
  {
    title: "No audio upload, ever",
    body: "There is no code path that sends your recordings to a server. Transcription is not 'private-by-policy' — the cloud client simply doesn't exist.",
  },
  {
    title: "Files you own outright",
    body: "The durable record is plain markdown in ~/meetings on your disk, written with 0600 permissions. Grep it, back it up, delete it — no export button between you and your data.",
  },
  {
    title: "No account, no vendor database",
    body: "There's nothing to sign up for, so there's no server-side profile of your conversations to breach or subpoena.",
  },
  {
    title: "Open source, MIT",
    body: "Every claim on this page is verifiable in the repository — capture, transcription, and storage are readable Rust, not a trust-center PDF.",
  },
] as const;

const sources = [
  { label: "Minutes on GitHub (MIT)", href: "https://github.com/silverstein/minutes" },
  { label: "Minutes for agents", href: "https://useminutes.app/for-agents" },
  { label: "Is Otter.ai HIPAA compliant?", href: "/resources/is-otter-ai-hipaa-compliant" },
  { label: "Compare Minutes", href: "/compare" },
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
          Nothing to trust. Nothing to breach. Nothing to subpoena.
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Cloud notetakers answer the security question with policies: encryption in transit,
          deletion windows, SOC 2 reports, BAAs. Minutes answers it with architecture. Your
          conversations are captured, transcribed, diarized, and stored on your own machine — so
          the promises those policies exist to make are simply not needed. &ldquo;We delete your
          audio after processing&rdquo; is a policy. &ldquo;We never had your audio&rdquo; is an
          architecture.
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
              <h2 className="font-serif text-[20px] text-[var(--text)]">{g.title}</h2>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{g.body}</p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What Does Touch The Network" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            A useful security page has to name the exceptions, so here is the complete list.
            Minutes downloads transcription and diarization models once, at setup.
            If you install updates, those come over the network too. And if you enable automated
            summarization — it is off by default — your transcript text goes wherever you point
            it: a local model via Ollama, an agent CLI you&rsquo;ve signed into (claude, codex,
            gemini — which round-trips through that provider&rsquo;s cloud), or a cloud API if
            you supply a key.
          </p>
          <p>
            What is never in that traffic: your audio and your transcripts, unless you yourself
            configured a summarizer to receive them. Out of the box, Minutes needs no API key and
            sends conversation content nowhere. When Claude summarizes a meeting through MCP, it
            reads local files through tools you granted — visible in your agent&rsquo;s tool log,
            not a background sync — and what it reads travels to your agent&rsquo;s model
            provider as conversation context, like anything else you show your agent.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="For Regulated Work" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Healthcare.</span> HIPAA&rsquo;s
            business-associate machinery exists because vendors receive PHI. On-device processing
            means no vendor receives anything — there is no business associate, so there is no BAA
            to negotiate. Your own obligations (disk encryption, access control, recording consent)
            remain, exactly as they do for the workstation your EHR runs on. See our full analysis:{" "}
            <a
              href="/resources/is-otter-ai-hipaa-compliant"
              className="text-[var(--accent)] hover:underline"
            >
              Is Otter.ai HIPAA compliant?
            </a>
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">Legal.</span> Privilege analyses get
            harder every time a third party touches client communications. A transcript that never
            leaves the attorney&rsquo;s machine involves no third-party disclosure to argue about.
            (Informational, not legal advice — run it past your ethics counsel.)
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">EU / GDPR.</span> Cloud notetakers
            make you evaluate a processor, sign a DPA, and check data-residency maps. With
            on-device processing there is no processor and no transfer — the data never leaves the
            controller&rsquo;s machine.
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
            word for an architecture claim — read the code, or have your security team do it.
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
