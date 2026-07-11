import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "HIPAA-compliant AI note takers: the actual state of play",
  description:
    "Which AI note takers can be used with PHI, under exactly what conditions — Otter, Fireflies, Fathom, Krisp, Granola — plus the on-device architecture that removes the BAA question entirely. Every claim sourced to the vendor's own documentation.",
  alternates: {
    canonical: "/resources/hipaa-compliant-ai-note-taker",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Which AI note takers are HIPAA compliant?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "As of July 2026, per each vendor's own documentation: Otter (Enterprise plan + signed BAA only), Fireflies (Enterprise + Private Storage + signed BAA, all three simultaneously), Fathom (publishes a blanket BAA; pricing lists HIPAA BAA under Enterprise), and Krisp (BAA on business/enterprise tiers). Granola states it is not HIPAA compliant and cannot sign BAAs. On-device tools like Minutes sit outside the BAA framework entirely because no vendor ever receives the audio.",
      },
    },
    {
      "@type": "Question",
      name: "Is any AI note taker 'HIPAA certified'?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "No. HHS certifies no product. 'HIPAA compliant' is a vendor self-attestation that its safeguards meet HIPAA requirements when used under a signed BAA. Any tool marketing itself as 'HIPAA certified' is being imprecise.",
      },
    },
    {
      "@type": "Question",
      name: "Does using a free or Pro plan of these tools violate HIPAA?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Recording PHI on a plan tier with no BAA in effect is an impermissible disclosure to a vendor, regardless of the vendor's general security quality. Every cloud vendor on this page gates its BAA to its top tier — free and mid-tier plans are not covered.",
      },
    },
    {
      "@type": "Question",
      name: "Why don't on-device note takers need a BAA?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "A BAA governs a vendor's handling of PHI it receives on your behalf. Software that captures, transcribes, and stores entirely on your own machine never gives the vendor the data — no business associate relationship is created, so there is nothing for a BAA to govern. Your own obligations (device encryption, access controls, patient consent) remain.",
      },
    },
  ],
} as const;

const vendors = [
  {
    name: "Otter.ai",
    verdict: "Conditional",
    condition: "Enterprise plan + signed BAA (announced July 2025)",
    architecture: "Cloud — audio processed and stored on Otter's servers",
    detail:
      "Basic, Pro, and Business tiers cannot obtain a BAA. Compliance is a plan-tier purchase, and the PHI still lives in Otter's cloud under contract.",
    href: "/resources/is-otter-ai-hipaa-compliant",
  },
  {
    name: "Fireflies.ai",
    verdict: "Conditional",
    condition: "Enterprise ($39/user/mo annual) + Private Storage + signed BAA — all three at once",
    architecture: "Cloud — US AWS/GCP by default; AI via OpenAI and ASR vendors under zero-retention BAAs",
    detail:
      "Documented unusually precisely by Fireflies itself: if any one condition lapses, compliance is disabled. Free, Pro, and Business are not covered.",
    href: "/resources/is-fireflies-ai-hipaa-compliant",
  },
  {
    name: "Fathom",
    verdict: "Conditional",
    condition: "Publishes a blanket BAA; pricing lists 'HIPAA BAA' under Enterprise",
    architecture: "Cloud — US-only storage, indefinite default retention; AI via Anthropic/OpenAI/Google",
    detail:
      "The published blanket BAA is unusually accessible. The plan gating is stated ambiguously across Fathom's own pages — confirm in writing which tier your BAA covers before recording PHI.",
    href: "/compare/fathom-vs-minutes",
  },
  {
    name: "Krisp",
    verdict: "Conditional",
    condition: "BAA available per its security page (which references a legacy 'Business tier'; pricing lists BAA under Enterprise)",
    architecture: "Hybrid — noise cancellation on-device; AI notes via Azure; transcripts in Krisp Cloud once notes are enabled",
    detail:
      "The famous on-device processing applies to the audio-cleanup path, not the notes pipeline. On-device transcript storage is an Enterprise feature.",
    href: "/compare/krisp-vs-minutes",
  },
  {
    name: "Granola",
    verdict: "No",
    condition: "None — Granola states it cannot sign BAAs on any plan",
    architecture: "Cloud — transcription via Deepgram/AssemblyAI, notes via OpenAI/Anthropic, storage on US AWS",
    detail:
      "Granola says it plainly in its own docs: not HIPAA compliant, don't use it for PHI. Respect the candor; ignore third-party posts claiming otherwise.",
    href: "/resources/is-granola-hipaa-compliant",
  },
  {
    name: "Minutes (ours)",
    verdict: "No BAA needed",
    condition: "Open source, free — the vendor never receives the data",
    architecture: "On-device — capture, transcription, diarization, and storage all on your own machine",
    detail:
      "Not 'HIPAA certified' (nothing is). On-device processing means no business associate exists; the compliance surface reduces to your own device controls and consent workflow.",
    href: "/security",
  },
] as const;

