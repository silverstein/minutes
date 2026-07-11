import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Is Otter.ai HIPAA compliant?",
  description:
    "Yes — but only on the Enterprise plan with a signed BAA, as of July 2025. Basic, Pro, and Business plans cannot be used for PHI. What that means, and the on-device alternative that removes the BAA question entirely.",
  alternates: {
    canonical: "/resources/is-otter-ai-hipaa-compliant",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Is Otter.ai HIPAA compliant?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Conditionally. Otter.ai announced HIPAA compliance in July 2025, but per its help center it is only available to Enterprise plan customers who sign a Business Associate Agreement (BAA). Users on Basic, Pro, or Business plans cannot obtain a BAA and cannot use Otter for protected health information in a compliant way.",
      },
    },
    {
      "@type": "Question",
      name: "Does Otter.ai sign a Business Associate Agreement (BAA)?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Yes, for Enterprise plan customers only, through its sales and account management process. Without a signed BAA in place, Otter is not acting as a HIPAA business associate.",
      },
    },
    {
      "@type": "Question",
      name: "Can I use Otter's free, Pro, or Business plan for patient conversations?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "No. Per Otter's help center, HIPAA compliance is restricted to the Enterprise plan with a signed BAA. Recording conversations containing PHI on other plans would be an impermissible disclosure to a vendor with no BAA.",
      },
    },
    {
      "@type": "Question",
      name: "Do on-device transcription tools need a BAA?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "No. A BAA is required when a vendor receives, stores, or processes PHI on your behalf. Software that transcribes entirely on your own device — such as open-source tools built on whisper.cpp, like Minutes — never transmits the conversation to a vendor, so there is no business associate relationship to paper over. Your own HIPAA obligations (device encryption, access controls) still apply.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "Otter.ai Help Center: HIPAA",
    href: "https://help.otter.ai/hc/en-us/articles/33975072019991-HIPAA-Otter-ai",
  },
  {
    label: "Otter.ai blog: Otter.ai Achieves HIPAA Compliance",
    href: "https://otter.ai/blog/otter-ai-achieves-hipaa-compliance",
  },
  {
    label: "HHS: Business Associate Contracts",
    href: "https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html",
  },
  { label: "Minutes security & privacy architecture", href: "https://useminutes.app/security" },
  { label: "Minutes on GitHub", href: "https://github.com/silverstein/minutes" },
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

export default function IsOtterAiHipaaCompliantPage() {
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
          <a href="/resources/is-otter-ai-hipaa-compliant.md" className="hover:text-[var(--accent)]">
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
          Is Otter.ai HIPAA compliant?
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Short answer: yes, since July 2025 — but only on the Enterprise plan, and only with a
          signed Business Associate Agreement. Most answers you&rsquo;ll find online are out of
          date in one direction or the other. Here is the current state, sourced from
          Otter&rsquo;s own documentation, and the architectural question worth asking before you
          buy your way out of the problem.
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
              Otter.ai is HIPAA compliant only for Enterprise customers with a signed BAA.
            </span>{" "}
            Otter announced HIPAA compliance in July 2025 following an independent assessment. Per
            its help center, the BAA is available exclusively on the Enterprise plan, arranged
            through sales.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              On Basic, Pro, and Business plans, the answer is no.
            </span>{" "}
            No BAA is available on those tiers, so recording conversations that contain protected
            health information on them is an impermissible disclosure to a vendor — regardless of
            how good Otter&rsquo;s general security is.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What A BAA Does — And What It Doesn't" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            A Business Associate Agreement makes it lawful for a vendor to receive and process PHI
            on your behalf, and binds the vendor to HIPAA&rsquo;s safeguards. It is a legal
            instrument, not an architectural one. With a signed BAA, your patient conversations
            still travel to Otter&rsquo;s cloud, are still transcribed on Otter&rsquo;s servers,
            and still live in Otter&rsquo;s storage — the disclosure is permitted and governed,
            not eliminated.
          </p>
          <p>
            That distinction matters when you think about breach surface. A BAA obligates the
            vendor to report breaches; it does not make the vendor unbreachable. Every cloud
            notetaker with a BAA is still a third party holding your patients&rsquo; conversations.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Checklist If You Stay With Otter" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>Using Otter with PHI in a compliant way requires all of the following:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>An Enterprise plan — not Basic, Pro, or Business.</li>
            <li>A BAA actually signed with Otter before any PHI is recorded, not after.</li>
            <li>
              Workspace policies that keep PHI recordings inside the covered workspace — a
              clinician&rsquo;s personal Pro account doesn&rsquo;t inherit the organization&rsquo;s
              BAA.
            </li>
            <li>
              Your own HIPAA obligations: consent workflows for recording, access controls, and
              minimum-necessary practices don&rsquo;t transfer to the vendor.
            </li>
          </ul>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Question Under The Question" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            The reason a BAA is needed at all is that the audio leaves your machine. There is a
            second way to resolve the HIPAA question: use transcription that runs entirely on your
            own device, so no vendor ever receives the conversation. No third party, no business
            associate, no BAA to negotiate — the legal question dissolves because the disclosure
            never happens.
          </p>
          <p>
            That&rsquo;s the architecture{" "}
            <span className="font-medium text-[var(--text)]">Minutes</span> is built on: audio is
            captured and transcribed on your device with whisper.cpp, and the record is markdown on
            your own disk with owner-only file permissions. It&rsquo;s open source, so this claim
            is verifiable in code rather than asserted on a trust page. To be precise about the
            framing: no local tool is &ldquo;HIPAA certified&rdquo; — HIPAA governs covered
            entities and their vendors. On-device processing removes the vendor from the equation;
            securing the device (disk encryption, access control) remains your responsibility, as
            it already is for your EHR workstation.
          </p>
          <p>
            If your compliance team&rsquo;s core question is &ldquo;where does the audio go?&rdquo;,
            the on-device answer is one sentence long.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="Which Fits Your Situation" />
        <div className="overflow-x-auto rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] shadow-[var(--shadow-panel)]">
          <table className="min-w-full border-collapse text-left">
            <thead>
              <tr className="border-b border-[color:var(--border)]">
                <th className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Your situation
                </th>
                <th className="px-4 py-3 font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--text-secondary)]">
                  Reasonable path
                </th>
              </tr>
            </thead>
            <tbody>
              <tr className="border-b border-[color:var(--border)]">
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text)]">
                  Large org, already on Otter Enterprise, wants team features
                </td>
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text-secondary)]">
                  Sign the BAA, lock recording to the covered workspace, audit regularly
                </td>
              </tr>
              <tr className="border-b border-[color:var(--border)]">
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text)]">
                  Solo practitioner or small practice on a consumer/Pro plan
                </td>
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text-secondary)]">
                  Stop recording PHI with it today — no BAA is available at your tier. Consider
                  on-device transcription instead
                </td>
              </tr>
              <tr>
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text)]">
                  Any practice whose bar is &ldquo;patient audio never touches a third party&rdquo;
                </td>
                <td className="px-4 py-4 align-top text-[14px] leading-7 text-[var(--text-secondary)]">
                  On-device tools are the only architecture that meets it — a BAA governs
                  disclosure, it doesn&rsquo;t prevent it
                </td>
              </tr>
            </tbody>
          </table>
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
            href="/compare/otter-vs-minutes"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Minutes vs Otter
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
          This page is informational, not legal advice. Verify plan details and BAA terms with
          Otter and your compliance counsel before recording PHI with any tool.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
