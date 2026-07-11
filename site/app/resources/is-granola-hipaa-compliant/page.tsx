import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Is Granola HIPAA compliant?",
  description:
    "No — Granola's own documentation states it is not HIPAA compliant, cannot sign BAAs, and should not be used for PHI. The sourced answer, what Granola does offer, and the on-device alternative for clinical conversations.",
  alternates: {
    canonical: "/resources/is-granola-hipaa-compliant",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Is Granola HIPAA compliant?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "No. Granola's official documentation states: 'Granola is not currently HIPAA compliant and should not be used to store or process Protected Health Information (PHI) at this time.' This applies to every plan, including Enterprise.",
      },
    },
    {
      "@type": "Question",
      name: "Does Granola sign BAAs?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "No. Granola's docs state it 'cannot sign Business Associate Agreements (BAAs)' on any plan, and give no timeline for HIPAA compliance.",
      },
    },
    {
      "@type": "Question",
      name: "What security does Granola actually offer?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "A genuinely decent cloud posture for non-PHI work: SOC 2 Type 2 (achieved July 2025), GDPR with a standard DPA, audio deleted after transcription, and contractual bans on OpenAI/Anthropic training on customer data. Transcripts and notes are stored on US AWS servers.",
      },
    },
    {
      "@type": "Question",
      name: "What should clinicians use instead for recorded conversations?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Either a vendor that offers a BAA in a documented configuration (Otter Enterprise, Fireflies Enterprise with Private Storage, Fathom's published BAA), or an on-device tool where no vendor ever receives the audio — such as Minutes, open source, which transcribes and stores everything on your own machine, so no BAA is needed because no disclosure occurs.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "Granola docs: Is Granola HIPAA compliant?",
    href: "https://docs.granola.ai/help-center/consent-security-privacy/is-granola-hipaa-compliant",
  },
  {
    label: "Granola docs: security, privacy & data FAQs",
    href: "https://docs.granola.ai/help-center/consent-security-privacy/security-privacy-data-faqs",
  },
  { label: "Granola security", href: "https://www.granola.ai/security" },
  { label: "Granola pricing", href: "https://www.granola.ai/pricing" },
  { label: "Minutes vs Granola — full comparison", href: "/compare/granola-vs-minutes" },
  { label: "Minutes security & privacy architecture", href: "/security" },
  { label: "Is Otter.ai HIPAA compliant?", href: "/resources/is-otter-ai-hipaa-compliant" },
  { label: "Is Fireflies.ai HIPAA compliant?", href: "/resources/is-fireflies-ai-hipaa-compliant" },
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

export default function IsGranolaHipaaCompliantPage() {
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
          <a href="/resources/is-granola-hipaa-compliant.md" className="hover:text-[var(--accent)]">
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
          Is Granola HIPAA compliant?
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          No — and unusually for this category, there&rsquo;s no ambiguity to untangle: Granola
          says so itself, plainly, in its own documentation. That candor deserves respect. What
          needs correcting is the handful of third-party articles implying otherwise. Here is
          the record, and what clinicians who like Granola&rsquo;s botless design can use
          instead.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-07-11
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Sourced answer
          </span>
        </div>
      </section>

      <section className="mt-12 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Quick answer
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">No, on every plan.</span> From
            Granola&rsquo;s own help center: &ldquo;Granola is not currently HIPAA compliant and
            should not be used to store or process Protected Health Information (PHI) at this
            time.&rdquo; Its docs add that Granola &ldquo;cannot sign Business Associate
            Agreements (BAAs)&rdquo; and give no timeline for compliance. The $35/user Enterprise
            tier does not change this — no plan lists HIPAA, BAA, or PHI support.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              Ignore third-party posts claiming otherwise.
            </span>{" "}
            A few SEO articles imply Granola suits medical transcription. They contradict
            Granola&rsquo;s own documentation, which is the only source that matters here.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What Granola Does Offer" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            For non-PHI work, Granola&rsquo;s cloud posture is genuinely decent: SOC 2 Type 2
            (achieved July 2025), GDPR with a standard DPA, botless capture, audio deleted after
            transcription, and contractual bans on OpenAI and Anthropic training on customer
            data. The structural facts remain: transcription happens in the cloud (Deepgram and
            AssemblyAI), note enhancement through OpenAI and Anthropic, and transcripts live on
            US-based AWS servers. That architecture is why the HIPAA answer is what it is — a
            cloud notepad would need a BAA chain across every one of those vendors to touch PHI
            lawfully, and Granola has chosen not to build one yet.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="If You're A Clinician Who Likes Granola's Design" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            The appeal of Granola is real: no bot in the call, capture on your own device, notes
            that feel like yours. If that&rsquo;s what drew you in, note that the part you like
            — device-side capture — is exactly the part that can be taken all the way.{" "}
            <span className="font-medium text-[var(--text)]">Minutes</span> keeps capture on
            your device like Granola does, then keeps transcription and storage there too:
            whisper.cpp locally, markdown on your own disk, owner-only permissions, open source.
            No vendor receives the conversation, so there is no business associate and no BAA
            question at all — for a solo practice, that&rsquo;s the entire compliance
            conversation about the vendor, finished. (Your own duties — device encryption,
            access control, patient consent to recording — remain, as they do with any tool.)
          </p>
          <p>
            If you need a cloud product with team features and a BAA, the documented options are
            Otter (Enterprise + BAA), Fireflies (Enterprise + Private Storage + BAA), and Fathom
            (published blanket BAA) — see our sourced write-ups linked below before relying on
            any of them.
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
            href="/compare/granola-vs-minutes"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Minutes vs Granola
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
          This page is informational, not legal advice. Vendor policies change — verify against
          Granola&rsquo;s current documentation and your compliance counsel before recording any
          patient conversation.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
