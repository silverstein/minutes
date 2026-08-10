import type { Metadata } from "next";
import { FaqSection } from "@/components/faq-section";
import { PublicFooter } from "@/components/public-footer";
import { SectionLabel } from "@/components/section-label";
import { faqPageSchema } from "@/lib/schema";

export const metadata: Metadata = {
  title: "HIPAA-compliant transcription: what the rule actually requires",
  description:
    "No product is HIPAA certified, because HHS certifies nothing. What the rule turns on is whether a vendor receives PHI at all. The three transcription architectures, what a BAA does and does not buy, and the obligations that stay yours either way.",
  alternates: {
    canonical: "/resources/hipaa-compliant-transcription",
  },
};

const faqs = [
  {
    question: "Is there such a thing as HIPAA-certified transcription software?",
    answer:
      "No. HHS does not certify or endorse any product, service, or vendor as HIPAA compliant, and no standard requires a covered entity to obtain certification. A vendor advertising itself as \"HIPAA certified\" is describing a private audit it purchased, which carries no government recognition and does not shield you from an OCR finding. What matters is whether the arrangement satisfies the rule, not what badge is on the marketing page.",
  },
  {
    question: "Does a transcription service need to sign a BAA?",
    answer:
      "If it receives protected health information on your behalf, yes. 45 CFR 160.103 defines a business associate as a person who creates, receives, maintains, or transmits PHI on behalf of a covered entity, and the Privacy Rule requires satisfactory assurances in a written contract before you disclose PHI to them. Encryption does not substitute for the contract, and the contract does not substitute for the safeguards.",
  },
  {
    question: "Is on-device transcription HIPAA compliant?",
    answer:
      "That question is aimed at the wrong object. Software is not compliant or non-compliant; an arrangement is. What on-device processing changes is narrower and more useful: if the vendor never receives, maintains, or transmits PHI, it does not meet the definition of a business associate, so there is no BAA to negotiate and no vendor breach exposure to inherit. Every obligation you already have (device encryption, access control, workforce training, patient authorization where required, breach notification) is unchanged.",
  },
  {
    question: "What is the difference between transcription services and transcription software?",
    answer:
      "Services use human transcriptionists and can produce certified transcripts; the transcriptionist and the agency are business associates and need a BAA. Cloud software sends your audio to a vendor's servers, which also makes that vendor a business associate. On-device software runs the model on your own machine, so no third party receives the audio at all. All three can be appropriate; they differ in who else ends up holding the PHI.",
  },
  {
    question: "Does the HIPAA conduit exception cover a transcription vendor?",
    answer:
      "Almost never. 45 CFR 160.103 lists only four exclusions from the business associate definition, covering health care providers receiving treatment disclosures, plan sponsors, government agencies determining eligibility, and covered entities in an organized health care arrangement. There is no exception for software vendors or storage providers. OCR reads the conduit concept narrowly, as mere transmission without access to content, which a service that transcribes and stores your audio plainly exceeds.",
  },
] as const;

