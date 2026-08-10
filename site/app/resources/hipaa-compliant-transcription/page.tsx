import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "HIPAA-compliant transcription: what the rule actually requires",
  description:
    "No product is HIPAA certified, because HHS certifies none. What decides the vendor question is the business associate test in 45 CFR 160.103. The three transcription architectures, what a BAA does and does not settle, and the obligations that stay yours either way.",
  alternates: {
    canonical: "/resources/hipaa-compliant-transcription",
  },
};

const faqs = [
  {
    question: "Is there such a thing as HIPAA-certified transcription software?",
    answer:
      "Not in any official sense. HHS states that no standard requires a covered entity to certify compliance, that it does not recognize private Security Rule certifications, and that OCR does not endorse or certify specific products. A private certification can still be real evidence of diligence, but it carries no government recognition and does not prevent an OCR finding later. Treat the badge as a claim to examine, not a legal status.",
  },
  {
    question: "Does a transcription vendor need to sign a BAA?",
    answer:
      "Ordinarily yes, if it processes identifiable patient audio for you. Under 45 CFR 160.103 a business associate is a person who, on behalf of a covered entity and other than as a member of its workforce, creates, receives, maintains, or transmits PHI for a function or activity the rule regulates. A transcription service handling your patients' recordings normally meets that test, and 45 CFR 164.502(e) then requires the written contract before you disclose. The BAA is necessary, but it does not by itself make the disclosure permissible or complete your own risk analysis.",
  },
  {
    question: "Is on-device transcription HIPAA compliant?",
    answer:
      "Software is not compliant or non-compliant; a deployment is. What local processing can change is narrower: where the software vendor never receives, maintains, or transmits PHI, merely supplying that software does not make it a business associate, so there is no BAA to negotiate with them. That conclusion depends on the deployment actually being local-only. Enable a cloud summarizer, sync the folder to a hosted drive, or grant vendor support access to the files, and a party is receiving PHI again, which requires its own analysis and ordinarily a BAA.",
  },
  {
    question: "What is the difference between transcription services and transcription software?",
    answer:
      "Human transcription services use transcriptionists, can produce certified transcripts where a use requires one, and are ordinarily business associates. Cloud software sends audio to a vendor's servers, which ordinarily makes that vendor a business associate too. On-device software runs the model on your own machine, so in a genuinely local-only deployment, where no separate service receives or maintains the files, no third party receives the audio. Local inference by itself does not establish that: sync, backup, upload, and remote support paths all have to be absent too. All three architectures can be appropriate; they differ in who else ends up holding the PHI and what contracts that requires.",
  },
  {
    question: "Does the HIPAA conduit exception cover a transcription vendor?",
    answer:
      "Almost never. OCR reads the conduit concept narrowly, covering transmission-only services plus any storage that is temporary and incidental to transmission, and access that is transient or infrequent and necessary to that transmission. A service that transcribes your audio and stores the result persistently exceeds that, and OCR has said persistent storage defeats conduit status even where the provider cannot decrypt the data. Note also that a vendor which never meets the positive definition of business associate does not need an exception at all.",
  },
] as const;

const architectures = [
  {
    name: "Human transcription service",
    role: "Ordinarily a business associate",
    baa: "BAA required before PHI is disclosed",
    highlight: false,
    who: "The agency and its transcriptionists receive and store your audio and the finished transcript.",
    note: "The traditional model, and where a use requires a certified transcript this is generally the route to one. Ask who subcontracts, where staff are located, and whether the agency signs BAAs with its own vendors.",
  },
  {
    name: "Cloud AI transcription",
    role: "Ordinarily a business associate",
    baa: "BAA required before PHI is disclosed",
    highlight: false,
    who: "The vendor receives your audio, processes it on its servers, and usually stores the transcript.",
    note: "Fast and inexpensive, but you are adding a party that holds PHI. Check whether your plan is one the vendor will actually sign a BAA for, since several vendors offer that only on higher tiers; the plan decides whether the required contract is even available to you.",
  },
  {
    name: "On-device transcription",
    role: "No vendor in the path, if deployed local-only",
    baa: "No BAA with the software vendor, provided it never receives PHI",
    highlight: true,
    who: "The model runs on a machine you already control, and no third party receives the audio.",
    note: "Whether this holds is a fact about your deployment, not a property of the software. Any cloud summarizer, synced folder, hosted backup, or vendor support access puts PHI back in someone else's hands and needs its own analysis. The tradeoff is that the endpoint becomes the security surface, and there is no vendor contract to fall back on.",
  },
] as const;