const sources = [
  { label: "Otter Help Center: HIPAA", href: "https://help.otter.ai/hc/en-us/articles/33975072019991-HIPAA-Otter-ai" },
  { label: "Fireflies: set up HIPAA compliance", href: "https://guide.fireflies.ai/articles/3704059205-set-up-hipaa-compliance-for-your-workspace" },
  { label: "Fathom blanket BAA", href: "https://www.fathom.ai/baa" },
  { label: "Fathom HIPAA help article", href: "https://help.fathom.video/en/articles/5291265" },
  { label: "Krisp security for AI Meeting Assistant", href: "https://krisp.ai/security-for-ai-meeting-assistant/" },
  { label: "Granola docs: Is Granola HIPAA compliant?", href: "https://docs.granola.ai/help-center/consent-security-privacy/is-granola-hipaa-compliant" },
  { label: "HHS: Business Associate Contracts", href: "https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html" },
  { label: "Minutes security & privacy architecture", href: "/security" },
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

export default function HipaaCompliantAiNoteTakerPage() {
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
          <a href="/resources/hipaa-compliant-ai-note-taker.md" className="hover:text-[var(--accent)]">
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
          HIPAA-compliant AI note takers: the actual state of play
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Every vendor in this category now says something about HIPAA, and almost every summary
          you&rsquo;ll find flattens the conditions that make the claims true or false. Here is
          the July 2026 state of play, one vendor at a time, each sourced to that vendor&rsquo;s
          own documentation — including ours.
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
          Two rules before the list
        </p>
        <div className="mt-4 space-y-3 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Nothing is &ldquo;HIPAA certified.&rdquo;</span>{" "}
            HHS certifies no product, ever. &ldquo;Compliant&rdquo; means the vendor attests its
            safeguards meet HIPAA&rsquo;s requirements when used under a signed BAA — it is a
            self-attestation plus a contract, not a government stamp.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">The plan tier is the whole ballgame.</span>{" "}
            Every cloud vendor below gates its BAA to a top tier. A clinician on a free or Pro
            plan of any of them is making impermissible disclosures, no matter how good the
            vendor&rsquo;s security page looks.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Vendor By Vendor" />
        <div className="grid gap-4">
          {vendors.map((v) => (
            <div
              key={v.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h2 className="font-serif text-[22px] text-[var(--text)]">{v.name}</h2>
                <span
                  className={`rounded-full px-3 py-1 font-mono text-[10px] uppercase tracking-[0.14em] ${
                    v.verdict === "No"
                      ? "bg-[var(--bg-hover)] text-[var(--text-secondary)]"
                      : v.verdict === "No BAA needed"
                        ? "bg-[var(--accent-soft)] text-[var(--accent)]"
                        : "bg-[var(--bg-hover)] text-[var(--text-secondary)]"
                  }`}
                >
                  {v.verdict}
                </span>
              </div>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text)]">{v.condition}</p>
              <p className="mt-1 font-mono text-[12px] leading-6 text-[var(--text-secondary)]">
                {v.architecture}
              </p>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">
                {v.detail}{" "}
                <a href={v.href} className="text-[var(--accent)] hover:underline">
                  Full analysis →
                </a>
              </p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="How To Read This As A Buyer" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Notice the pattern: for every cloud vendor, HIPAA is a <em>pricing tier</em>. That
            isn&rsquo;t cynicism — BAAs carry real legal exposure, and vendors charge for it. But
            it means the compliance question and the procurement question are the same question,
            and downgrades, lapsed contracts, or a clinician&rsquo;s personal account silently
            break compliance. If you go cloud, put the BAA condition in your renewal checklist.
          </p>
          <p>
            The alternative is to change the architecture instead of buying the contract: with
            on-device processing, the vendor never receives PHI, so there is no business
            associate and nothing to lapse. That&rsquo;s what{" "}
            <span className="font-medium text-[var(--text)]">Minutes</span> is — open source,
            free, capture-to-storage on your own machine. The trade-offs are real (no hosted team
            features, macOS-first) and the remaining duties are yours (encrypted disk, access
            control, patient consent). But the vendor-risk column of your HIPAA analysis goes to
            zero, permanently, on every &ldquo;plan.&rdquo;
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
            href="/resources/is-otter-ai-hipaa-compliant"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Start with the Otter analysis
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
          Informational, not legal advice. Vendor policies and plan gating change — verify
          against each vendor&rsquo;s current documentation and your compliance counsel before
          recording PHI with any tool.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
