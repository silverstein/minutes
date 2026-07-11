import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Is Fireflies.ai HIPAA compliant?",
  description:
    "Yes — but only on the Enterprise plan with Private Storage enabled AND a signed BAA, all three at once. Free, Pro, and Business plans are not HIPAA-covered. The sourced answer, plus the on-device alternative that removes the BAA question.",
  alternates: {
    canonical: "/resources/is-fireflies-ai-hipaa-compliant",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Is Fireflies.ai HIPAA compliant?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Conditionally. Per Fireflies' own documentation, HIPAA compliance requires three things simultaneously: an active Enterprise plan, Private Storage enabled, and a signed BAA. If any one lapses, compliance is disabled. Free, Pro, and Business plans cannot be used for PHI in a compliant way.",
      },
    },
    {
      "@type": "Question",
      name: "Does Fireflies sign a BAA?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Yes, via a self-serve BAA page (fireflies.ai/baa) — but the BAA only takes effect with Private Storage enabled, and the HIPAA configuration is Enterprise-only ($39/user/month, billed annually).",
      },
    },
    {
      "@type": "Question",
      name: "Where does Fireflies process and store meeting audio?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "In Fireflies' cloud in the United States (AWS and GCP) by default, with transcription and AI passing through vendors including OpenAI under zero-retention BAAs. Enterprise Private Storage allows dedicated or bring-your-own storage. Nothing runs on the user's device.",
      },
    },
    {
      "@type": "Question",
      name: "Is there a meeting notetaker that doesn't need a BAA at all?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Yes — on-device tools. A BAA exists to govern a vendor's handling of PHI; software that transcribes and stores entirely on your own machine (like Minutes, open source) never gives a vendor the data, so no business associate relationship is created. Device security and consent obligations remain yours.",
      },
    },
  ],
} as const;

const sources = [
  { label: "Fireflies security", href: "https://fireflies.ai/security" },
  { label: "Fireflies HIPAA page", href: "https://fireflies.ai/hipaa" },
  { label: "Fireflies self-serve BAA", href: "https://fireflies.ai/baa" },
  {
    label: "Fireflies guide: set up HIPAA compliance for your workspace",
    href: "https://guide.fireflies.ai/articles/3704059205-set-up-hipaa-compliance-for-your-workspace",
  },
  {
    label: "Fireflies guide: data storage and transfer",
    href: "https://guide.fireflies.ai/articles/9596505232-learn-about-data-storage-and-transfer",
  },
  { label: "Fireflies pricing", href: "https://fireflies.ai/pricing" },
  { label: "Minutes security & privacy architecture", href: "/security" },
  { label: "Is Otter.ai HIPAA compliant?", href: "/resources/is-otter-ai-hipaa-compliant" },
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

export default function IsFirefliesHipaaCompliantPage() {
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
            href="/resources/is-fireflies-ai-hipaa-compliant.md"
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
          Is Fireflies.ai HIPAA compliant?
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Yes — with three conditions that must all hold at once, and a price tier attached.
          Fireflies documents this more precisely than most vendors, so the answer is knowable;
          it&rsquo;s just longer than the marketing headline. Here it is, sourced to
          Fireflies&rsquo; own documentation, followed by the architectural question the
          checklist implies.
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
            <span className="font-medium text-[var(--text)]">
              Fireflies can be HIPAA-compliant, but only in one configuration:
            </span>{" "}
            an active Enterprise plan ($39/user/month, billed annually) <em>plus</em> Private
            Storage enabled <em>plus</em> a signed BAA — all three simultaneously. Fireflies&rsquo;
            own setup guide states compliance is disabled if any single requirement is removed,
            downgraded, or expires.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              On Free, Pro, and Business plans, the answer is no.
            </span>{" "}
            Recording PHI on those tiers is a disclosure to a vendor with no BAA in effect —
            regardless of Fireflies&rsquo; generally strong security posture (SOC 2 Type II,
            zero-retention agreements with its AI vendors).
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What The Compliant Configuration Involves" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Credit where due: Fireflies publishes a self-serve BAA page and documents the
            requirements plainly. The compliant setup means Enterprise pricing, enabling Private
            Storage (dedicated or bring-your-own AWS S3/GCS, with EU region options), signing the
            BAA, and keeping all three alive — your workspace&rsquo;s security checklist shows
            &ldquo;HIPAA Compliance: Enabled&rdquo; when configured. Fireflies also states it has
            signed BAAs downstream with OpenAI and its speech-recognition vendors, with
            zero-retention terms.
          </p>
          <p>
            What the configuration doesn&rsquo;t change: your patients&rsquo; conversations are
            still processed in Fireflies&rsquo; cloud (US-based AWS/GCP by default) and still
            pass through third-party AI vendors. The BAA chain makes those disclosures lawful and
            governed. It does not make them not-disclosures — every link in that chain is a party
            holding or touching PHI, bound by contract rather than removed from the picture.
          </p>
          <p>
            One nuance worth flagging: Fireflies&rsquo; healthcare marketing describes features
            &ldquo;available to all Fireflies customers,&rdquo; while the compliance docs gate
            the HIPAA configuration to Enterprise. The features are broad; the compliance is
            not. Read the setup guide, not the press release.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Question Under The Question" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            A three-condition compliance checklist exists because the audio leaves your machine.
            The alternative architecture makes the checklist disappear: transcription that runs
            entirely on your own device never gives any vendor the conversation, so there is no
            business associate, no BAA, no Private Storage tier, and no configuration to keep
            alive. That&rsquo;s <span className="font-medium text-[var(--text)]">Minutes</span> —
            open source, on-device transcription and diarization, markdown on your own disk with
            owner-only permissions. No local tool is &ldquo;HIPAA certified&rdquo; (no tool of
            any kind is — HHS certifies nobody); on-device processing simply removes the vendor
            from the analysis. Device encryption, access control, and recording consent remain
            your responsibilities, as they already are for the machine your EHR runs on.
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
            href="/compare/fireflies-vs-minutes"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Minutes vs Fireflies
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
          This page is informational, not legal advice. Verify plan details, Private Storage
          setup, and BAA terms with Fireflies and your compliance counsel before recording PHI
          with any tool.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