const sources = [
  {
    label: "45 CFR 160.103 — definition of business associate (eCFR)",
    href: "https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-160/subpart-A/section-160.103",
  },
  {
    label: "45 CFR 164.502(e) — business associate contracts (eCFR)",
    href: "https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-164/subpart-E/section-164.502",
  },
  {
    label: "HHS FAQ: are we required to certify our organization's compliance with the standards?",
    href: "https://www.hhs.gov/hipaa/for-professionals/faq/2003/are-we-required-to-certify-our-organizations-compliance-with-the-standards/index.html",
  },
  {
    label: "HHS: business associate guidance",
    href: "https://www.hhs.gov/hipaa/for-professionals/privacy/guidance/business-associates/index.html",
  },
  {
    label: "OCR: guidance on HIPAA and cloud computing (conduit scope, storage, encryption)",
    href: "https://www.hhs.gov/hipaa/for-professionals/special-topics/health-information-technology/cloud-computing/index.html",
  },
  {
    label: "HHS FAQ: is the use of encryption mandatory in the Security Rule?",
    href: "https://www.hhs.gov/hipaa/for-professionals/faq/2001/is-the-use-of-encryption-mandatory-in-the-security-rule/index.html",
  },
  {
    label: "HHS: guidance on risk analysis requirements under the Security Rule",
    href: "https://www.hhs.gov/hipaa/for-professionals/security/guidance/guidance-risk-analysis/index.html",
  },
  {
    label: "HHS: breach notification guidance (unsecured PHI, risk assessment)",
    href: "https://www.hhs.gov/hipaa/for-professionals/breach-notification/guidance/index.html",
  },
  {
    label: "HHS FAQ: difference between consent and authorization",
    href: "https://www.hhs.gov/hipaa/for-professionals/faq/264/what-is-the-difference-between-consent-and-authorization/index.html",
  },
  { label: "Minutes security & privacy architecture", href: "/security" },
] as const;

