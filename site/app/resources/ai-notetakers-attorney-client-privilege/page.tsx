import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "AI notetakers and attorney–client privilege",
  description:
    "Privilege depends on confidentiality, and cloud AI notetakers put a third party inside your client conversations. What ABA Formal Opinion 512 says, the questions to ask any vendor, and why on-device transcription changes the analysis.",
  alternates: {
    canonical: "/resources/ai-notetakers-attorney-client-privilege",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Does using an AI notetaker waive attorney–client privilege?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "There is no blanket answer — waiver is fact-specific and courts have not squarely resolved cloud AI notetakers. The risk vector is clear, though: privilege depends on confidentiality, and sending client conversations to a third-party cloud service is a disclosure that opposing counsel can probe. Vendor terms allowing human review or model training make the argument harder to defend.",
      },
    },
    {
      "@type": "Question",
      name: "What does ABA Formal Opinion 512 say about AI tools and confidentiality?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "ABA Formal Opinion 512 (July 2024) addresses generative AI under Model Rule 1.6: lawyers must evaluate a tool's data handling before inputting client information, may need informed client consent for self-learning tools, and remain responsible for understanding where client data goes. It does not ban AI tools; it requires lawyers to actually understand the data flow.",
      },
    },
    {
      "@type": "Question",
      name: "Do I need consent to record client meetings?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Recording-consent law is separate from privilege and varies by state: some states require all-party consent, others one-party. Best practice for client conversations is explicit consent regardless of state law — it is also the professional norm.",
      },
    },
    {
      "@type": "Question",
      name: "How does on-device transcription change the privilege analysis?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "If transcription runs entirely on the attorney's own machine and the transcript is a local file, no third party ever receives the communication — there is no vendor disclosure to analyze, no terms of service governing client data, and no vendor to subpoena. The remaining duties are the familiar ones: device security and access control.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "ABA Formal Opinion 512: Generative Artificial Intelligence Tools (July 2024)",
    href: "https://www.americanbar.org/content/dam/aba/administrative/professional_responsibility/ethics-opinions/aba-formal-opinion-512.pdf",
  },
  {
    label: "RCFP Reporter's Recording Guide (state-by-state consent law)",
    href: "https://www.rcfp.org/reporters-recording-guide/",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
  {
    label: "Is Otter.ai HIPAA compliant? (the parallel analysis for healthcare)",
    href: "/resources/is-otter-ai-hipaa-compliant",
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

export default function AiNotetakersPrivilegePage() {
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
          <a
            href="/resources/ai-notetakers-attorney-client-privilege.md"
            className="hover:text-[var(--accent)]"
          >
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
          AI notetakers and attorney&ndash;client privilege
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Privilege has one load-bearing wall: confidentiality. An AI notetaker that streams your
          client conversations to a vendor&rsquo;s cloud puts a third party inside that wall, and
          nobody — not the vendor, not the bar, not yet a court — has given lawyers a clean
          answer on what that does to privilege. Here is the current state of the analysis, and
          the architectural choice that makes most of it moot.
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
              Waiver is fact-specific and unsettled — but the risk vector is not.
            </span>{" "}
            Privilege requires confidentiality; a cloud notetaker is a disclosure to a third
            party under that vendor&rsquo;s terms. Whether that disclosure defeats privilege will
            depend on the vendor&rsquo;s data handling, your diligence, and a judge — three
            things you don&rsquo;t fully control.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              On-device transcription removes the third party entirely.
            </span>{" "}
            A transcript generated and stored on the attorney&rsquo;s own machine involves no
            outside disclosure to argue about. The confidentiality question collapses back to the
            one you already answer daily: is your laptop secure?
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What The Bar Has Actually Said" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            The closest authoritative guidance is{" "}
            <span className="font-medium text-[var(--text)]">ABA Formal Opinion 512</span> (July
            2024) on generative AI. Its core demand is unglamorous: before client information
            goes into an AI tool, the lawyer must actually understand the tool&rsquo;s data
            handling — who can access inputs, whether they train models, how long they&rsquo;re
            retained — and in some cases obtain informed client consent. Competence about the
            tool is part of the duty of confidentiality, not separate from it.
          </p>
          <p>
            Applied to meeting notetakers, that means reading the vendor&rsquo;s terms the way
            opposing counsel would: Does the vendor&rsquo;s staff have access to transcripts for
            &ldquo;service improvement&rdquo;? Are recordings used for model training, by them or
            their subprocessors? What happens to your clients&rsquo; conversations under a
            subpoena served on the vendor? Every &ldquo;yes&rdquo; and &ldquo;unclear&rdquo; is
            material you may someday have to defend.
          </p>
          <p>
            Separately from privilege, recording consent law applies: some states require
            all-party consent to record a conversation, others one-party. For client meetings,
            explicit consent is the professional norm regardless of what your state minimally
            permits — the RCFP guide linked below has the state-by-state map.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Vendor Questions That Matter" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>If you&rsquo;re evaluating any cloud notetaker for client work, get written answers to:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>Where is audio transcribed, and by which subprocessors?</li>
            <li>Is any human review of transcripts possible, ever, for any purpose?</li>
            <li>Are recordings or transcripts used to train models — theirs or anyone&rsquo;s?</li>
            <li>What is the retention period, and is deletion verifiable?</li>
            <li>What is their process when they receive legal process for your data?</li>
            <li>Will they sign confidentiality terms that survive their standard ToS?</li>
          </ul>
          <p>
            Notice what this list is: a due-diligence file you must build and maintain for a
            vendor whose entire involvement is optional.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Architectural Answer" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Every question above exists because the conversation leaves your machine. Run
            transcription on-device instead, and the third party disappears: no vendor receives
            the communication, no terms of service govern it, no subpoena served on a vendor can
            reach it, and there is no &ldquo;service improvement&rdquo; clause to parse. That is
            the design of <span className="font-medium text-[var(--text)]">Minutes</span> — open
            source, on-device transcription and diarization, transcripts as markdown files on
            your own disk with owner-only permissions. The privilege analysis returns to familiar
            ground: secure the device, control access by matter, and document client consent to
            recording.
          </p>
          <p>
            What on-device does <em>not</em> do: it doesn&rsquo;t satisfy consent law for you,
            secure an unencrypted laptop, or make a transcript less discoverable in litigation —
            your own files are always discoverable. It removes the third-party disclosure
            question, which is the one you can&rsquo;t fix after the fact.
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
            See the on-device architecture
          </a>
          <a
            href="/compare"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Compare notetakers
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
          This page is informational, not legal advice. Privilege and consent questions are
          jurisdiction- and fact-specific — consult your ethics counsel before adopting any
          recording tool for client work.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
