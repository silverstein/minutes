import type { Metadata } from "next";
import { PublicFooter } from "@/components/public-footer";

export const metadata: Metadata = {
  title: "Is it legal to record a meeting? Consent law, explained",
  description:
    "One-party vs all-party consent states, what changes when an AI notetaker does the recording, workplace and cross-state rules, and the consent script that keeps you clear everywhere. Sourced, with a state-law reference.",
  alternates: {
    canonical: "/resources/is-it-legal-to-record-a-meeting",
  },
};

const faqJsonLd = {
  "@context": "https://schema.org",
  "@type": "FAQPage",
  mainEntity: [
    {
      "@type": "Question",
      name: "Is it legal to record a meeting you're part of?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "In most US states, yes — federal law and roughly three dozen states require only one party's consent, and as a participant you are that party. But eleven-plus states, including California, Florida, Illinois, Massachusetts, Pennsylvania, and Washington, require all parties' consent for confidential communications. Cross-state calls should follow the strictest applicable rule.",
      },
    },
    {
      "@type": "Question",
      name: "Does using an AI notetaker change recording-consent law?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "The consent rules are the same — a recording is a recording whether a human presses the button or software does. What changes is disclosure mechanics: bot notetakers announce themselves as visible participants, while device-side capture tools are silent, so the duty to inform participants falls entirely on you. Some vendors also display recording notices; that supplements but does not replace your consent obligation in all-party states.",
      },
    },
    {
      "@type": "Question",
      name: "What is the safest practice for recording meetings?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Announce and get affirmative acknowledgment at the start of every recorded meeting, regardless of state: 'I'd like to record this for notes — everyone okay with that?' It satisfies all-party states, is professionally courteous everywhere, and creates a consent record on the recording itself.",
      },
    },
    {
      "@type": "Question",
      name: "Can my employer record meetings without telling me?",
      acceptedAnswer: {
        "@type": "Answer",
        text: "Workplace rules layer on top of state law: one-party states give employers more room, all-party states don't. Many employers handle consent through policy and meeting-platform notices. Union agreements, sector rules, and jurisdictions like the EU (GDPR) add further constraints. Check policy and local law rather than assuming.",
      },
    },
  ],
} as const;

const sources = [
  {
    label: "RCFP Reporter's Recording Guide — state-by-state consent law",
    href: "https://www.rcfp.org/reporters-recording-guide/",
  },
  {
    label: "Justia: Recording Phone Calls and Conversations — 50-state survey",
    href: "https://www.justia.com/50-state-surveys/recording-phone-calls-and-conversations/",
  },
  {
    label: "18 U.S.C. § 2511(2)(d) — federal one-party consent",
    href: "https://www.law.cornell.edu/uscode/text/18/2511",
  },
  {
    label: "AI notetakers and attorney–client privilege — our analysis",
    href: "/resources/ai-notetakers-attorney-client-privilege",
  },
  {
    label: "How to remove AI notetaker bots from meetings",
    href: "/resources/remove-ai-notetaker-bots-from-meetings",
  },
  { label: "Minutes consent-in-frontmatter design", href: "/writing/governance-built-in-not-retrofitted" },
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

export default function IsItLegalToRecordAMeetingPage() {
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
            href="/resources/is-it-legal-to-record-a-meeting.md"
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
          Is it legal to record a meeting?
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Usually yes, if you&rsquo;re in it — but the exceptions are exactly the states where a
          lot of business happens, and AI notetakers have made the question urgent for people
          who never thought about wiretap law before. Here&rsquo;s the map, what changes when
          software does the recording, and the one habit that keeps you clear everywhere.
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
              Federal law and most states: one-party consent.
            </span>{" "}
            If you&rsquo;re a participant, you are the one party — you can record without asking
            (18 U.S.C. § 2511(2)(d), absent criminal or tortious purpose).
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              Eleven-plus states: all-party consent
            </span>{" "}
            for confidential communications — including California, Florida, Illinois, Maryland,
            Massachusetts, Montana, Nevada (as interpreted), New Hampshire, Pennsylvania, and
            Washington. On a call spanning states, follow the strictest rule in the room. The
            RCFP guide linked below is the current state-by-state reference.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What AI Notetakers Change — And Don't" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            The law doesn&rsquo;t care whether a human or software presses record. What changes
            is the <em>disclosure mechanics</em>. A bot notetaker announces itself — it sits in
            the participant list as &ldquo;Otter Notetaker&rdquo; or &ldquo;Fireflies.ai
            Notetaker,&rdquo; which is a form of notice (though silence is not consent in
            all-party states). Device-side tools — Granola, Krisp&rsquo;s botless mode, and our
            own Minutes — record without any visible artifact in the meeting. That&rsquo;s
            better product design and worse automatic disclosure: the duty to inform lands
            entirely on you.
          </p>
          <p>
            We build a botless tool, so let&rsquo;s be unambiguous about our own product:
            botless capture is not a mechanism for recording people without their knowledge,
            and using it that way in an all-party state is illegal. It&rsquo;s why Minutes&rsquo;
            recording frontmatter carries a consent field — so the record itself can document
            how consent was obtained.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Habit That Solves It Everywhere" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Open every recorded meeting with one sentence and a pause:{" "}
            <span className="font-medium text-[var(--text)]">
              &ldquo;I&rsquo;d like to record this for notes — everyone okay with that?&rdquo;
            </span>{" "}
            Affirmative answers satisfy the strictest all-party state, the acknowledgment lives
            on the recording itself, and — the underrated part — it&rsquo;s just good manners.
            Most consent-law trouble in business contexts starts as a courtesy failure before it
            becomes a legal one.
          </p>
          <p>
            Layered on top: workplace policy (employers often standardize notice via meeting
            platforms), sector rules (health, finance, education), and non-US law — the EU
            treats recordings as personal-data processing under GDPR, which is a consent-or-
            legal-basis analysis, not a one-party/all-party one. When in doubt, announce and
            ask; when the stakes are real, ask counsel.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="One More Distinction Worth Having" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Consent to record and consent to <em>upload</em> are different things, and
            participants increasingly know it. &ldquo;Okay if I record?&rdquo; quietly became
            &ldquo;okay if this conversation goes to a transcription vendor, an LLM provider,
            and a US cloud?&rdquo; when cloud notetakers took over. With on-device tools the two
            questions collapse back into one — the recording exists only on the machine of the
            person who asked. Several people have told us that&rsquo;s made the consent
            conversation itself easier: &ldquo;it stays on my laptop&rdquo; is a sentence
            everyone in the room can evaluate.
          </p>
        </div>
      </section>

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/writing/governance-built-in-not-retrofitted"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            How Minutes records consent
          </a>
          <a
            href="/resources/ai-notetakers-attorney-client-privilege"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            The privilege analysis
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
          Informational, not legal advice. Consent law is state- and fact-specific and changes;
          the RCFP guide is the reference to check, and counsel is the answer when it matters.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