export default function HipaaTranscriptionPage() {
  return (
    <div className="mx-auto max-w-[980px] px-6 pb-16 pt-10 sm:px-8 sm:pt-14">
      <script
        type="application/ld+json"
        dangerouslySetInnerHTML={{ __html: JSON.stringify(faqPageSchema(faqs)) }}
      />
      <div className="mb-10 flex items-center justify-between border-b border-[color:var(--border)] pb-4">
        <a href="/" className="font-mono text-[15px] font-medium text-[var(--text)]">
          minutes
        </a>
        <div className="flex gap-5 text-sm text-[var(--text-secondary)]">
          <a
            href="/resources/hipaa-compliant-transcription.md"
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
          HIPAA-compliant transcription: what the rule actually requires
        </h1>
        <p className="mt-5 text-[17px] leading-8 text-[var(--text-secondary)]">
          Every vendor in this category has a page telling you to look for encryption, SOC 2,
          and a signed BAA. That list is not wrong, but it skips the question underneath it:
          whether the vendor should be receiving your patients&rsquo; audio at all. Here is the
          test the rule actually applies, the three architectures it produces, and the
          obligations that stay yours no matter which you pick.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-08-10
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Sourced answer
          </span>
        </div>
        <p className="mt-6 text-[14px] leading-7 text-[var(--text-secondary)]">
          Scope: this concerns HIPAA covered entities and their business associates. Not every
          clinician is a covered entity, since that status also depends on conducting covered
          electronic transactions. If HIPAA does not apply to you, state law, professional
          ethics rules, and your own contracts still do.
        </p>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Two Things To Get Straight First" />
        <div className="space-y-5 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">
              There is no official HIPAA certification.
            </span>{" "}
            HHS says no standard requires a covered entity to certify its compliance, that it
            does not recognize private Security Rule certifications, and that OCR does not
            endorse or certify particular products. A private certification may still be
            meaningful evidence of diligence, and some represent substantial audits. What none
            of them carries is government recognition, and none prevents OCR finding a violation
            afterward. When a vendor leads with a HIPAA badge, the useful response is to ask
            what program issued it and what it examined.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              Compliance is a property of the arrangement, not the software.
            </span>{" "}
            No tool can be compliant on your behalf, because most of the obligations are about
            what your organization does: your risk analysis, who can access the files, how the
            endpoint is configured, whether staff are trained. The right question is never
            &ldquo;is this app HIPAA compliant.&rdquo; It is &ldquo;who ends up holding this
            audio, under what contract, and have I analyzed the risk of the setup I actually
            have.&rdquo;
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Test The Rule Applies" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Vendor selection follows from one definition. Under{" "}
            <span className="font-medium text-[var(--text)]">45 CFR 160.103</span>, a business
            associate is a person who,
          </p>
          <blockquote className="border-l-2 border-[color:var(--accent)] pl-5 text-[var(--text)]">
            &ldquo;On behalf of such covered entity&hellip; but other than in the capacity of a
            member of the workforce of such covered entity&hellip; creates, receives, maintains,
            or transmits protected health information for a function or activity regulated by
            this subchapter&hellip;&rdquo;
          </blockquote>
          <p>
            Every element matters, and reducing this to &ldquo;they received PHI&rdquo; is the
            most common way to get it wrong. The party must be acting{" "}
            <em>on your behalf</em>, must be outside your workforce, and the activity must be
            one the rule regulates. Someone can receive PHI and still not be a business
            associate, most obviously another provider receiving it for treatment. A separate
            branch of the definition covers enumerated professional services, such as legal or
            accounting work, where providing the service involves disclosure of PHI.
          </p>
          <p>
            Applied to this category the answer is usually straightforward: a transcription
            provider that processes identifiable patient recordings for you is ordinarily a
            business associate, and{" "}
            <span className="font-medium text-[var(--text)]">45 CFR 164.502(e)</span> then
            requires a written contract with satisfactory assurances{" "}
            <em>before</em> you disclose the recording.
          </p>
          <p>
            Notice what the definition does not turn on: encryption strength, data center
            location, or SOC 2. Those may be useful inputs to your risk management and your
            vendor diligence, but none of them determines business associate status, and two of
            them are not HIPAA requirements at all. What the Security Rule requires is
            reasonable and appropriate safeguards, with encryption treated as addressable rather
            than mandatory in every case.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Exclusions, And The Conduit Idea" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Paragraph (4) of the definition excludes four categories, each with conditions worth
            reading rather than paraphrasing loosely:
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              A health care provider, with respect to disclosures{" "}
              <em>by a covered entity to that provider</em> concerning the treatment of the
              individual.
            </li>
            <li>
              A plan sponsor, with respect to disclosures{" "}
              <em>
                by a group health plan, or by a health insurance issuer or HMO with respect to a
                group health plan
              </em>
              , to the plan sponsor, and only to the extent the requirements of &sect;
              164.504(f) apply and are met.
            </li>
            <li>
              A government agency, with respect to determining eligibility for or enrollment in
              a government health plan that provides public benefits and is administered by
              another government agency,{" "}
              <em>or collecting PHI for such purposes</em>, to the extent those activities are
              authorized by law.
            </li>
            <li>
              A covered entity participating in an organized health care arrangement that
              performs a function or activity described in paragraph (1)(i) for that
              arrangement, or provides a service described in paragraph (1)(ii) to or for it, by
              virtue of those activities or services.
            </li>
          </ul>
          <p>
            Software vendors are not on that list, and vendors sometimes present that as
            ominous. It is not, and the reason matters:{" "}
            <span className="font-medium text-[var(--text)]">
              paragraph (4) is not the only route to not being a business associate
            </span>
            . A vendor that never satisfies the positive definition, because it never creates,
            receives, maintains, or transmits PHI on your behalf, needs no exception at all. It
            simply is not one.
          </p>
          <p>
            The related &ldquo;conduit&rdquo; idea is narrower than its reputation. OCR treats
            it as covering transmission-only services, storage that is temporary and incidental
            to transmission, and access that is transient or infrequent and necessary to that
            transmission or required by law. A transcription service that stores your audio or
            transcripts persistently is outside it, and OCR has been explicit that persistent
            storage defeats conduit status even where the provider holds no decryption key.
          </p>
        </div>
      </section>

      <section className="mt-14">
        <SectionLabel label="The Three Architectures" />
        <div className="grid gap-4">
          {architectures.map((a) => (
            <div
              key={a.name}
              className="rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]"
            >
              <div className="flex flex-wrap items-center justify-between gap-3">
                <h3 className="font-serif text-[22px] text-[var(--text)]">{a.name}</h3>
                <span
                  className={`rounded-full px-3 py-1 font-mono text-[10px] uppercase tracking-[0.14em] ${
                    a.highlight
                      ? "bg-[var(--accent-soft)] text-[var(--accent)]"
                      : "bg-[var(--bg-hover)] text-[var(--text-secondary)]"
                  }`}
                >
                  {a.role}
                </span>
              </div>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{a.who}</p>
              <p className="mt-2 font-mono text-[12px] leading-6 text-[var(--text-secondary)]">
                {a.baa}
              </p>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{a.note}</p>
            </div>
          ))}
        </div>
        <p className="mt-6 max-w-[800px] text-[15px] leading-8 text-[var(--text-secondary)]">
          One caution that applies to the first two: a signed BAA is necessary when a vendor is
          a business associate, but it is not sufficient. The disclosure still has to be
          permissible, still has to satisfy minimum necessary where that applies, and your own
          risk analysis and safeguards remain your responsibility. The contract allocates
          obligations; it does not discharge yours.
        </p>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What On-Device Does Not Do" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            This is the part our own category oversells, so here it is plainly. Keeping
            processing local can remove one question, whether a software vendor receives PHI and
            therefore needs a BAA. It removes nothing else, and it creates work of its own.
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              <span className="font-medium text-[var(--text)]">
                You owe a fresh risk analysis.
              </span>{" "}
              The Security Rule requires an accurate and thorough assessment of risks to all
              ePHI you hold, and recording consultations creates a new corpus of audio and
              transcripts on an endpoint that may never have held a designated record set
              before. That is a change to analyze, not a step you skip because nothing was
              uploaded.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">Encryption is risk-based.</span>{" "}
              Encryption is an addressable implementation specification, so the question is what
              your risk analysis concludes and what you document, not a universal checkbox. In
              practice, full-disk encryption on a portable endpoint holding patient audio is
              very hard to reason your way out of.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                A lost laptop triggers an assessment, not automatic notification.
              </span>{" "}
              Breach notification concerns unsecured PHI and requires the regulatory risk
              assessment. PHI encrypted to the specified standard can fall outside the
              notification requirement entirely, which is the practical argument for encrypting
              before you need it.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                The rest of the Security Rule still applies.
              </span>{" "}
              Access control, audit controls, integrity, authentication, device and media
              controls, contingency planning and backup, and secure disposal all reach these
              files. How long you must keep the records themselves is a separate question: the
              Security Rule&rsquo;s six-year rule governs required HIPAA documentation, while
              medical record retention periods generally come from other federal or state law.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                Privacy Rule duties follow the record.
              </span>{" "}
              If a transcript becomes part of a designated record set, patient access and
              amendment rights attach to it.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                Recording consent is a separate question.
              </span>{" "}
              State wiretap and recording law, and professional ethics rules, apply regardless
              of where processing happens, and they are not the same thing as a HIPAA
              authorization. HIPAA permits many treatment, payment, and operations uses without
              individual authorization; authorization is required where the Privacy Rule does
              not otherwise permit the use or disclosure.
            </li>
            <li>
              <span className="font-medium text-[var(--text)]">
                You lose a party to hold accountable.
              </span>{" "}
              A business associate under contract carries obligations and liability. Your own
              endpoint carries none.
            </li>
          </ul>
          <p>
            The honest summary: local processing converts a vendor-disclosure problem into an
            endpoint problem. That can be a very good trade, particularly where the endpoint is
            already managed to the standard your other clinical systems require. It is a trade,
            not an exemption, and an unmanaged personal laptop is a materially worse place for
            this corpus than a managed workstation.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Where Minutes Fits, And Where It Does Not" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Minutes</span> is built for the
            third architecture. It records on your device, transcribes locally with whisper.cpp,
            diarizes with local models, and writes markdown into a folder you control. In a
            local-only deployment, we do not receive, maintain, or transmit your PHI, and
            supplying software to someone who uses it that way does not by itself create a
            business associate relationship.
          </p>
          <p>
            That conclusion is conditional on your deployment, and it is worth being blunt about
            what breaks it. Configure a provider-backed summarizer, which is off by default, and
            transcript text goes to whichever model provider you chose, whose terms then govern.
            Connect an AI agent over MCP and ask it to read your meetings, and what it reads
            travels to that agent's provider as context.
            Sync your meetings folder to a hosted drive and that host is now storing PHI. Grant
            anyone remote access to the machine and the same applies. None of those are exotic;
            they are ordinary choices that change the analysis, and each needs its own review
            and ordinarily a BAA with that party. Our{" "}
            <a href="/security" className="text-[var(--accent)] hover:underline">
              security page
            </a>{" "}
            enumerates every case where bytes touch the network.
          </p>
          <p className="font-medium text-[var(--text)]">Where it is the wrong tool:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              You need a certified transcript for a filing or proceeding. Requirements vary by
              jurisdiction and use, and that is generally a human service.
            </li>
            <li>
              You want an ambient clinical scribe that writes structured notes into your EHR,
              suggests codes, or drafts to a SOAP template. Minutes is not a medical scribe and
              has no EHR integration.
            </li>
            <li>
              You need centralized audit logs and administrative oversight across a practice.
              Local files give you ownership, not governance.
            </li>
            <li>
              Your posture depends on having a business associate to hold accountable. Sometimes
              that contract is precisely the point.
            </li>
            <li>
              The endpoint is an unmanaged personal device. Then you have moved the risk rather
              than reduced it.
            </li>
          </ul>
          <p>
            If you are comparing named products rather than architectures, we keep a sourced
            vendor-by-vendor breakdown of{" "}
            <a
              href="/resources/hipaa-compliant-ai-note-taker"
              className="text-[var(--accent)] hover:underline"
            >
              which AI note takers can be used with PHI, and on which plan tier
            </a>
            .
          </p>
        </div>
      </section>

      <FaqSection items={faqs} />

      <section className="mt-14 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 shadow-[var(--shadow-panel)]">
        <p className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent)]">
          Next step
        </p>
        <div className="mt-4 flex flex-wrap gap-3">
          <a
            href="/security"
            className="inline-flex items-center rounded-[5px] bg-[var(--accent)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-black hover:bg-[var(--accent-hover)]"
          >
            What touches the network
          </a>
          <a
            href="/resources/hipaa-compliant-ai-note-taker"
            className="inline-flex items-center rounded-[5px] border border-[color:var(--border-mid)] px-5 py-2.5 font-mono text-[11px] uppercase tracking-[0.12em] text-[var(--text)] hover:bg-[var(--bg-hover)]"
          >
            Vendor-by-vendor breakdown
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
        <p className="mt-6 text-[13px] leading-7 text-[var(--text-secondary)]">
          Informational, not legal advice. HIPAA analysis is fact-specific and turns on your
          actual deployment; covered-entity status, permitted uses, and state recording law all
          vary. Vendor terms and plan gating change. Your own counsel or compliance officer is
          the one who signs off, and this page is a starting point for that conversation rather
          than a substitute for it.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
