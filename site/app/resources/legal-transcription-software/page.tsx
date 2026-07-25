import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Legal transcription software: what confidentiality actually requires",
  description:
    "Choosing transcription software for legal work: when you need certified human transcripts, when software is the right tool, and why on-device processing is the only architecture that keeps privileged audio out of third-party clouds.",
  alternates: {
    canonical: "/resources/legal-transcription-software",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "What's the difference between legal transcription services and legal transcription software?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Services use human transcriptionists and can produce certified transcripts admissible in court proceedings. Software produces working transcripts instantly and privately for internal use — depositions prep, client meetings, dictated memos. Most firms need both, for different jobs: certification is a human service; speed and confidentiality for internal work is a software job.",
      },
    },
    {
      "@type": "Question",
      name: "Is AI transcription safe for privileged conversations?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "It depends on the architecture. Cloud AI transcription sends privileged audio to a third party under that vendor's terms — a disclosure your ethics counsel has to analyze. On-device transcription (software that runs entirely on your own machine) involves no third party, so there is no vendor disclosure to analyze. Device security and consent obligations remain yours either way.",
      },
    },
    {
      "@type": "Question",
      name: "Can software-generated transcripts be used in court?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Software transcripts are working documents, not certified transcripts. Court-grade transcripts of proceedings generally require a certified transcriptionist or court reporter. Use software for speed and confidentiality on internal material; hire certification when the transcript itself must be evidence.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "AI notetakers and attorney–client privilege — our full analysis",
    href: "/resources/ai-notetakers-attorney-client-privilege",
  },
  {
    label: "ABA Formal Opinion 512: Generative AI Tools (July 2024)",
    href: "https://www.americanbar.org/content/dam/aba/administrative/professional_responsibility/ethics-opinions/aba-formal-opinion-512.pdf",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
  { label: "Minutes on GitHub (MIT)", href: "https://github.com/silverstein/minutes" },
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

export default function LegalTranscriptionSoftwarePage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqJsonLd) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a href="/resources/legal-transcription-software.md" className="hover:text-[var(--accent)]">
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
          Legal transcription software: what confidentiality actually requires
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Most &ldquo;legal transcription software&rdquo; roundups compare turnaround times and
          per-minute prices, and skip the only question that can end a career: where does the
          privileged audio go? Here&rsquo;s a clear map of the category — what needs a human,
          what needs software, and what the software&rsquo;s architecture must look like when
          the recording is privileged.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-07-11
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Not legal advice
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick answer
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">
              Certified transcripts are a human service, not a software feature.
            </span>{" "}
            When the transcript itself must be evidence — proceedings, certified deposition
            transcripts — you hire a certified transcriptionist or court reporter. No software
            replaces that.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              Everything else is a software job, and architecture is the buying criterion.
            </span>{" "}
            Client meetings, dictated memos, interview prep, internal case discussions: software
            transcribes them instantly. But cloud transcription tools put a third party inside
            privileged conversations. On-device tools — where audio is transcribed and stored
            entirely on your own machine — are the only architecture where no outside disclosure
            occurs at all.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Three Jobs Firms Actually Have" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">1. Court-grade transcripts.</span>{" "}
            Human, certified, formatted to jurisdiction rules. Budget for a service; this
            category isn&rsquo;t what software is for.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">2. Working transcripts of privileged material.</span>{" "}
            Client calls, strategy discussions, dictated case notes. Speed matters, but
            confidentiality governs: this is where the vendor-in-the-loop question from our{" "}
            <a
              href="/resources/ai-notetakers-attorney-client-privilege"
              className="text-[var(--accent)] hover:underline"
            >
              privilege analysis
            </a>{" "}
            applies with full force. ABA Formal Opinion 512 requires understanding a tool&rsquo;s
            data handling before client information goes in — an analysis that gets one sentence
            long when the tool never transmits anything.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">3. Non-privileged volume.</span>{" "}
            Public hearings, recorded CLEs, marketing content. Any decent transcription tool
            works; pick on price and convenience.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Where Minutes Fits — And Doesn't" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Minutes</span> is built for job #2.
            It records and transcribes on your own machine (whisper.cpp — the audio has no
            network path), labels speakers, and stores everything as markdown files on your disk
            with owner-only permissions — organized, greppable, and readable by your AI
            assistant straight from local files, with structured action items filled in once you
            connect one. It&rsquo;s open source (MIT), so your security review can read the code
            instead of a vendor questionnaire.
          </p>
          <p>
            Where it is <em>not</em> the tool: certified transcripts (job #1 — hire a human),
            jurisdiction-formatted verbatim output, or medical-legal templating. And it
            doesn&rsquo;t change your own obligations — recording consent, device encryption,
            matter-based access control stay with you, where they already live.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/resources/ai-notetakers-attorney-client-privilege"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            Read the privilege analysis
          </a>
          <a
            href="/security"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            See the architecture
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
        <p className="mt-6 text-[13px] leading-6 text-[var(--text-tertiary)]">
          Informational, not legal advice. Certification requirements and consent law vary by
          jurisdiction — confirm with your ethics counsel.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