const architectures = [
  {
    name: "Human transcription service",
    role: "Business associate",
    needsBaa: true,
    who: "The agency and its transcriptionists receive and store your audio and the finished transcript.",
    note: "The traditional model, and still the one that produces certified transcripts. Ask who subcontracts, where staff are located, and whether the agency signs BAAs with its own vendors.",
  },
  {
    name: "Cloud AI transcription",
    role: "Business associate",
    needsBaa: true,
    who: "The vendor receives your audio, processes it on its servers, and usually stores the transcript.",
    note: "Fast and cheap, but you are adding a party that holds PHI. Check the plan tier: most vendors gate BAAs to an enterprise plan, so the same product is appropriate on one tier and impermissible on another.",
  },
  {
    name: "On-device transcription",
    role: "Not a business associate",
    needsBaa: false,
    who: "Nobody outside your organization receives the audio. The model runs on the machine you already control.",
    note: "There is no BAA because there is no disclosure to contract about. The tradeoff is that the device becomes the whole security surface, and you get no vendor to hold accountable if you misconfigure it.",
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
    label: "HHS: sample business associate agreement provisions",
    href: "https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html",
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
          rule as written, the three architectures it produces, and the obligations that stay
          yours no matter which you pick.
        </p>
        <div className="mt-6 flex flex-wrap gap-3">
          <span className="rounded-full bg-[var(--bg-elevated)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--text-secondary)]">
            Last reviewed: 2026-08-10
          </span>
          <span className="rounded-full bg-[var(--accent-soft)] px-3 py-1 font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--accent)]">
            Sourced answer
          </span>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Two Things To Get Straight First" />
        <div className="space-y-5 rounded-[8px] border border-[color:var(--border)] bg-[var(--bg-elevated)] p-6 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">
              Nothing is &ldquo;HIPAA certified.&rdquo;
            </span>{" "}
            HHS certifies no product, service, or vendor, and no standard requires a covered
            entity to obtain certification. A private audit can be genuinely useful evidence of
            diligence, but it carries no government recognition and does not stop OCR finding a
            violation afterward. When a transcription vendor leads with a HIPAA badge, you are
            looking at marketing, not a legal status.
          </p>
          <p>
            <span className="font-medium text-[var(--text)]">
              Compliance is a property of the arrangement, not the software.
            </span>{" "}
            No tool can be compliant on your behalf, because most of the obligations are about
            what your organization does: who can access the files, whether the disk is
            encrypted, whether staff are trained, whether the patient authorized the disclosure.
            The right question is never &ldquo;is this app HIPAA compliant.&rdquo; It is
            &ldquo;who ends up holding this audio, and under what contract.&rdquo;
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="The Rule As Written" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            Everything about vendor selection follows from one definition. Under{" "}
            <span className="font-medium text-[var(--text)]">45 CFR 160.103</span>, a business
            associate is a person who, on behalf of a covered entity,
          </p>
          <blockquote className="border-l-2 border-[color:var(--accent)] pl-5 text-[var(--text)]">
            &ldquo;creates, receives, maintains, or transmits protected health information for a
            function or activity regulated by this subchapter&hellip;&rdquo;
          </blockquote>
          <p>
            Read those four verbs carefully, because the entire vendor question is decided by
            them. A transcription provider that takes in your audio{" "}
            <em>receives</em> PHI. One that keeps the transcript on its servers{" "}
            <em>maintains</em> it. Either verb makes the provider a business associate, and the
            Privacy Rule then requires a written contract with satisfactory assurances{" "}
            <em>before</em> you hand over the recording.
          </p>
          <p>
            Note what the definition does not say. It does not turn on how good the encryption
            is, where the data center sits, or whether the vendor has SOC 2. Those are
            safeguards that a business associate must have; they are not what makes someone a
            business associate. The trigger is receipt of PHI.
          </p>
          <p>
            One more thing worth knowing, because vendors reach for it:{" "}
            <span className="font-medium text-[var(--text)]">
              there is no software exception
            </span>
            . Paragraph (4) of the definition excludes exactly four categories, being health
            care providers receiving treatment disclosures, plan sponsors, government agencies
            determining eligibility, and covered entities within an organized health care
            arrangement. Software vendors, cloud providers, and transcription platforms appear
            nowhere on that list. The narrow &ldquo;conduit&rdquo; idea covers mere transmission
            without access to content, which a service that transcribes and stores your audio
            plainly exceeds.
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
                    a.needsBaa
                      ? "bg-[var(--bg-hover)] text-[var(--text-secondary)]"
                      : "bg-[var(--accent-soft)] text-[var(--accent)]"
                  }`}
                >
                  {a.role}
                </span>
              </div>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{a.who}</p>
              <p className="mt-2 font-mono text-[12px] leading-6 text-[var(--text-secondary)]">
                {a.needsBaa ? "BAA required before any PHI is disclosed" : "No BAA — no disclosure to contract about"}
              </p>
              <p className="mt-3 text-[15px] leading-8 text-[var(--text-secondary)]">{a.note}</p>
            </div>
          ))}
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="What On-Device Does Not Do" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            This is the part our own category tends to oversell, so here it is plainly. Running
            transcription on your own machine removes one question: whether a third party
            receives PHI, and therefore whether you need a BAA and inherit that vendor&rsquo;s
            breach exposure. It removes nothing else.
          </p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              You still owe the Security Rule&rsquo;s safeguards. Full-disk encryption, screen
              lock, access control, audit, and a real answer for a stolen laptop.
            </li>
            <li>
              You still owe workforce training and sanctions, and you still owe breach
              notification if the device is lost.
            </li>
            <li>
              Consent and authorization rules are untouched. Recording a patient encounter is
              subject to the same state law and the same professional obligations either way.
            </li>
            <li>
              Local files are still discoverable, and a subpoena reaches your disk as readily as
              a vendor&rsquo;s.
            </li>
            <li>
              You lose a party to hold accountable. A cloud vendor under a BAA carries
              contractual obligations; your laptop carries none.
            </li>
          </ul>
          <p>
            The honest summary: on-device processing converts a vendor-disclosure problem into a
            device-security problem. For many clinicians that is a very good trade, because the
            device-security problem is one they already solved for their EHR workstation. It is
            still a trade, not an exemption.
          </p>
        </div>
      </section>

      <section className="mt-14 max-w-[800px]">
        <SectionLabel label="Where Minutes Fits, And Where It Does Not" />
        <div className="space-y-4 text-[15px] leading-8 text-[var(--text-secondary)]">
          <p>
            <span className="font-medium text-[var(--text)]">Minutes</span> is the third
            architecture. It records on your device, transcribes locally with whisper.cpp,
            diarizes speakers with local models, and writes markdown to a folder you control. No
            audio is uploaded, so no vendor receives PHI, so there is no BAA to negotiate with
            us. We are not a business associate because we are never in the path.
          </p>
          <p>
            To be exact about the one carve-out: transcript text leaves your machine only if you
            deliberately configure a provider-backed summarizer, which is off by default. Point
            it at a cloud model and you have re-created the disclosure this architecture avoids,
            and that provider&rsquo;s terms then govern. Our{" "}
            <a href="/security" className="text-[var(--accent)] hover:underline">
              security page
            </a>{" "}
            lists every case where bytes touch the network.
          </p>
          <p className="font-medium text-[var(--text)]">Where it is the wrong tool:</p>
          <ul className="list-disc space-y-2 pl-6">
            <li>
              You need a <span className="font-medium text-[var(--text)]">certified</span>{" "}
              transcript for a legal or regulatory filing. That is a human service.
            </li>
            <li>
              You want an ambient clinical scribe that writes structured notes into your EHR,
              suggests codes, or drafts to a SOAP template. Minutes is not a medical scribe and
              has no EHR integration.
            </li>
            <li>
              You need vendor-side audit logs and administrative oversight across a practice.
              Local files give you ownership, not centralized governance.
            </li>
            <li>
              Your compliance posture depends on having a business associate to hold
              accountable. Sometimes that contract is the point.
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
          Informational, not legal advice. HIPAA analysis is fact-specific, vendor terms and
          plan gating change, and your own counsel or compliance officer is the one who signs
          off. Verify against the regulation and the vendor&rsquo;s current documentation before
          relying on any of it.
        </p>
      </section>

      <PublicFooter />
    </div>
  );
}
