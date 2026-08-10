# HIPAA-compliant transcription: what the rule actually requires

Last reviewed: 2026-08-10

Every vendor in this category has a page telling you to look for encryption, SOC 2, and a signed BAA. That list is not wrong, but it skips the question underneath it: whether the vendor should be receiving your patients' audio at all.

## Two things to get straight first

**Nothing is "HIPAA certified."** HHS certifies no product, service, or vendor, and no standard requires a covered entity to obtain certification. A private audit can be useful evidence of diligence, but it carries no government recognition and does not stop OCR finding a violation afterward. A HIPAA badge on a vendor page is marketing, not a legal status.

**Compliance is a property of the arrangement, not the software.** No tool can be compliant on your behalf, because most obligations are about what your organization does: who can access the files, whether the disk is encrypted, whether staff are trained, whether the patient authorized the disclosure. The question is never "is this app HIPAA compliant." It is "who ends up holding this audio, and under what contract."

## The rule as written

Under **45 CFR 160.103**, a business associate is a person who, on behalf of a covered entity,

> "creates, receives, maintains, or transmits protected health information for a function or activity regulated by this subchapter…"

Read those four verbs carefully; the entire vendor question is decided by them. A transcription provider that takes in your audio *receives* PHI. One that keeps the transcript on its servers *maintains* it. Either verb makes it a business associate, and the Privacy Rule then requires a written contract with satisfactory assurances *before* you hand over the recording.

Note what the definition does not say. It does not turn on encryption strength, data center location, or SOC 2. Those are safeguards a business associate must have; they are not what makes someone a business associate. The trigger is receipt of PHI.

And there is **no software exception**. Paragraph (4) excludes exactly four categories: health care providers receiving treatment disclosures, plan sponsors, government agencies determining eligibility, and covered entities within an organized health care arrangement. Software vendors, cloud providers, and transcription platforms appear nowhere. The narrow "conduit" idea covers mere transmission without access to content, which a service that transcribes and stores your audio plainly exceeds.

## The three architectures

**Human transcription service** — business associate, BAA required. The agency and its transcriptionists receive and store your audio and the finished transcript. Still the model that produces certified transcripts. Ask who subcontracts, where staff are located, and whether the agency signs BAAs with its own vendors.

**Cloud AI transcription** — business associate, BAA required. The vendor receives your audio, processes it on its servers, and usually stores the transcript. Fast and cheap, but you are adding a party that holds PHI. Check the plan tier: most vendors gate BAAs to enterprise, so the same product is appropriate on one tier and impermissible on another.

**On-device transcription** — not a business associate, no BAA. Nobody outside your organization receives the audio; the model runs on a machine you already control. There is no BAA because there is no disclosure to contract about. The tradeoff is that the device becomes the whole security surface, and there is no vendor to hold accountable if you misconfigure it.

## What on-device does not do

Running transcription on your own machine removes one question: whether a third party receives PHI, and therefore whether you need a BAA and inherit that vendor's breach exposure. It removes nothing else.

- You still owe the Security Rule's safeguards: full-disk encryption, screen lock, access control, audit, and a real answer for a stolen laptop
- You still owe workforce training and sanctions, and breach notification if the device is lost
- Consent and authorization rules are untouched; recording a patient encounter is subject to the same state law either way
- Local files are still discoverable, and a subpoena reaches your disk as readily as a vendor's
- You lose a party to hold accountable; a cloud vendor under a BAA carries contractual obligations, your laptop carries none

On-device processing converts a vendor-disclosure problem into a device-security problem. For many clinicians that is a good trade, because the device-security problem is one they already solved for their EHR workstation. It is still a trade, not an exemption.

## Where Minutes fits, and where it does not

Minutes is the third architecture: records on your device, transcribes locally with whisper.cpp, diarizes with local models, writes markdown to a folder you control. No audio is uploaded, so no vendor receives PHI, so there is no BAA to negotiate with us. We are not a business associate because we are never in the path.

The one carve-out, stated exactly: transcript text leaves your machine only if you deliberately configure a provider-backed summarizer, which is off by default. Point it at a cloud model and you have re-created the disclosure this architecture avoids, and that provider's terms govern.

Where it is the wrong tool:

- You need a **certified** transcript for a legal or regulatory filing (that is a human service)
- You want an ambient clinical scribe writing structured notes into your EHR, suggesting codes, or drafting to a SOAP template — Minutes is not a medical scribe and has no EHR integration
- You need vendor-side audit logs and administrative oversight across a practice; local files give you ownership, not centralized governance
- Your compliance posture depends on having a business associate to hold accountable — sometimes that contract is the point

## Sources

- 45 CFR 160.103, business associate definition: https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-160/subpart-A/section-160.103
- 45 CFR 164.502(e), business associate contracts: https://www.ecfr.gov/current/title-45/subtitle-A/subchapter-C/part-164/subpart-E/section-164.502
- HHS FAQ on certifying compliance: https://www.hhs.gov/hipaa/for-professionals/faq/2003/are-we-required-to-certify-our-organizations-compliance-with-the-standards/index.html
- HHS business associate guidance: https://www.hhs.gov/hipaa/for-professionals/privacy/guidance/business-associates/index.html
- HHS sample BAA provisions: https://www.hhs.gov/hipaa/for-professionals/covered-entities/sample-business-associate-agreement-provisions/index.html

Informational, not legal advice. HIPAA analysis is fact-specific, vendor terms and plan gating change, and your own counsel or compliance officer is the one who signs off.

## Related

- Which AI note takers can be used with PHI, vendor by vendor: https://useminutes.app/resources/hipaa-compliant-ai-note-taker
- What touches the network: https://useminutes.app/security
